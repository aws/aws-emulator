// SPDX-License-Identifier: CC0-1.0

//! What the AWS-managed platform catalog REPORTS about the platforms it
//! publishes, driven end to end over the service's awsQuery HTTP surface.
//!
//! Three rules, each asserted over a SET of inputs rather than over one recorded
//! case:
//!
//! 1. The catalog publishes every platform Elastic Beanstalk has published under
//!    the `PlatformName` it groups that platform's branches by — including a
//!    platform whose whole branch history has retired, which is published under
//!    its own platform name — so a `PlatformName` filter reports that platform's
//!    branches instead of an empty list.
//! 2. `ListPlatformVersions` reports `PlatformLifecycleState` on EVERY summary:
//!    `PlatformSummary$PlatformLifecycleState` documents "Possible values:
//!    `recommended` | empty" and "If an empty value is returned, the platform
//!    version is supported but isn't the recommended one for its branch", so the
//!    member is present with an empty value on a version that is not its
//!    branch's recommended one and carries the state on the one that is.
//! 3. Every `CustomAmiList` entry of a published platform version names its
//!    virtualization type AND the machine image the version runs for that type:
//!    the entry for the virtualization type the version ships an image for
//!    carries an image id of the form EC2 issues one in, the same id on every
//!    read, and no two versions share an image.
//!
//! The expectations are computed from the service's own published catalog
//! (`eb::state::SOLUTION_STACKS`, what `ListAvailableSolutionStacks` reports) and
//! from the rules above; the one hand-written table names its source and covers
//! platforms this issue did not cite.
//!
//! The HTTP harness ([`crate::lifecycle::Emulator`]) and the branch reader
//! ([`crate::platform_branches`]) are the ones the other platform suites use, so
//! this suite adds no second copy of either.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{Answer, Emulator, TEST_REGION};
use crate::platform_branches::{branches, filter_params, full_catalog, list, Branch, TIERS};

/// `PlatformLifecycleState` of a version that is its branch's recommended one,
/// from `PlatformSummary$PlatformLifecycleState` ("Possible values:
/// `recommended` | empty"), in the capitalized spelling the service reports.
const RECOMMENDED: &str = "Recommended";

/// A second region, so an assertion about a REGIONAL value (a machine image is
/// an EC2 resource of one region) spans more than one.
const OTHER_REGION: &str = "eu-west-1";

/// The `CustomAmi$VirtualizationType` values a published platform version
/// reports an entry for, in the order `DescribePlatformVersion` lists them, and
/// which of them a current platform version ships an image for.
///
/// SOURCE: the two EC2 virtualization types. HVM is the one every current EC2
/// instance generation boots, so it is the one Elastic Beanstalk builds a
/// published platform version's image for; the paravirtual type precedes those
/// generations, so no current version ships an image for it. Spelled out here
/// rather than read from the service, so this suite states the rule itself.
const VIRTUALIZATION_TYPES: &[(&str, bool)] = &[("pv", false), ("hvm", true)];

/// The prefix and the digit count of an image id EC2 has issued, which is the
/// form an `ImageId` naming one takes ("THe ID of the image used to create the
/// custom AMI").
///
/// SOURCE: EC2's own published AMI id form — `ami-` followed by 17 lowercase
/// hexadecimal digits under a leading `0`, the shape of the documented
/// `ami-0123456789abcdef0` examples.
const IMAGE_ID_PREFIX: &str = "ami-";
/// See [`IMAGE_ID_PREFIX`].
const IMAGE_ID_DIGITS: usize = 17;

/// One row of the legacy-platform table below: a branch of Elastic Beanstalk's
/// published platform-branch inventory that belongs to a platform published
/// under its own `PlatformName`.
struct LegacyBranch {
    platform_name: &'static str,
    branch_name: &'static str,
    /// The REPORTED (capitalized) life-cycle state.
    lifecycle_state: &'static str,
    branch_order: i32,
    /// Whether the branch runs worker environments as well as web server ones.
    both_tiers: bool,
}

/// The legacy platforms the catalog publishes under a platform name of their
/// own, with the branches, life-cycle states and ordinals Elastic Beanstalk
/// publishes for them.
///
/// SOURCE: the "Elastic Beanstalk supported platforms" and "Retired platform
/// branches" documents, which is what the real `ListPlatformBranches`
/// enumerates. The rows span the legacy IIS-on-Windows-Server platform (web
/// server tier only, both branches published together so they share one
/// ordinal) AND the legacy Java SE platform this issue did not cite, so the rule
/// is asserted beyond the one platform the divergence named.
const LEGACY_BRANCHES: &[LegacyBranch] = &[
    LegacyBranch {
        platform_name: "Windows",
        branch_name: "Legacy - IIS 7.5 running on 64bit Windows Server",
        lifecycle_state: "Retired",
        branch_order: 9700,
        both_tiers: false,
    },
    LegacyBranch {
        platform_name: "Windows",
        branch_name: "Legacy - IIS 8 running on 64bit Windows Server",
        lifecycle_state: "Retired",
        branch_order: 9700,
        both_tiers: false,
    },
    LegacyBranch {
        platform_name: "Java",
        branch_name: "Java 8 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9800,
        both_tiers: true,
    },
    LegacyBranch {
        platform_name: "Java",
        branch_name: "Java 7 running on 64bit Amazon Linux",
        lifecycle_state: "Retired",
        branch_order: 9700,
        both_tiers: true,
    },
];

/// The branches of `platform`, as the server reports them for a
/// `PlatformName = <platform>` filter.
async fn of_platform(emulator: &Emulator, platform: &str) -> Vec<Branch> {
    let params = filter_params(0, "PlatformName", "=", &[platform.to_string()]);
    let answer = list(emulator, &params).await;
    branches(&answer, platform)
}

#[tokio::test]
async fn every_published_platform_reports_its_branches_under_its_platform_name() {
    let emulator = Emulator::new();
    let catalog = full_catalog(&emulator).await;

    // The platform names the catalog publishes, read off the catalog itself, so
    // the case set grows with it.
    let mut expected: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for branch in &catalog {
        expected
            .entry(branch.platform_name.clone())
            .or_default()
            .push(branch.branch_name.clone());
    }
    assert!(
        expected.len() > 1,
        "case \"catalog\": expected the catalog to publish several platforms, got {:?}",
        expected.keys().collect::<Vec<&String>>()
    );

    for (platform_name, branch_names) in &expected {
        let case = platform_name.as_str();
        let reported: Vec<String> = of_platform(&emulator, platform_name)
            .await
            .into_iter()
            .map(|branch| branch.branch_name)
            .collect();
        assert!(
            !reported.is_empty(),
            "case {case:?}: a platform the catalog publishes reports its branches, got an empty \
             list"
        );
        assert_eq!(
            &reported, branch_names,
            "case {case:?}: expected the branches the catalog publishes for this platform, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_legacy_platform_publishes_the_branches_aws_publishes_for_it() {
    let emulator = Emulator::new();

    for row in LEGACY_BRANCHES {
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
            "case {case:?}: unexpected LifecycleState, got {:?}",
            branch.lifecycle_state
        );
        assert_eq!(
            branch.branch_order, row.branch_order,
            "case {case:?}: unexpected BranchOrder, got {}",
            branch.branch_order
        );
        let expected_tiers: Vec<String> = match row.both_tiers {
            true => TIERS.iter().map(|tier| (*tier).to_string()).collect(),
            false => vec![TIERS[0].to_string()],
        };
        assert_eq!(
            branch.supported_tiers, expected_tiers,
            "case {case:?}: unexpected SupportedTierList, got {:?}",
            branch.supported_tiers
        );
    }
}

/// The `PlatformBranchName` and `PlatformVersion` of one `PlatformSummary`, with
/// the `PlatformLifecycleState` elements it carries — the EMPTY one included,
/// since "absent" and "present and empty" are different answers here.
fn summaries(answer: &Answer) -> Vec<(String, String, Vec<String>)> {
    answer
        .elements_at("PlatformSummaryList/member")
        .into_iter()
        .map(|member| {
            (
                member.value("PlatformBranchName").unwrap_or_default().to_string(),
                member.value("PlatformVersion").unwrap_or_default().to_string(),
                member
                    .values_at_including_empty("PlatformLifecycleState")
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_listed_version_reports_whether_it_is_its_branchs_recommended_one() {
    let emulator = Emulator::new();
    let answer = emulator.call("ListPlatformVersions", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case \"unfiltered\": expected 200 from ListPlatformVersions, got {} and body {}",
        answer.status, answer.body
    );

    // The recommended version of a branch is the newest release the catalog
    // publishes for it, computed from the catalog rather than from the response.
    let recommended: BTreeMap<&str, &str> = eb::state::SOLUTION_STACKS
        .iter()
        .map(|stack| (stack.branch_name, stack.platform_version))
        .collect();

    let listed = summaries(&answer);
    assert!(
        !listed.is_empty(),
        "case \"unfiltered\": expected the published catalog to report platform versions, got \
         body {}",
        answer.body
    );
    let mut states: BTreeSet<String> = BTreeSet::new();
    for (branch_name, platform_version, lifecycle_states) in &listed {
        let case = &format!("{branch_name} v{platform_version}");
        assert_eq!(
            lifecycle_states.len(),
            1,
            "case {case:?}: every summary reports exactly one PlatformLifecycleState element, \
             got {lifecycle_states:?}"
        );
        let state = lifecycle_states[0].as_str();
        let expected = match recommended.get(branch_name.as_str()) {
            Some(newest) if *newest == platform_version.as_str() => RECOMMENDED,
            // A release its branch's newest one superseded is supported but is
            // not the recommended version, so the member carries no value.
            _ => "",
        };
        assert_eq!(
            state, expected,
            "case {case:?}: expected PlatformLifecycleState {expected:?}, got {state:?}"
        );
        states.insert(state.to_string());
    }
    // Both answers are reachable in the published catalog, so neither branch of
    // the rule above is asserted only in theory.
    assert_eq!(
        states,
        [RECOMMENDED.to_string(), String::new()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case \"unfiltered\": expected both a recommended and a superseded version to be \
         listed, got the states {states:?}"
    );
}

/// One case of the machine-image table: a published platform version, named by
/// the branch it is a version within and the region it is addressed in.
struct ImageCase {
    branch_name: &'static str,
    /// Whether this is the branch's CURRENT release (`true`) or the release that
    /// one superseded (`false`), both of which the catalog publishes.
    current: bool,
    region: &'static str,
}

/// The platform versions the machine-image rule is asserted over: branches of
/// different platform families and operating systems, a current release and a
/// superseded one of the same branch, and one version addressed in a second
/// region — the dimensions the rule generalises over. The branch names are read
/// from the service's own catalog below, so no row transcribes a version number.
const IMAGE_CASES: &[ImageCase] = &[
    ImageCase {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        current: true,
        region: TEST_REGION,
    },
    ImageCase {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        current: false,
        region: TEST_REGION,
    },
    ImageCase {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        current: true,
        region: OTHER_REGION,
    },
    ImageCase {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2022",
        current: true,
        region: TEST_REGION,
    },
    ImageCase {
        branch_name: "Docker running on 64bit Amazon Linux 2023",
        current: true,
        region: TEST_REGION,
    },
    ImageCase {
        branch_name: "Tomcat 10 with Corretto 21 running on 64bit Amazon Linux 2023",
        current: true,
        region: TEST_REGION,
    },
];

/// Whether `value` is an image id of the form EC2 issues one in (see
/// [`IMAGE_ID_PREFIX`]): the prefix, then 17 lowercase hexadecimal digits under
/// a leading `0`, not all of them `0`.
fn is_issued_image_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix(IMAGE_ID_PREFIX) else {
        return false;
    };
    suffix.len() == IMAGE_ID_DIGITS
        && suffix.starts_with('0')
        && suffix
            .chars()
            .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase())
        && suffix.chars().any(|digit| digit != '0')
}

/// The `(VirtualizationType, ImageId)` pairs of one `DescribePlatformVersion`
/// answer, keeping an `ImageId` element that carries no value.
fn custom_amis(answer: &Answer) -> Vec<(String, Vec<String>)> {
    answer
        .elements_at("PlatformDescription/CustomAmiList/member")
        .into_iter()
        .map(|member| {
            (
                member.value("VirtualizationType").unwrap_or_default().to_string(),
                member
                    .values_at_including_empty("ImageId")
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn each_custom_ami_names_the_image_its_version_runs_for_that_virtualization_type() {
    let emulator = Emulator::new();
    // Every image id this suite read, so the ids of different versions can be
    // held apart: `case` -> the id of its HVM entry.
    let mut images: BTreeMap<String, String> = BTreeMap::new();

    for row in IMAGE_CASES {
        let stack = eb::state::SOLUTION_STACKS
            .iter()
            .find(|stack| stack.branch_name == row.branch_name)
            .unwrap_or_else(|| panic!("the catalog publishes the branch {:?}", row.branch_name));
        let published = stack.published_versions();
        let platform_version = match row.current {
            true => published
                .first()
                .expect("a published branch publishes its current release")
                .clone(),
            false => published
                .get(1)
                .expect("the catalog publishes the release the current one superseded")
                .clone(),
        };
        let case = format!("{} v{platform_version} in {}", row.branch_name, row.region);
        let arn = eb::state::platform_arn_of(row.region, row.branch_name, &platform_version);

        let answer = emulator
            .call_in_region(
                row.region,
                "DescribePlatformVersion",
                &[("PlatformArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from DescribePlatformVersion, got {} and body {}",
            answer.status, answer.body
        );

        let amis = custom_amis(&answer);
        assert_eq!(
            amis.iter()
                .map(|(virtualization_type, _)| virtualization_type.as_str())
                .collect::<Vec<&str>>(),
            VIRTUALIZATION_TYPES
                .iter()
                .map(|(virtualization_type, _)| *virtualization_type)
                .collect::<Vec<&str>>(),
            "case {case:?}: expected one entry per virtualization type, got body {}",
            answer.body
        );
        for ((virtualization_type, ships_an_image), (reported_type, image_ids)) in
            VIRTUALIZATION_TYPES.iter().zip(&amis)
        {
            let case = &format!("{case} ({reported_type})");
            assert_eq!(
                image_ids.len(),
                1,
                "case {case:?}: every entry reports exactly one ImageId element, got \
                 {image_ids:?}"
            );
            let image_id = image_ids[0].as_str();
            match ships_an_image {
                true => {
                    assert!(
                        is_issued_image_id(image_id),
                        "case {case:?}: the {virtualization_type:?} entry must name the image \
                         the version runs, as an id of the form EC2 issues, got {image_id:?}"
                    );
                    images.insert(case.clone(), image_id.to_string());
                }
                // A virtualization type the version ships no image for names
                // none, rather than an id this service would have invented.
                false => assert_eq!(
                    image_id, "",
                    "case {case:?}: the {virtualization_type:?} entry names no image, got \
                     {image_id:?}"
                ),
            }
        }

        // Reading the record again reports the same image: the image a version
        // runs is a fact of that version, not of the call.
        let again = emulator
            .call_in_region(
                row.region,
                "DescribePlatformVersion",
                &[("PlatformArn", arn.as_str())],
            )
            .await;
        assert_eq!(
            custom_amis(&again),
            amis,
            "case {case:?}: expected the same CustomAmiList on a second read, got body {}",
            again.body
        );
    }

    // No two of the versions above run one image: an image belongs to one
    // platform version in one region.
    let distinct: BTreeSet<&String> = images.values().collect();
    assert_eq!(
        distinct.len(),
        images.len(),
        "case \"distinct images\": expected one image per version and region, got {images:?}"
    );
}
