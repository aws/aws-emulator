// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `EngineVersion` filter of
//! `DescribeOrderableDBInstanceOptions` — "a filter to include only the available options
//! for the specified engine version".
//!
//! Every call goes over the wire through the same router the production emulator mounts
//! (`crate::db_instances::emulator`), as a real awsQuery form POST.
//!
//! The rule under test: the report describes the engine version the request NAMES. This
//! emulator's engine-version catalogue is a sample of the real service's live version list,
//! never a claim about which releases exist, so a release INSIDE a major version line that
//! catalogue publishes is described with that line's options even where the catalogue carries
//! no row for it — where that line's own releases are written as plain release numbers. What
//! is not described is a release of a line the engine does not publish, a plain number against
//! a line whose releases carry a qualifier (an Aurora release, a SQL Server build, an Extended
//! Support build), the name of a LINE on its own (the member is an exact filter), a release the
//! catalogue reports as deprecated, a value that is not an engine version at all, and a release
//! outside the version list of an engine whose list the SERVICE defines and this emulator
//! carries whole — the engines of the sibling control planes this one front door serves.
//!
//! The rules are stated over TABLES: the engines are rows (cluster-storage and
//! per-instance-storage engines, three licence families and two version-string forms), the
//! probed version is derived from each engine's own published lines rather than hard-coded,
//! and each answer is cross-checked against the options the line's published release is
//! offered.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::db_engine_versions::{all_versions_of, Version};
use crate::db_instances::{call, emulator, expect_ok, optional_text, subtrees, texts, REGION};
// The option projection, the paging loop and the published per-storage-type bounds this
// suite reads are the ones the sibling orderable-options suite already owns, so the two
// suites cannot disagree about what an option is or about AWS's published storage table.
use crate::orderable_db_instance_options::{
    all_options, engine_max_allocated_storage, options, published_min_storage_size,
    storage_rule, OrderableOption,
};

/// The operation under test.
const ACTION: &str = "DescribeOrderableDBInstanceOptions";

/// The engines this suite spans: the MySQL, MariaDB and PostgreSQL engines whose storage is
/// ordered with the instance, an Aurora engine whose cluster manages it, and a SQL Server
/// edition, whose published versions are written in another form (`15.00.4382.1.v1`) — so
/// every branch of the line rule is a row, including engines the issue that prompted this
/// suite did not cite.
const ENGINES: [&str; 5] = [
    "aurora-mysql",
    "mariadb",
    "mysql",
    "postgres",
    "sqlserver-ee",
];

/// `Status` of a version the engine catalogue still offers.
const STATUS_AVAILABLE: &str = "available";

/// `Status` of a version the engine catalogue no longer offers.
const STATUS_DEPRECATED: &str = "deprecated";

/// The report of one engine filtered to one engine version, paged to exhaustion.
async fn options_for_version(
    router: &MultiServiceRouter,
    case: &str,
    engine: &str,
    engine_version: &str,
) -> Vec<OrderableOption> {
    all_options(
        router,
        case,
        &[
            ("Engine", engine.to_string()),
            ("EngineVersion", engine_version.to_string()),
        ],
    )
    .await
}

/// What an option is offered as, without the engine version it is offered FOR: the
/// combination a release of a line and the line's published release must be offered the same
/// set of.
fn combinations(offered: &[OrderableOption]) -> BTreeSet<(String, String, String)> {
    offered
        .iter()
        .map(|option| {
            (
                option.db_instance_class.clone(),
                option.license_model.clone(),
                option.storage_type.clone(),
            )
        })
        .collect()
}

/// The versions the engine catalogue reports as available for `engine`.
async fn available(router: &MultiServiceRouter, engine: &str) -> Vec<Version> {
    all_versions_of(router, engine)
        .await
        .into_iter()
        .filter(|version| version.status == STATUS_AVAILABLE)
        .collect()
}

/// A release of the major version line `major` that the engine catalogue carries no row for.
///
/// Built from the line itself, so it is a release of a line the engine publishes whatever the
/// catalogue samples of that line: the line, a dot, and a release number past the ones any
/// engine of this service publishes.
fn release_outside_the_catalogue(major: &str) -> String {
    format!("{major}.997")
}

/// Whether `version` is written as a PLAIN release number — dot-separated numbers and nothing
/// else — which is the form a release of a line must be written in to be a release of that
/// line at all.
///
/// A line whose releases carry a lettered qualifier publishes the FORM of its releases as
/// surely as it publishes the releases themselves: Aurora MySQL writes the Aurora release into
/// the string (`8.0.mysql_aurora.3.08.0`), Microsoft SQL Server its build and revision
/// (`15.00.4382.1.v1`) and an Amazon RDS Extended Support build the day it was cut
/// (`11.22-rds.20241121`), so a bare number is no release of such a line however well it
/// matches its leading digits.
fn is_a_plain_release_number(version: &str) -> bool {
    !version.is_empty()
        && version
            .split('.')
            .all(|component| !component.is_empty() && component.bytes().all(|c| c.is_ascii_digit()))
}

/// The `MinStorageSize` / `MaxStorageSize`, in GiB, a DB instance whose storage is ordered
/// with it reports for `storage_type`.
///
/// Both come from the sibling suite's published tables: the minimum from `STORAGE_RULES` for
/// the (engine, type) pair — `gp2` publishes the ENGINE's own floor and the later types their
/// own — and the maximum from the largest `AllocatedStorage` the ENGINE accepts
/// (`engine_max_allocated_storage`). They are a property of the (engine, storage type) pair
/// and of neither the class nor the engine VERSION, so a release inside a published line
/// reports the same bounds as the line's published release.
fn published_storage_bounds(storage_type: &str, engine: &str) -> (i32, i32) {
    let min = published_min_storage_size(storage_rule(storage_type), engine).unwrap_or_else(|| {
        panic!("a per-instance storage type publishes a minimum size: {storage_type}")
    });
    (min, engine_max_allocated_storage(engine))
}

#[tokio::test]
async fn an_engine_whose_version_list_the_service_defines_keeps_the_empty_report() {
    let router = emulator();

    // The engines of the two sibling control planes this one front door serves: their version
    // list is one the SERVICE publishes and this emulator carries whole, rather than a sample
    // of the upstream engine's releases, so a release outside it is one that engine cannot run
    // — and describing one here would make this view of the door disagree with that engine's
    // own view of it.
    for engine in ["docdb", "neptune"] {
        let published = available(&router, engine).await;
        for version in &published {
            let probe = release_outside_the_catalogue(&version.major_engine_version);
            if published
                .iter()
                .any(|published| published.engine_version == probe)
            {
                continue;
            }
            let case = format!(
                "{engine} {probe} (line {}, released as {})",
                version.major_engine_version, version.engine_version
            );
            let body = expect_ok(
                &case,
                call(
                    &router,
                    ACTION,
                    &[
                        ("Engine", engine.to_string()),
                        ("EngineVersion", probe.clone()),
                    ],
                )
                .await,
            );
            assert!(
                options(&body).is_empty() && optional_text(&body, "Marker").is_none(),
                "case {case:?}: expected an empty report with no Marker, got {body}"
            );
            // The release the service DOES publish is still offered, so the report is empty
            // for the probe rather than for the engine.
            let offered =
                options_for_version(&router, &case, engine, &version.engine_version).await;
            assert!(
                !offered.is_empty(),
                "case {case:?}: the published release {} must still be offered",
                version.engine_version
            );
        }
    }
}

/// The members of an option that are the VERSION's rather than the engine's or the storage
/// type's: what a release inside a published line inherits from its line's capability profile
/// ([`a_release_inside_a_published_line_is_offered_that_lines_options`] holds them to the
/// line's published release).
///
/// Source: `OrderableDBInstanceOption`'s own members — the read-replica capability, the engine
/// modes the version is offered in, whether it can back a global database and whether a
/// cluster of it has an HTTP endpoint to turn on.
const VERSION_CAPABILITY_MEMBERS: [&str; 4] = [
    "ReadReplicaCapable",
    "SupportedEngineModes",
    "SupportsGlobalDatabases",
    "SupportsHttpEndpoint",
];

/// One member of an option's XML as a comparable string: its text where the member is a
/// scalar, and the members of its list joined where it is a list (`SupportedEngineModes`).
fn member_text(xml: &str, member: &str) -> String {
    match subtrees(xml, member).first() {
        Some(subtree) if !texts(subtree, "member").is_empty() => texts(subtree, "member").join(","),
        _ => optional_text(xml, member).unwrap_or_default(),
    }
}

#[tokio::test]
async fn a_release_inside_a_published_line_is_offered_that_lines_options() {
    let router = emulator();

    for engine in ENGINES {
        let published = available(&router, engine).await;
        assert!(
            !published.is_empty(),
            "case {engine:?}: an engine this service publishes versions for must have some"
        );
        // One row per published line of the engine, so the rule is asserted over every line
        // rather than over one of them.
        let lines: BTreeSet<String> = published
            .iter()
            .map(|version| version.major_engine_version.clone())
            .collect();
        for line in lines {
            let probe = release_outside_the_catalogue(&line);
            assert!(
                !published
                    .iter()
                    .any(|version| version.engine_version == probe),
                "case {engine:?} {line:?}: the probe {probe:?} must be a release the \
                 catalogue carries no row for"
            );
            // The release the line's capabilities are read from: the last one the catalogue
            // publishes for the line that is written as a plain release number. A line that
            // writes an Aurora release, a SQL Server build or an Extended Support build into
            // its version strings publishes no release a bare number names, so the probe is
            // no release of it and describes nothing.
            let plain_release = published
                .iter()
                .rfind(|version| {
                    version.major_engine_version == line
                        && is_a_plain_release_number(&version.engine_version)
                })
                .map(|version| version.engine_version.clone());

            let case = format!("{engine} {probe} (line {line}, plain release {plain_release:?})");
            let offered = options_for_version(&router, &case, engine, &probe).await;
            let Some(release) = plain_release else {
                assert!(
                    offered.is_empty(),
                    "case {case:?}: a plain number is no release of a line whose releases \
                     all carry a qualifier, so it must be offered nothing, got {} options",
                    offered.len()
                );
                continue;
            };
            assert!(
                !offered.is_empty(),
                "case {case:?}: a release of a line this engine publishes must be offered \
                 the line's options, got an empty report"
            );
            for option in &offered {
                assert_eq!(
                    (option.engine.as_str(), option.engine_version.as_str()),
                    (engine, probe.as_str()),
                    "case {case:?}: every option must be offered for the engine and the \
                     version the request named, got {}",
                    option.xml
                );
            }

            // The capabilities of a release are its LINE's, so the release the catalogue
            // publishes for the line and a release inside it are offered the same
            // combinations of class, licence model and storage type.
            let line_release = options_for_version(
                &router,
                &format!("{engine} {release}"),
                engine,
                &release,
            )
            .await;
            assert_eq!(
                combinations(&offered),
                combinations(&line_release),
                "case {case:?}: a release of the line must be offered the same combinations \
                 as the line's published release {release:?}"
            );
            // And the VERSION-level capability members, which are what the sampled release
            // inherits from the line rather than deriving from the engine: every one of them
            // must read as the line's published release reports it.
            for member in VERSION_CAPABILITY_MEMBERS {
                let sampled: Vec<String> = offered
                    .iter()
                    .map(|option| member_text(&option.xml, member))
                    .collect();
                let published_by_the_line: Vec<String> = line_release
                    .iter()
                    .map(|option| member_text(&option.xml, member))
                    .collect();
                assert_eq!(
                    sampled, published_by_the_line,
                    "case {case:?}: {member} of a release of the line must be the one its \
                     published release {release:?} reports"
                );
            }
        }
    }
}

#[tokio::test]
async fn naming_a_major_version_line_alone_selects_nothing() {
    let router = emulator();

    // `EngineVersion` is an exact filter — "a filter to include only the available options
    // for the specified engine version" — so the NAME OF A LINE is no engine version and
    // selects no option, while every release of that line still does.
    for engine in ENGINES {
        let published = available(&router, engine).await;
        let lines: BTreeSet<String> = published
            .iter()
            .map(|version| version.major_engine_version.clone())
            .collect();
        for line in lines {
            let case = format!("{engine} line {line}");
            let body = expect_ok(
                &case,
                call(
                    &router,
                    ACTION,
                    &[
                        ("Engine", engine.to_string()),
                        ("EngineVersion", line.clone()),
                    ],
                )
                .await,
            );
            assert!(
                options(&body).is_empty() && optional_text(&body, "Marker").is_none(),
                "case {case:?}: the name of a line is no engine version, so it must select \
                 no option, got {body}"
            );
            let release = published
                .iter()
                .find(|version| version.major_engine_version == line)
                .expect("the line was read from the published versions")
                .engine_version
                .clone();
            let offered = options_for_version(
                &router,
                &format!("{engine} {release}"),
                engine,
                &release,
            )
            .await;
            assert!(
                !offered.is_empty(),
                "case {case:?}: the published release {release:?} of the line must be offered \
                 options"
            );
        }
    }
}

#[tokio::test]
async fn a_value_that_names_no_release_of_a_published_line_is_offered_nothing() {
    let router = emulator();

    for engine in ENGINES {
        let published = available(&router, engine).await;
        let a_line = published
            .first()
            .expect("an engine of this suite publishes versions")
            .major_engine_version
            .clone();
        let every_available: BTreeSet<String> = published
            .iter()
            .map(|version| version.engine_version.clone())
            .collect();
        let deprecated: Vec<String> = all_versions_of(&router, engine)
            .await
            .into_iter()
            .filter(|version| version.status == STATUS_DEPRECATED)
            .map(|version| version.engine_version)
            .collect();

        // (case, the value `EngineVersion` carries)
        let mut rows: Vec<(String, String)> = vec![
            (
                "a line this engine does not publish".to_string(),
                "99.9.9".to_string(),
            ),
            (
                // The line is a PREFIX of this value, and this value is no release of it:
                // the line is followed by more of a number rather than by a release.
                "a longer line number that starts with a published one".to_string(),
                format!("{a_line}9.1"),
            ),
            (
                "a value that is not an engine version at all".to_string(),
                "not-a-version".to_string(),
            ),
            (
                "a value past the longest engine version".to_string(),
                format!("{a_line}.{}", "9".repeat(80)),
            ),
        ];
        if let Some(version) = deprecated.first() {
            rows.push((
                "a release this engine no longer offers".to_string(),
                version.clone(),
            ));
        }

        for (case, value) in rows {
            let case = format!("{engine}: {case} ({value})");
            assert!(
                !every_available.contains(&value),
                "case {case:?}: the probe must not be a release the catalogue offers"
            );
            let body = expect_ok(
                &case,
                call(
                    &router,
                    ACTION,
                    &[
                        ("Engine", engine.to_string()),
                        ("EngineVersion", value.clone()),
                    ],
                )
                .await,
            );
            assert!(
                options(&body).is_empty() && optional_text(&body, "Marker").is_none(),
                "case {case:?}: expected an empty report with no Marker, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_sizing_client_reads_the_storage_and_zones_of_the_version_it_named() {
    let router = emulator();

    // What a pre-flight check does: name the engine, the release and the class it is about
    // to create, and read the storage range and zones that class is orderable with. The
    // first row is the recorded case; the others differ along engine, release form, class
    // family and class size, and the last names a release the catalogue carries a row for,
    // so the exact-match reading is a row too.
    //
    // (engine, EngineVersion, DBInstanceClass, the storage types the class is offered with)
    type Row = (&'static str, &'static str, &'static str, &'static [&'static str]);
    const PER_INSTANCE_STORAGE: &[&str] = &["gp2", "gp3", "io1", "io2"];
    const CLUSTER_STORAGE: &[&str] = &["aurora", "aurora-iopt1"];
    let rows: [Row; 5] = [
        ("mysql", "8.4.9", "db.c6gd.12xlarge", PER_INSTANCE_STORAGE),
        ("mysql", "8.4.9", "db.r5d.4xlarge", PER_INSTANCE_STORAGE),
        ("mysql", "8.0.41", "db.x2iedn.xlarge", PER_INSTANCE_STORAGE),
        ("postgres", "17.7", "db.t4g.micro", PER_INSTANCE_STORAGE),
        (
            "aurora-mysql",
            "8.0.mysql_aurora.3.10.3",
            "db.r6g.large",
            CLUSTER_STORAGE,
        ),
    ];

    for (engine, engine_version, class, storage_types) in rows {
        let case = format!("{engine} {engine_version} on {class}");
        let offered = all_options(
            &router,
            &case,
            &[
                ("Engine", engine.to_string()),
                ("EngineVersion", engine_version.to_string()),
                ("DBInstanceClass", class.to_string()),
            ],
        )
        .await;
        let reported: Vec<String> = offered
            .iter()
            .map(|option| option.storage_type.clone())
            .collect();
        assert_eq!(
            reported, storage_types,
            "case {case:?}: the class must be offered once per storage type it can be \
             ordered with, got {reported:?}"
        );
        for option in &offered {
            assert_eq!(
                (
                    option.engine.as_str(),
                    option.engine_version.as_str(),
                    option.db_instance_class.as_str()
                ),
                (engine, engine_version, class),
                "case {case:?}: an option answers the request's own engine, version and \
                 class, got {}",
                option.xml
            );

            // The zones and the storage range a caller sizes an instance from: the zones of
            // the region the request was signed for, and the published bounds of the option's
            // own storage type for an engine whose storage is ordered with the instance.
            let zones = subtrees(&option.xml, "AvailabilityZones");
            assert_eq!(
                texts(&zones[0], "Name"),
                ::service_common::aws_availability_zones::availability_zones(REGION),
                "case {case:?}: an option's zones are the zones the signed-for region \
                 publishes, got {}",
                option.xml
            );
            let bounds = (
                optional_text(&option.xml, "MinStorageSize"),
                optional_text(&option.xml, "MaxStorageSize"),
            );
            let cluster_managed = storage_types == CLUSTER_STORAGE;
            assert_eq!(
                bounds.0.is_none() && bounds.1.is_none(),
                cluster_managed,
                "case {case:?}: storage bounds are reported exactly where the storage is \
                 ordered with the instance, got {bounds:?}"
            );
            if !cluster_managed {
                let min: i32 = bounds
                    .0
                    .as_deref()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| panic!("case {case:?}: MinStorageSize is an integer"));
                let max: i32 = bounds
                    .1
                    .as_deref()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| panic!("case {case:?}: MaxStorageSize is an integer"));
                assert_eq!(
                    (min, max),
                    published_storage_bounds(&option.storage_type, engine),
                    "case {case:?}: the published bounds of {} for this engine, got \
                     {min}..{max}",
                    option.storage_type
                );
            }
        }
    }
}
