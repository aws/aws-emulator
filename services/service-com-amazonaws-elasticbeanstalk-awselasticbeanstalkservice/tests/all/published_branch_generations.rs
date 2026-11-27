// SPDX-License-Identifier: CC0-1.0

//! The GENERATIONS `ListPlatformBranches` publishes per platform.
//!
//! The rule under test: for each `PlatformName`, the operation reports every
//! branch generation Elastic Beanstalk has published for that platform — the
//! current ones, the PREVIEW branch published beside a general branch of the
//! same generation, and the operating-system generations behind them (the
//! Amazon Linux 2 and Amazon Linux ones, the 32-bit Amazon Linux ones, the
//! preconfigured-Docker ones on Debian, the Windows Server generations before
//! IIS 10.0) — each one
//!
//! * under the branch name the service publishes, carrying the runtime
//!   qualifier (the Java version of a preconfigured-Docker runtime, the
//!   application server an Amazon Linux Ruby branch is served behind) and the
//!   operating-system generation, with no distribution code name;
//! * with the life-cycle state the service publishes for it, so a generation
//!   still receiving releases reads `Supported`, the one behind it `Deprecated`
//!   and the ones that have reached end of life `Retired`;
//! * with `BranchOrder` — "a larger `BranchOrder` value designates a newer
//!   platform branch within the platform" — issued per platform in steps of
//!   100, so an older operating-system generation's branches never outrank a
//!   newer generation's, and a preview branch carries the ordinal of the
//!   general branch it previewed (or, where that branch was later issued one of
//!   its own, the step below it);
//! * with `SupportedTierList` holding both documented tiers for a Linux branch
//!   and the web server tier alone for a Windows one.
//!
//! Every assertion spans a SET of inputs: the table rows below are one per
//! shape of generation, read off Elastic Beanstalk's published platform
//! inventory and mostly branches no divergence report cited, and the invariants
//! (preview pairing, ordinal bands, branch-name uniqueness, preconfigured-Docker
//! shape) are asserted over the WHOLE catalog the server reports, so they hold
//! as it grows.
//!
//! The HTTP harness and the branch reader are [`crate::lifecycle`]'s and
//! [`crate::platform_branches`]', so this suite adds no second copy of either.

use std::collections::BTreeMap;

use crate::lifecycle::Emulator;
use crate::platform_branches::{
    expected_tiers, full_catalog, of_platform, Branch, CatalogRow, BRANCH_ORDER_STEP,
};

/// The preview prefix a branch published as a PREVIEW of a general branch of
/// the same generation carries, as the catalog spells it.
const PREVIEW_PREFIX: &str = "(BETA) ";

/// The prefix of a preconfigured-Docker branch name: a legacy platform whose
/// versions shipped the runtime inside an image Elastic Beanstalk built.
const PRECONFIGURED_DOCKER_PREFIX: &str = "Preconfigured Docker - ";

/// The preview branches the service publishes one ordinal STEP below the
/// general branch they previewed, rather than at its ordinal: the general
/// branch was re-issued an ordinal of its own after the preview.
///
/// SOURCE: the recorded `Node.js` inventory, where the preview sits at 11000
/// and the general `Node.js 10` branch at 11100. Every other preview the
/// catalog holds shares its general branch's ordinal, which is what
/// [`a_preview_branch_is_published_beside_the_general_branch_it_previewed`]
/// requires of them.
const PREVIEWS_A_STEP_BELOW: &[&str] = &["(BETA) Node.js 10 running on 64bit Amazon Linux 2"];

/// One branch per shape of published generation.
///
/// SOURCE: the "Elastic Beanstalk supported platforms" and "Retired platform
/// branches" documents — the inventory the real `ListPlatformBranches`
/// enumerates. The rows span a Windows Server generation older than the IIS
/// 10.0 ones, the Windows generation that has deprecated but not retired, a
/// preconfigured-Docker branch on Debian and one on Amazon Linux, the
/// ECS-managed and multi-container Docker branches, a Java SE branch of each
/// generation of the one platform that publishes them, a general Amazon Linux 2
/// branch that shares its ordinal with a preview branch, the oldest Amazon Linux
/// generation of a runtime, an Amazon Linux Ruby branch named after its
/// application server, and the newest Tomcat generation.
const GENERATION_ROWS: &[CatalogRow] = &[
    CatalogRow {
        platform_name: ".NET on Windows Server",
        branch_name: "IIS 8.5 running on 64bit Windows Server 2012 R2",
        lifecycle_state: "Retired",
        branch_order: 9600,
        both_tiers: false,
    },
    CatalogRow {
        platform_name: ".NET on Windows Server",
        branch_name: "IIS 10.0 running on 64bit Windows Server 2016",
        lifecycle_state: "Deprecated",
        branch_order: 9800,
        both_tiers: false,
    },
    CatalogRow {
        platform_name: "GlassFish",
        branch_name: "Preconfigured Docker - GlassFish 4.0 with Java 7 running on 64bit Debian",
        lifecycle_state: "Retired",
        branch_order: 9700,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Go",
        branch_name: "Preconfigured Docker - Go 1.3 running on 64bit Debian",
        lifecycle_state: "Retired",
        branch_order: 9800,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Docker",
        branch_name: "Multi-container Docker running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 10000,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Java",
        branch_name: "Corretto 17 running on 64bit Amazon Linux 2023",
        lifecycle_state: "Supported",
        branch_order: 11200,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Java",
        branch_name: "Java 7 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9700,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Node.js",
        branch_name: "Node.js 12 running on 64bit Amazon Linux 2",
        lifecycle_state: "Retired",
        branch_order: 11100,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "PHP",
        branch_name: "PHP 5.4 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9400,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Ruby",
        branch_name: "Passenger Standalone with Ruby 2.4 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9100,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Tomcat",
        branch_name: "Tomcat 11 with Corretto 17 running on 64bit Amazon Linux 2023",
        lifecycle_state: "Supported",
        branch_order: 12600,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Python",
        branch_name: "Preconfigured Docker - Python 3.4 running on 64bit Debian",
        lifecycle_state: "Retired",
        branch_order: 9300,
        both_tiers: true,
    },
];

/// The operating-system generations the catalog publishes branches on, newest
/// first within each operating system.
///
/// SOURCE: the operating-system component of the published branch names, with
/// the ARCHITECTURE taken off: a generation of Amazon Linux published its
/// 32-bit and its 64-bit branches side by side, so the two spellings name ONE
/// generation and rank together here. The RANK is what the ordinal bands are
/// asserted against: a branch of a later entry here is of an older generation
/// than a branch of an earlier one.
const GENERATIONS: &[&str] = &[
    "Amazon Linux 2023",
    "Amazon Linux 2",
    "Amazon Linux",
    "Debian",
    "Windows Server 2025",
    "Windows Server Core 2025",
    "Windows Server 2022",
    "Windows Server Core 2022",
    "Windows Server 2019",
    "Windows Server Core 2019",
    "Windows Server 2016",
    "Windows Server Core 2016",
    "Windows Server 2012 R2",
    "Windows Server Core 2012 R2",
    "Windows Server 2012",
    "Windows Server 2008 R2",
    "Windows Server",
];

/// The architectures a published branch name states, which are not generations
/// of their own (see [`GENERATIONS`]).
const ARCHITECTURES: &[&str] = &["64bit ", "32bit "];

/// The operating system `branch_name` names, and how old its generation is: 0
/// is the newest generation of its operating system, and a larger number an
/// older one. A branch name that names no operating system (the custom-platform
/// builder) has no generation.
fn generation_of(branch_name: &str) -> Option<(&'static str, usize)> {
    let (_, operating_system) = branch_name.split_once(" running on ")?;
    let generation = ARCHITECTURES
        .iter()
        .find_map(|architecture| operating_system.strip_prefix(*architecture))
        .unwrap_or(operating_system);
    let rank = GENERATIONS
        .iter()
        .position(|published| *published == generation)
        .unwrap_or_else(|| {
            panic!(
                "case {branch_name:?}: the branch names the operating-system generation \
                 {generation:?}, which is not one the published inventory lists"
            )
        });
    Some((GENERATIONS[rank], rank))
}

// The tiers a branch reports come from `expected_tiers`, and the branches of
// one platform from `of_platform` — both shared with the other catalog suites
// rather than written a second time here.

#[tokio::test]
async fn each_published_generation_is_reported_under_its_platform() {
    let emulator = Emulator::new();

    for row in GENERATION_ROWS {
        let case = row.branch_name;
        let reported = of_platform(&emulator, row.platform_name).await;
        let branch = reported
            .iter()
            .find(|branch| branch.branch_name == row.branch_name)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the platform {:?} to publish this generation, got {:?}",
                    row.platform_name,
                    reported
                        .iter()
                        .map(|branch| branch.branch_name.clone())
                        .collect::<Vec<String>>()
                )
            });
        assert_eq!(
            branch.lifecycle_state, row.lifecycle_state,
            "case {case:?}: expected LifecycleState {:?}, got {:?}",
            row.lifecycle_state, branch.lifecycle_state
        );
        assert_eq!(
            branch.branch_order, row.branch_order,
            "case {case:?}: expected BranchOrder {}, got {}",
            row.branch_order, branch.branch_order
        );
        assert_eq!(
            branch.supported_tiers,
            row.expected_tiers(),
            "case {case:?}: unexpected SupportedTierList, got {:?}",
            branch.supported_tiers
        );
    }
}

#[tokio::test]
async fn a_preview_branch_is_published_beside_the_general_branch_it_previewed() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // Every preview branch the catalog holds is a case here, so the rule spans
    // each platform that published one.
    let previews: Vec<&Branch> = catalog
        .iter()
        .filter(|branch| branch.branch_name.starts_with(PREVIEW_PREFIX))
        .collect();
    assert!(
        previews.len() >= 2,
        "case \"previews\": expected the preview branches of the Amazon Linux 2 generation, got \
         {previews:?}"
    );

    for preview in previews {
        let case = preview.key();
        let general_name = preview
            .branch_name
            .strip_prefix(PREVIEW_PREFIX)
            .unwrap_or_default();
        let general = catalog
            .iter()
            .find(|branch| {
                branch.platform_name == preview.platform_name
                    && branch.branch_name == general_name
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a preview branch previews a general branch of the same \
                     generation, so {general_name:?} must be published too, got {:?}",
                    catalog
                        .iter()
                        .filter(|branch| branch.platform_name == preview.platform_name)
                        .map(|branch| branch.branch_name.clone())
                        .collect::<Vec<String>>()
                )
            });
        // The preview and the branch it previewed were published together, so
        // they carry ONE ordinal — except where the general branch was later
        // issued an ordinal of its own, which the catalog does for the branches
        // [`PREVIEWS_A_STEP_BELOW`] names and nowhere else.
        let expected_order = match PREVIEWS_A_STEP_BELOW.contains(&preview.branch_name.as_str()) {
            true => general.branch_order - BRANCH_ORDER_STEP,
            false => general.branch_order,
        };
        assert_eq!(
            preview.branch_order, expected_order,
            "case {case:?}: expected the ordinal {expected_order} beside {general_name:?} ({}), \
             got {}",
            general.branch_order, preview.branch_order
        );
        assert_eq!(
            preview.supported_tiers, general.supported_tiers,
            "case {case:?}: expected the tiers of {general_name:?} ({:?}), got {:?}",
            general.supported_tiers, preview.supported_tiers
        );
        assert_eq!(
            preview.lifecycle_state, general.lifecycle_state,
            "case {case:?}: expected the life-cycle state of {general_name:?} ({:?}), got {:?}",
            general.lifecycle_state, preview.lifecycle_state
        );
    }
}

#[tokio::test]
async fn an_older_generations_branches_never_outrank_a_newer_generations() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    for branch in &catalog {
        let Some((generation, rank)) = generation_of(&branch.branch_name) else {
            continue;
        };
        let case = branch.key();
        for other in &catalog {
            if other.platform_name != branch.platform_name {
                continue;
            }
            let Some((other_generation, other_rank)) = generation_of(&other.branch_name) else {
                continue;
            };
            // A newer generation's branch is at least as high in the platform's
            // ordinal order as an older generation's.
            if other_rank > rank {
                assert!(
                    branch.branch_order >= other.branch_order,
                    "case {case:?}: the {generation:?} generation must not rank below the older \
                     {other_generation:?} one, got {} against {:?} at {}",
                    branch.branch_order,
                    other.branch_name,
                    other.branch_order
                );
            }
        }
    }
}

#[tokio::test]
async fn a_preconfigured_docker_branch_names_its_runtime_and_runs_both_tiers() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    let legacy: Vec<&Branch> = catalog
        .iter()
        .filter(|branch| branch.branch_name.starts_with(PRECONFIGURED_DOCKER_PREFIX))
        .collect();
    assert!(
        legacy.len() >= 2,
        "case \"preconfigured Docker\": expected the legacy preconfigured-Docker branches, got \
         {legacy:?}"
    );

    for branch in legacy {
        let case = branch.key();
        // Those platforms are all at end of life, and every one of them ran web
        // server and worker environments alike.
        assert_eq!(
            branch.lifecycle_state, "Retired",
            "case {case:?}: expected a retired legacy branch, got {:?}",
            branch.lifecycle_state
        );
        assert_eq!(
            branch.supported_tiers,
            expected_tiers(true),
            "case {case:?}: unexpected SupportedTierList, got {:?}",
            branch.supported_tiers
        );
        // The operating system is a generation the catalog publishes, named
        // without a distribution code name: `generation_of` panics on anything
        // else (it is how "64bit Debian jessie" would fail this case), and the
        // Debian generation of these platforms carries no release word.
        let (generation, _) = generation_of(&branch.branch_name)
            .unwrap_or_else(|| panic!("case {case:?}: expected a branch naming its platform"));
        assert!(
            generation
                .split_whitespace()
                .skip(1)
                .all(|word| word.chars().next().is_some_and(char::is_uppercase)
                    || word.chars().all(|character| character.is_ascii_digit())),
            "case {case:?}: the operating-system generation carries a distribution code name: \
             {generation:?}"
        );
    }
}

#[tokio::test]
async fn one_platform_publishes_each_branch_of_a_runtime_family() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // A branch name names ONE platform's generation, so no two platforms
    // publish the same branch: the Java SE branches (the Corretto ones and the
    // legacy `Java <n>` ones they replaced) are one platform's inventory, not
    // two overlapping ones.
    let mut platforms_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for branch in &catalog {
        platforms_of
            .entry(branch.branch_name.as_str())
            .or_default()
            .push(branch.platform_name.as_str());
    }
    for (branch_name, platforms) in &platforms_of {
        assert_eq!(
            platforms.len(),
            1,
            "case {branch_name:?}: expected one publishing platform, got {platforms:?}"
        );
    }

    // The Java SE platform publishes both spellings of its history, so a client
    // enumerating it sees the current runtime and the generations behind it.
    let java = of_platform(&emulator, "Java").await;
    let names: Vec<&str> = java
        .iter()
        .map(|branch| branch.branch_name.as_str())
        .collect();
    for expected in [
        "Corretto 25 running on 64bit Amazon Linux 2023",
        "Corretto 11 running on 64bit Amazon Linux 2",
        "Java 8 running on 64bit Amazon Linux",
    ] {
        assert!(
            names.contains(&expected),
            "case {expected:?}: expected the Java platform to publish this branch, got {names:?}"
        );
    }
    // Its current generation is the supported one, and it leads the list.
    let leading = java.first().expect("the Java platform publishes branches");
    assert_eq!(
        (leading.lifecycle_state.as_str(), leading.branch_order),
        ("Supported", 11400),
        "case \"Java\": expected the newest Corretto branch to lead the platform, got {leading:?}"
    );
}
