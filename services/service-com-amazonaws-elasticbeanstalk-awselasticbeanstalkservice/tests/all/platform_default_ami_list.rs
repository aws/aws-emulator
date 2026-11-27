// SPDX-License-Identifier: CC0-1.0

//! The default machine images a platform version reports, driven end to end over
//! the service's awsQuery HTTP surface.
//!
//! The rule under test: `PlatformDescription$DefaultAmiList` is "the AMIs that
//! Elastic Beanstalk uses by default for the platform version, one for each
//! supported processor architecture", and "this list isn't returned for custom
//! platforms" — so a version the SERVICE publishes reports one entry per
//! processor architecture an AMI can have, each naming that architecture and an
//! image id of the form EC2 issues one in, the same ids on every read and no two
//! versions sharing an image; a version an ACCOUNT built from its own platform
//! definition reports no `DefaultAmiList` element at all.
//!
//! The rows span what the rule generalises over — platforms of different
//! families and operating systems, a branch's current release and the release it
//! superseded, and one version addressed in a second region, since a machine
//! image is a regional EC2 resource — with the version numbers read from the
//! service's own catalog (`eb::state::SOLUTION_STACKS`) rather than transcribed.
//!
//! The HTTP harness ([`crate::lifecycle::Emulator`]) and the image-id form
//! ([`crate::published_platform_catalog::is_issued_image_id`]) are the ones the
//! other platform suites use, so this suite adds no second copy of either.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{Answer, Emulator, TEST_REGION};
use crate::published_platform_catalog::is_issued_image_id;

/// A second region, so an assertion about a REGIONAL value (a machine image is
/// an EC2 resource of one region) spans more than one.
const OTHER_REGION: &str = "eu-west-1";

/// The processor architectures a platform version's default AMIs cover, SORTED —
/// this suite asserts the SET of architectures rather than an order, so the
/// order the service lists them in is not read as part of the rule.
///
/// SOURCE: `DefaultAmi$Architecture` targets the model's `AmiArchitecture` enum,
/// whose values are `x86_64` and `arm64` ("The processor architecture of the
/// AMI"). Spelled out here rather than read from the service, so this suite
/// states the rule itself.
const ARCHITECTURES: &[&str] = &["arm64", "x86_64"];

/// One case of the table: a published platform version, named by the branch it
/// is a version within and the region it is addressed in.
struct Case {
    branch_name: &'static str,
    /// Whether this is the branch's CURRENT release (`true`) or the release that
    /// one superseded (`false`), both of which the catalog publishes.
    current: bool,
    region: &'static str,
}

/// The platform versions the rule is asserted over. The branch names come from
/// the service's own catalog; the version numbers are read from it below.
const CASES: &[Case] = &[
    Case {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        current: true,
        region: TEST_REGION,
    },
    Case {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        current: false,
        region: TEST_REGION,
    },
    Case {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        current: true,
        region: OTHER_REGION,
    },
    Case {
        branch_name: "Docker running on 64bit Amazon Linux 2023",
        current: true,
        region: TEST_REGION,
    },
    Case {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2022",
        current: true,
        region: TEST_REGION,
    },
    Case {
        branch_name: "Corretto 21 running on 64bit Amazon Linux 2023",
        current: true,
        region: TEST_REGION,
    },
];

/// The `(Architecture, ImageId)` pairs of one `DescribePlatformVersion` answer,
/// keeping an element that carries no value so an entry naming no image is told
/// apart from one carrying an id.
fn default_amis(answer: &Answer) -> Vec<(String, Vec<String>)> {
    answer
        .elements_at("PlatformDescription/DefaultAmiList/member")
        .into_iter()
        .map(|member| {
            (
                member.value("Architecture").unwrap_or_default().to_string(),
                member
                    .values_at_including_empty("ImageId")
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )
        })
        .collect()
}

/// `DescribePlatformVersion` for `arn`, signed for `region`.
async fn described(emulator: &Emulator, region: &str, arn: &str, case: &str) -> Answer {
    let answer = emulator
        .call_in_region(region, "DescribePlatformVersion", &[("PlatformArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from DescribePlatformVersion, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// The version of `branch_name` this row is about: the branch's current release,
/// or the release that one superseded, read from the service's own catalog.
fn version_of(branch_name: &str, current: bool) -> String {
    let stack = eb::state::SOLUTION_STACKS
        .iter()
        .find(|stack| stack.branch_name == branch_name)
        .unwrap_or_else(|| panic!("the catalog publishes the branch {branch_name:?}"));
    let published = stack.published_versions();
    match current {
        true => published
            .first()
            .expect("a published branch publishes its current release")
            .clone(),
        false => published
            .get(1)
            .expect("the catalog publishes the release the current one superseded")
            .clone(),
    }
}

#[tokio::test]
async fn a_published_version_names_one_default_image_per_processor_architecture() {
    let emulator = Emulator::new();
    // Every image id this suite read: `(case, architecture)` -> the id, so the
    // ids of different versions, regions and architectures can be held apart.
    let mut images: BTreeMap<String, String> = BTreeMap::new();

    for row in CASES {
        let platform_version = version_of(row.branch_name, row.current);
        let case = format!("{} v{platform_version} in {}", row.branch_name, row.region);
        let arn = eb::state::platform_arn_of(row.region, row.branch_name, &platform_version);

        let answer = described(&emulator, row.region, &arn, &case).await;
        let amis = default_amis(&answer);

        // One entry per architecture, each naming its own: the member is "one
        // for each supported processor architecture", so neither a missing
        // architecture nor a repeated one satisfies it.
        let reported: Vec<String> = amis
            .iter()
            .map(|(architecture, _)| architecture.clone())
            .collect();
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            ARCHITECTURES
                .iter()
                .map(|architecture| (*architecture).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected one DefaultAmiList entry per processor architecture, got \
             {reported:?} in body {}",
            answer.body
        );

        for (architecture, image_ids) in &amis {
            let case = &format!("{case} ({architecture})");
            assert_eq!(
                image_ids.len(),
                1,
                "case {case:?}: every entry reports exactly one ImageId element, got {image_ids:?}"
            );
            let image_id = image_ids[0].as_str();
            // Elastic Beanstalk publishes a managed version for every
            // architecture, so every entry names the image it uses by default
            // there, in the form EC2 issues an image id in.
            assert!(
                is_issued_image_id(image_id),
                "case {case:?}: expected the default image for this architecture, as an id of \
                 the form EC2 issues, got {image_id:?}"
            );
            images.insert(case.clone(), image_id.to_string());
        }

        // Reading the record again reports the same images: the image a version
        // uses by default is a fact of that version, not of the call.
        let again = described(&emulator, row.region, &arn, &case).await;
        assert_eq!(
            default_amis(&again),
            amis,
            "case {case:?}: expected the same DefaultAmiList on a second read, got body {}",
            again.body
        );
    }

    // No two of the entries above name one image: a default AMI belongs to one
    // platform version, in one region, for one architecture.
    let distinct: BTreeSet<&String> = images.values().collect();
    assert_eq!(
        distinct.len(),
        images.len(),
        "case \"distinct images\": expected one image per version, region and architecture, got \
         {images:?}"
    );
}

#[tokio::test]
async fn a_custom_platform_version_reports_no_default_ami_list() {
    let emulator = Emulator::new();
    let case = "a version built from an account's own platform definition";

    let created = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", "default-ami-platform"),
                ("PlatformVersion", "1.0.0"),
                ("PlatformDefinitionBundle.S3Bucket", "platform-bundles"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    let arn = created
        .expect("PlatformSummary/PlatformArn", case)
        .to_string();

    let answer = described(&emulator, TEST_REGION, &arn, case).await;
    // The read answers for the version that was created — so the absence below
    // is an answered description without the member, not an empty answer.
    assert_eq!(
        answer.value("PlatformDescription/PlatformArn"),
        Some(arn.as_str()),
        "case {case:?}: the read must describe the created version, got body {}",
        answer.body
    );
    assert!(
        answer
            .values_at_including_empty("PlatformDescription/DefaultAmiList")
            .is_empty(),
        "case {case:?}: a custom platform version reports no DefaultAmiList, got body {}",
        answer.body
    );

    // And the list IS reported for a version the service publishes, so the
    // absence above is this version's and not the member's.
    let branch_name = CASES[0].branch_name;
    let platform_version = version_of(branch_name, true);
    let published = described(
        &emulator,
        TEST_REGION,
        &eb::state::platform_arn_of(TEST_REGION, branch_name, &platform_version),
        "a version the service publishes",
    )
    .await;
    assert_eq!(
        default_amis(&published).len(),
        ARCHITECTURES.len(),
        "case \"a version the service publishes\": expected {} DefaultAmiList entries, got body \
         {}",
        ARCHITECTURES.len(),
        published.body
    );
}
