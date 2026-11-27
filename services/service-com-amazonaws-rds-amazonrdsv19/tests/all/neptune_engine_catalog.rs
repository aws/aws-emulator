// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GRAPH engine's half of the two published engine reports this
//! front door answers.
//!
//! This one front door serves three control planes (`src/service.rs`): Amazon RDS, Amazon
//! DocumentDB and Amazon Neptune all publish the same wire identity, so a Neptune client
//! signs `rds` and its catalog reads land here. The rule these tables pin is therefore the
//! same rule the RDS engines already obey, applied to the engine of a control plane this
//! door serves on behalf of another API:
//!
//! * `DescribeDBEngineVersions` publishes that engine's OWN release line — every
//!   maintained minor family, each family's releases in ascending order, each row carrying
//!   that family's DB parameter group family, that engine's product name in its version
//!   description, and an upgrade chain onto its later versions;
//! * `DescribeOrderableDBInstanceOptions` offers an option for every version that report
//!   calls available, under that engine's OWN licence model and storage type.
//!
//! The rows span the engine's major-version families, which is what the rule generalises
//! over: the family name form changes across them (`neptune1` for the first,
//! `neptune<major>` from the second on), the per-version capability changes across them
//! (a global database is offered from the second family on), and each family carries a
//! different number of releases.
//!
//! The harness (`emulator`, `call`, `expect_ok` and the element readers) is the
//! [`crate::db_instances`] suite's, reused rather than copied.

use crate::db_instances::{call, emulator, expect_ok, subtrees, text, texts};

/// The engine this suite is about: the graph engine of the Amazon Neptune control plane
/// this front door serves. It is not a value this model's own `Engine` member lists, which
/// is exactly why it needs its own coverage here.
const ENGINE: &str = "neptune";

/// One published major-version family of [`ENGINE`].
struct Family {
    /// `MajorEngineVersion`, and the value `EngineVersion` takes to narrow the report to
    /// this family.
    major: &'static str,
    /// The `DBParameterGroupFamily` every version of this family carries.
    parameter_group_family: &'static str,
    /// Whether a cluster of this family can join a global database
    /// (`SupportsGlobalDatabases`).
    global_databases: bool,
    /// The lowest number of releases this family publishes. A family publishes its own
    /// release line rather than one sample of it, so it offers at least one minor upgrade.
    least_releases: usize,
}

/// The published major-version families, oldest first.
///
/// Rows differ along every axis the rule generalises over: the FORM of the parameter group
/// family name, the global-database capability, and the number of releases. `1.2` and
/// `1.4` are families the issue cited nothing about.
const FAMILIES: &[Family] = &[
    Family {
        major: "1.1",
        parameter_group_family: "neptune1",
        global_databases: false,
        least_releases: 2,
    },
    Family {
        major: "1.2",
        parameter_group_family: "neptune1.2",
        global_databases: true,
        least_releases: 2,
    },
    Family {
        major: "1.3",
        parameter_group_family: "neptune1.3",
        global_databases: true,
        least_releases: 2,
    },
    Family {
        major: "1.4",
        parameter_group_family: "neptune1.4",
        global_databases: true,
        least_releases: 2,
    },
];

/// The `DBEngineDescription` every version of this engine carries: the product's own full
/// name, as distinct from the product name a version description is built from.
const ENGINE_DESCRIPTION: &str = "Amazon Neptune";

/// The product name this engine writes into `DBEngineVersionDescription`, before the
/// RELEASE of the version it describes (`Neptune 1.4.8.0.R1`): this engine ships releases
/// of a version, and the description names the release the service runs
/// (`::service_common::neptune_engine_releases`).
const VERSION_DESCRIPTION_PRODUCT: &str = "Neptune";

/// The description this engine publishes for `version`: its product and the RELEASE of that
/// version, read from the one shared table of its published releases so this suite states
/// the rule rather than a second copy of the builds.
///
/// The published BUILDS themselves are pinned as literals by the sibling suite
/// ([`crate::graph_engine_published_classes`]), and by [`A_PUBLISHED_RELEASE`] below, so an
/// entry described from this table cannot drift silently.
fn version_description(version: &str) -> String {
    format!(
        "{VERSION_DESCRIPTION_PRODUCT} {}",
        ::service_common::neptune_engine_releases::release(version)
    )
}

/// One release of this engine as a LITERAL: the version, and the description the report
/// publishes for it. It is the oldest published release, whose current build is a later one
/// than the first, so this suite fails on a wrong published build rather than only on the two
/// planes disagreeing.
const A_PUBLISHED_RELEASE: (&str, &str) = ("1.1.1.0", "Neptune 1.1.1.0.R7");

/// The one licence model this engine's orderable options are offered under: it is licensed
/// by AWS itself rather than by a database vendor.
///
/// `pub(crate)` so the sibling suite that spans this engine's published CLASSES
/// ([`crate::graph_engine_published_classes`]) reads the same constant rather than a second
/// copy of it.
pub(crate) const LICENSE_MODEL: &str = "amazon-license";

/// The storage types a cluster member of this engine is offered with, in the order its own
/// `CreateDBCluster$StorageType` publishes them ("Valid values: standard, iopt1"): the
/// CLUSTER owns the storage, so the instance orders no size or IOPS of its own, but the type
/// is part of what an order picks and each of them is its own option.
///
/// `pub(crate)` for the same reason as [`LICENSE_MODEL`].
pub(crate) const STORAGE_TYPES: [&str; 2] = ["standard", "iopt1"];

/// The network types an option of this engine is offered for: the values `NetworkType`
/// publishes for a DB cluster of this front door, in the order that value list names them
/// ("IPV4 | DUAL"), which is the order an option reports them in.
const NETWORK_TYPES: [&str; 2] = ["IPV4", "DUAL"];

/// Read one engine-version entry's scalar members, with the `<ValidUpgradeTarget>`
/// subtrees removed: a target carries its own `<Engine>` and `<EngineVersion>`, so the
/// entry's own members can only be read off the entry with its targets stripped.
///
/// `pub(crate)` for the same reason as [`LICENSE_MODEL`]: one reader of an entry for both
/// suites of this engine.
pub(crate) fn scalars_of(entry: &str) -> String {
    let mut scalars = entry.to_string();
    for target in subtrees(entry, "ValidUpgradeTarget") {
        scalars = scalars.replace(&target, "");
    }
    scalars
}

#[tokio::test]
async fn every_published_family_of_the_graph_engine_reports_its_own_release_line() {
    let router = emulator();

    // The one release this suite pins as a LITERAL ([`A_PUBLISHED_RELEASE`]), so the
    // description rule below is checked against a published build and not only against the
    // table the handler reads.
    let (pinned_version, pinned_description) = A_PUBLISHED_RELEASE;
    assert_eq!(
        version_description(pinned_version),
        pinned_description,
        "the report must publish {pinned_description} for {pinned_version}"
    );

    for family in FAMILIES {
        let case = format!("{ENGINE} {}", family.major);
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBEngineVersions",
                &[("Engine", ENGINE), ("EngineVersion", family.major)],
            )
            .await,
        );
        let entries = subtrees(&body, "DBEngineVersion");
        assert!(
            entries.len() >= family.least_releases,
            "case {case:?}: expected at least {} releases in this family, got {} in {body}",
            family.least_releases,
            entries.len()
        );

        let mut versions: Vec<String> = Vec::new();
        for entry in &entries {
            let scalars = scalars_of(entry);
            let version = text(&scalars, "EngineVersion");
            assert_eq!(
                text(&scalars, "Engine"),
                ENGINE,
                "case {case:?}: every entry is of the engine asked for, got {scalars}"
            );
            assert_eq!(
                text(&scalars, "MajorEngineVersion"),
                family.major,
                "case {case:?}: every entry belongs to the family asked for, got {scalars}"
            );
            assert_eq!(
                text(&scalars, "DBParameterGroupFamily"),
                family.parameter_group_family,
                "case {case:?}: expected the family's own DB parameter group family, got \
                 {scalars}"
            );
            assert_eq!(
                text(&scalars, "DBEngineDescription"),
                ENGINE_DESCRIPTION,
                "case {case:?}: expected this engine's own description, got {scalars}"
            );
            assert_eq!(
                text(&scalars, "DBEngineVersionDescription"),
                version_description(&version),
                "case {case:?}: expected the version description to name this engine's \
                 product and the release of that version, got {scalars}"
            );
            assert_eq!(
                text(&scalars, "SupportsGlobalDatabases"),
                family.global_databases.to_string(),
                "case {case:?}: expected SupportsGlobalDatabases \
                 {}, got {scalars}",
                family.global_databases
            );
            // A reader of one of this engine's clusters is another member of that cluster,
            // not an RDS read replica: its control plane models no
            // `CreateDBInstanceReadReplica` at all, so no version of it claims one — the
            // same answer every Aurora version gives (`db_engine_versions.rs`).
            assert_eq!(
                text(&scalars, "SupportsReadReplica"),
                "false",
                "case {case:?}: a cluster member is a reader of its cluster rather than \
                 an RDS read replica, got {scalars}"
            );
            // Every release of this engine is a four-part release number.
            let parts: Vec<&str> = version.split('.').collect();
            assert!(
                parts.len() == 4 && parts.iter().all(|part| part.parse::<u32>().is_ok()),
                "case {case:?}: expected a four-part release number, got {version:?}"
            );
            versions.push(version);
        }

        let mut ascending = versions.clone();
        ascending.sort_by_key(|version| release_number(version));
        assert_eq!(
            versions, ascending,
            "case {case:?}: a family's releases are reported in ascending order, got \
             {versions:?}"
        );

        // Every release but the family's latest can be upgraded to the next one, and the
        // upgrade stays inside the family, so it is a MINOR upgrade.
        for (index, entry) in entries.iter().enumerate() {
            let version = &versions[index];
            let targets = subtrees(entry, "UpgradeTarget");
            let Some(next) = versions.get(index + 1) else {
                continue;
            };
            let within = targets
                .iter()
                .find(|target| &text(target, "EngineVersion") == next)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {version} must be upgradable to {next}, got {entry}"
                    )
                });
            assert_eq!(
                text(within, "IsMajorVersionUpgrade"),
                "false",
                "case {case:?}: {version} -> {next} stays in one family, got {within}"
            );
            assert_eq!(
                text(within, "Description"),
                version_description(next),
                "case {case:?}: an upgrade target describes itself the way the entry for \
                 that version does, got {within}"
            );
        }
    }
}

#[tokio::test]
async fn one_release_of_the_graph_engine_is_reported_on_its_own() {
    let router = emulator();

    // A release the issue cited nothing about, named exactly rather than by its family:
    // `EngineVersion` narrows the report to one entry.
    let case = "one release by its full version";
    let body = expect_ok(
        case,
        call(
            &router,
            "DescribeDBEngineVersions",
            &[("Engine", ENGINE), ("EngineVersion", "1.3.3.0")],
        )
        .await,
    );
    let entries = subtrees(&body, "DBEngineVersion");
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected exactly the release named, got {body}"
    );
    let scalars = scalars_of(&entries[0]);
    assert_eq!(
        text(&scalars, "EngineVersion"),
        "1.3.3.0",
        "case {case:?}: expected the release named, got {scalars}"
    );

    // Its upgrade chain reaches the LATER families as major-version upgrades, which is
    // what a client computing an upgrade path reads.
    let targets = subtrees(&entries[0], "UpgradeTarget");
    let major_upgrades: Vec<String> = targets
        .iter()
        .filter(|target| text(target, "IsMajorVersionUpgrade") == "true")
        .map(|target| text(target, "EngineVersion"))
        .collect();
    assert!(
        major_upgrades
            .iter()
            .all(|version| version.starts_with("1.4.")),
        "case {case:?}: expected every major-version upgrade of a 1.3 release to be a 1.4 \
         release, got {major_upgrades:?}"
    );
    assert!(
        !major_upgrades.is_empty(),
        "case {case:?}: expected a major-version upgrade onto the later family, got \
         {targets:?}"
    );
}

#[tokio::test]
async fn the_graph_engine_is_orderable_for_every_version_it_publishes() {
    let router = emulator();

    // The versions the engine report calls available, read from that report so the two
    // reports cannot disagree about which versions exist.
    let reported = expect_ok(
        "the published versions",
        call(
            &router,
            "DescribeDBEngineVersions",
            &[("Engine", ENGINE)],
        )
        .await,
    );
    let versions: Vec<String> = subtrees(&reported, "DBEngineVersion")
        .iter()
        .map(|entry| text(&scalars_of(entry), "EngineVersion"))
        .collect();
    assert!(
        versions.len() >= FAMILIES.len(),
        "expected at least one release per published family, got {versions:?}"
    );

    for version in &versions {
        let case = format!("orderable options for {ENGINE} {version}");
        let body = expect_ok(
            &case,
            call(
                &router,
                "DescribeOrderableDBInstanceOptions",
                &[
                    ("Engine", ENGINE),
                    ("EngineVersion", version.as_str()),
                    ("MaxRecords", "1000"),
                ],
            )
            .await,
        );
        let options = subtrees(&body, "OrderableDBInstanceOption");
        assert!(
            !options.is_empty(),
            "case {case:?}: a version this service publishes as available must be \
             orderable, got {body}"
        );
        for option in &options {
            assert_eq!(
                text(option, "Engine"),
                ENGINE,
                "case {case:?}: every option is of the engine asked for, got {option}"
            );
            assert_eq!(
                text(option, "EngineVersion"),
                *version,
                "case {case:?}: EngineVersion narrows the report, got {option}"
            );
            assert_eq!(
                text(option, "LicenseModel"),
                LICENSE_MODEL,
                "case {case:?}: expected this engine's own licence model, got {option}"
            );
            let storage_type = text(option, "StorageType");
            assert!(
                STORAGE_TYPES.contains(&storage_type.as_str()),
                "case {case:?}: expected one of the cluster-managed storage types \
                 {STORAGE_TYPES:?}, got {option}"
            );
            assert_eq!(
                text(option, "Vpc"),
                "true",
                "case {case:?}: every option is offered in a VPC, got {option}"
            );
            assert!(
                !texts(option, "AvailabilityZone").is_empty(),
                "case {case:?}: an option names the zones it is offered in, got {option}"
            );
            let network_types = subtrees(option, "SupportedNetworkTypes");
            assert_eq!(
                network_types.len(),
                1,
                "case {case:?}: the network types are reported once, got {option}"
            );
            assert_eq!(
                texts(&network_types[0], "member"),
                NETWORK_TYPES
                    .iter()
                    .map(|network_type| (*network_type).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected the network types a cluster of this engine can be \
                 placed on, got {option}"
            );
        }
    }
}

/// A four-part release number as the tuple it sorts by, so `1.3.10.0` sorts ABOVE the
/// earlier `1.3.9.0` where the version STRING would not.
fn release_number(version: &str) -> (u32, u32, u32, u32) {
    let mut parts = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}
