// SPDX-License-Identifier: CC0-1.0

//! The per-platform branch inventory `ListPlatformBranches` publishes for the
//! runtime platforms whose history runs across several generations.
//!
//! The rule under test: for each `PlatformName` the operation reports every
//! branch that platform has published — its current branches AND each
//! deprecated and retired generation behind them — under the branch name the
//! service publishes, which carries the variant that identifies a branch within
//! its generation:
//!
//! * the application server an Amazon Linux Ruby branch is served behind
//!   (`Puma with Ruby 2.5 …`, `Passenger Standalone with Ruby 2.5 …`), so the
//!   two variants of one runtime are separate branches;
//! * the servlet container AND the JDK a Tomcat branch pairs, the JDK named as
//!   the generation names it (the Corretto distribution on Amazon Linux 2 and
//!   Amazon Linux 2023, the Java version itself on Amazon Linux);
//! * the ARCHITECTURE of an Amazon Linux branch, a generation having published
//!   its 32-bit branches beside its 64-bit ones;
//! * the `(BETA) ` prefix of a branch published as a preview of a general
//!   branch of the same generation;
//!
//! and each branch carries the per-platform `BranchOrder` the platform issued
//! it ("a larger `BranchOrder` value designates a newer platform branch within
//! the platform"), so the list a `PlatformName` filter reports leads with the
//! branch a client deploys onto.
//!
//! SOURCE of the rows below: Elastic Beanstalk's own published platform
//! inventory — the "Elastic Beanstalk supported platforms" and "Retired
//! platform branches" documents, which is what the real `ListPlatformBranches`
//! enumerates.
//!
//! Every assertion spans a SET of inputs: the table covers each platform whose
//! inventory runs over more than one generation and MOST of its rows are
//! branches no divergence report cited, the newest-branch rows cover every such
//! platform plus two the reports never mentioned, and the variant, JDK and
//! architecture rules are asserted over the WHOLE inventory of each platform
//! that publishes them.
//!
//! The HTTP harness and the branch reader are [`crate::lifecycle`]'s and
//! [`crate::platform_branches`]', so this suite adds no second copy of either.

use std::collections::BTreeSet;

use crate::lifecycle::Emulator;
use crate::platform_branches::{of_platform, Branch, CatalogRow};

/// Branches of the published inventory, spanning the generations behind each
/// platform's current branches. Rows marked "cited" are the few a divergence
/// report named; the rest are entries of the same generations that no report
/// cited, so the rule is asserted beyond the cited cases.
const INVENTORY_ROWS: &[CatalogRow] = &[
    // Java SE: the Amazon Linux 2 generation, whose Corretto 8 branch was
    // published beside the preview branch that previewed it.
    CatalogRow {
        platform_name: "Java",
        branch_name: "Corretto 17 running on 64bit Amazon Linux 2",
        lifecycle_state: "Retired",
        branch_order: 10100,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Java",
        branch_name: "(BETA) Corretto 8 running on 64bit Amazon Linux 2",
        lifecycle_state: "Retired",
        branch_order: 9900,
        both_tiers: true,
    },
    // PHP: the Amazon Linux generation on both architectures it published.
    CatalogRow {
        platform_name: "PHP",
        branch_name: "PHP 5.5 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9500,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "PHP",
        branch_name: "PHP 5.3 running on 32bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9000,
        both_tiers: true,
    },
    // Python: a runtime the Amazon Linux generation published, and the 32-bit
    // branch of the same runtime.
    CatalogRow {
        platform_name: "Python",
        branch_name: "Python 2.6 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9700,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Python",
        branch_name: "Python 2.6 running on 32bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9400,
        both_tiers: true,
    },
    // Ruby: the two application-server variants of the Amazon Linux
    // generation, and a runtime from before that split.
    CatalogRow {
        platform_name: "Ruby",
        branch_name: "Puma with Ruby 2.2 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9600,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Ruby",
        branch_name: "Passenger Standalone with Ruby 2.1 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 8800,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Ruby",
        branch_name: "Ruby 1.9.3 running on 32bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 8400,
        both_tiers: true,
    },
    // Tomcat: a current servlet-container/JDK pairing, the preview branch of
    // the Amazon Linux 2 generation, and a 32-bit Amazon Linux pairing.
    CatalogRow {
        platform_name: "Tomcat",
        branch_name: "Tomcat 9 with Corretto 17 running on 64bit Amazon Linux 2023",
        lifecycle_state: "Supported",
        branch_order: 12200,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Tomcat",
        branch_name: "(BETA) Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2",
        lifecycle_state: "Retired",
        branch_order: 11100,
        both_tiers: true,
    },
    CatalogRow {
        platform_name: "Tomcat",
        branch_name: "Tomcat 6 with Java 6 running on 32bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9500,
        both_tiers: true,
    },
];

/// The branch a client that enumerates a platform to deploy onto picks: the
/// newest branch of that platform, which is the one the `PlatformName` filter
/// leads with. The last two rows are platforms no divergence report cited.
const NEWEST_BRANCHES: &[(&str, &str)] = &[
    ("Java", "Corretto 25 running on 64bit Amazon Linux 2023"),
    ("PHP", "PHP 8.5 running on 64bit Amazon Linux 2023"),
    ("Python", "Python 3.14 running on 64bit Amazon Linux 2023"),
    ("Ruby", "Ruby 4.0 running on 64bit Amazon Linux 2023"),
    (
        "Tomcat",
        "Tomcat 11 with Corretto 25 running on 64bit Amazon Linux 2023",
    ),
    ("Node.js", "Node.js 24 running on 64bit Amazon Linux 2023"),
    ("Go", "Go 1 running on 64bit Amazon Linux 2023"),
];

/// The application servers an Amazon Linux Ruby branch names, as the catalog
/// spells them.
const RUBY_APPLICATION_SERVERS: &[&str] = &["Puma with ", "Passenger Standalone with "];

/// The operating-system generation an Amazon Linux branch name ends with, and
/// the JDK spelling a Tomcat branch of that generation uses: the Corretto
/// distribution on the Amazon Linux 2 and Amazon Linux 2023 generations, the
/// Java version itself on the Amazon Linux one.
const JDK_SPELLING: &[(&str, &str)] = &[
    ("64bit Amazon Linux 2023", "Corretto"),
    ("64bit Amazon Linux 2", "Corretto"),
    ("64bit Amazon Linux", "Java"),
    ("32bit Amazon Linux", "Java"),
];

/// The runtime version `branch_name` states after `prefix`, e.g. `2.5` in
/// `Puma with Ruby 2.5 running on 64bit Amazon Linux`.
fn runtime_after(branch_name: &str, prefix: &str) -> Option<String> {
    let rest = branch_name.strip_prefix(prefix)?;
    let (runtime, _) = rest.split_once(" running on ")?;
    Some(runtime.to_string())
}

/// The operating-system generation `branch_name` names.
fn operating_system(branch_name: &str) -> Option<&str> {
    branch_name.split_once(" running on ").map(|(_, os)| os)
}

#[tokio::test]
async fn each_platform_reports_the_generations_behind_its_current_branches() {
    let emulator = Emulator::new();

    for row in INVENTORY_ROWS {
        let case = row.branch_name;
        let reported = of_platform(&emulator, row.platform_name).await;
        let branch = reported
            .iter()
            .find(|branch| branch.branch_name == row.branch_name)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the platform {:?} to publish this branch, got {:?}",
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
async fn the_newest_branch_of_a_platform_leads_the_list_a_client_reads() {
    let emulator = Emulator::new();

    for (platform, newest) in NEWEST_BRANCHES {
        let case = *platform;
        let reported = of_platform(&emulator, platform).await;
        let leading = reported
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected this platform's branches, got none"));
        assert_eq!(
            leading.branch_name, *newest,
            "case {case:?}: expected the newest branch {newest:?} to lead the platform, got {:?}",
            leading.branch_name
        );
        // The branch a client deploys onto is one the service still supports,
        // and no branch of the platform outranks it.
        assert_eq!(
            leading.lifecycle_state, "Supported",
            "case {case:?}: expected the newest branch to be Supported, got {:?}",
            leading.lifecycle_state
        );
        for other in &reported {
            assert!(
                leading.branch_order >= other.branch_order,
                "case {case:?}: expected no branch to outrank {newest:?} ({}), got {:?} at {}",
                leading.branch_order,
                other.branch_name,
                other.branch_order
            );
        }
    }
}

#[tokio::test]
async fn an_amazon_linux_ruby_branch_names_the_application_server_it_runs_behind() {
    let emulator = Emulator::new();
    let reported = of_platform(&emulator, "Ruby").await;

    // The Ruby 2.x branches of the Amazon Linux generation: every one of them
    // is a case here, so the rule spans the whole variant history.
    let amazon_linux: Vec<&Branch> = reported
        .iter()
        .filter(|branch| {
            operating_system(&branch.branch_name) == Some("64bit Amazon Linux")
                && branch.branch_name.contains("Ruby 2.")
        })
        .collect();
    assert!(
        amazon_linux.len() >= 4,
        "case \"Ruby 2.x on Amazon Linux\": expected the application-server variants of that \
         generation, got {amazon_linux:?}"
    );

    // Each runtime version of that generation is published once per application
    // server, so the two variants cover the same runtimes: a client resolving a
    // Puma branch is never handed the Passenger one instead.
    let mut runtimes: Vec<BTreeSet<String>> = Vec::new();
    for server in RUBY_APPLICATION_SERVERS {
        let selected: BTreeSet<String> = amazon_linux
            .iter()
            .filter_map(|branch| runtime_after(&branch.branch_name, server))
            .collect();
        assert!(
            !selected.is_empty(),
            "case {server:?}: expected the branches this application server was published for, \
             got {:?}",
            amazon_linux
                .iter()
                .map(|branch| branch.branch_name.clone())
                .collect::<Vec<String>>()
        );
        runtimes.push(selected);
    }
    assert_eq!(
        runtimes[0], runtimes[1],
        "case \"application-server pairing\": expected {:?} to publish the same runtimes as {:?}, \
         got {:?} against {:?}",
        RUBY_APPLICATION_SERVERS[0], RUBY_APPLICATION_SERVERS[1], runtimes[0], runtimes[1]
    );

    // No branch of that generation names a runtime without naming the
    // application server serving it.
    for branch in &amazon_linux {
        let case = branch.branch_name.as_str();
        assert!(
            RUBY_APPLICATION_SERVERS
                .iter()
                .any(|server| case.starts_with(server)),
            "case {case:?}: expected a branch name leading with one of \
             {RUBY_APPLICATION_SERVERS:?}"
        );
    }
}

#[tokio::test]
async fn a_tomcat_branch_pairs_a_servlet_container_with_the_jdk_of_its_generation() {
    let emulator = Emulator::new();
    let reported = of_platform(&emulator, "Tomcat").await;
    assert!(
        reported.len() >= 8,
        "case \"Tomcat\": expected the platform's whole container history, got {reported:?}"
    );

    // Every published Tomcat branch is a case, so the rule spans each
    // generation the platform has published.
    for branch in &reported {
        let case = branch.branch_name.as_str();
        let name = case.strip_prefix("(BETA) ").unwrap_or(case);
        let container = name
            .strip_prefix("Tomcat ")
            .and_then(|rest| rest.split_once(" with "))
            .map(|(container, _)| container)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a branch naming its servlet container")
            });
        assert!(
            container
                .split('.')
                .all(|component| !component.is_empty()
                    && component.chars().all(|digit| digit.is_ascii_digit())),
            "case {case:?}: expected a numeric servlet-container version, got {container:?}"
        );
        let generation = operating_system(case)
            .unwrap_or_else(|| panic!("case {case:?}: expected a branch naming its generation"));
        let (_, jdk) = JDK_SPELLING
            .iter()
            .find(|(published, _)| *published == generation)
            .unwrap_or_else(|| {
                panic!("case {case:?}: unexpected operating-system generation {generation:?}")
            });
        let runtime = name
            .split_once(" with ")
            .and_then(|(_, rest)| rest.split_once(' '))
            .map(|(distribution, _)| distribution)
            .unwrap_or_default();
        assert_eq!(
            runtime, *jdk,
            "case {case:?}: the {generation:?} generation names its JDK {jdk:?}, got {runtime:?}"
        );
    }
}

#[tokio::test]
async fn a_generation_that_published_both_architectures_reports_the_32_bit_branches_below_them() {
    let emulator = Emulator::new();

    // One case per platform that published 32-bit Amazon Linux branches, read
    // off the catalog itself rather than listed here, so the rule holds as the
    // inventory grows.
    let mut covered = 0usize;
    for (platform, _) in NEWEST_BRANCHES {
        let case = *platform;
        let reported = of_platform(&emulator, platform).await;
        let thirty_two: Vec<&Branch> = reported
            .iter()
            .filter(|branch| operating_system(&branch.branch_name) == Some("32bit Amazon Linux"))
            .collect();
        if thirty_two.is_empty() {
            continue;
        }
        covered += 1;
        // The architecture is not a generation of its own: a platform that
        // published 32-bit branches published the 64-bit branches of the same
        // operating system too, and none of its 32-bit branches outranks them.
        let newest_64_bit = reported
            .iter()
            .filter(|branch| operating_system(&branch.branch_name) == Some("64bit Amazon Linux"))
            .map(|branch| branch.branch_order)
            .max()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a platform publishing 32-bit Amazon Linux branches publishes \
                     the 64-bit ones too, got {:?}",
                    reported
                        .iter()
                        .map(|branch| branch.branch_name.clone())
                        .collect::<Vec<String>>()
                )
            });
        for branch in thirty_two {
            assert!(
                branch.branch_order <= newest_64_bit,
                "case {case:?}: expected the 32-bit branch {:?} ({}) not to outrank the newest \
                 64-bit Amazon Linux branch ({newest_64_bit}), got {}",
                branch.branch_name,
                branch.branch_order,
                branch.branch_order
            );
        }
    }
    assert!(
        covered >= 3,
        "case \"architectures\": expected several platforms publishing 32-bit Amazon Linux \
         branches, got {covered}"
    );
}
