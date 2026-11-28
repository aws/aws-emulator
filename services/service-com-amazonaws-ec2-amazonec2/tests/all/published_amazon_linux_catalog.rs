// SPDX-License-Identifier: CC0-1.0

//! Resolving a launch AMI out of the published catalog the documented way, over the wire the
//! service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with
//! XML; README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: EVERY VARIANT AMAZON PUBLISHES UNDER THE `amazon` OWNER ALIAS HAS A
//! CURRENT BUILD IN THE CATALOG — one that is public, `available` and not deprecated — so a
//! `DescribeImages` narrowed to that owner, that variant's published name pattern and
//! `state=available` reports at least one image, each carrying the `ImageId` and the
//! `CreationDate` a caller sorts on, and the newest match launches. A variant whose only
//! builds are deprecated is a catalog that cannot be launched from: the operation models no
//! errors, so an empty set is the only way such a request can fail, and it fails silently.
//!
//! The owner is asked for both documented ways: named in `Owners`, which takes "a combination
//! of Amazon Web Services account IDs, self, amazon, aws-backup-vault, and aws-marketplace"
//! (`DescribeImagesRequest$Owners`), and selected by the `owner-alias` filter
//! (`DescribeImagesRequest$Filters`). Neither way may see a catalog the other does not.
//!
//! The rows span what that generalises over — every family the catalog publishes rather than
//! the Amazon Linux 2 one a launch tutorial names: Amazon Linux (HVM, its minimal variant and
//! the paravirtual instance-store form), Amazon Linux 2 in both architectures and both
//! root-device forms, kernel-pinned and minimal, both Amazon Linux 2023 variants, the Amazon
//! Linux 2027 preview, the ECS-optimized and Bottlerocket container hosts, the AWS PCS sample
//! images and the Windows Server base image — each by the name pattern a client writes rather
//! than by an AMI id, which is per-Region and per-release. The second test takes its rows from
//! the catalog itself, so a variant added later is covered without touching the table.
//!
//! The table, the two owner forms and the reading of one reported image live here and are
//! shared with [`super::amazon_published_image_discovery`], which reads the same catalog
//! WITHOUT a name pattern: one copy of each, so the two suites cannot disagree about which
//! variants Amazon publishes or how a response is read.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, elements, filters, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for; the image catalog is per account.
const REGION: &str = "us-east-1";

/// The owner alias Amazon's own published images carry (`Image$ImageOwnerAlias`), which is
/// also what a caller asks for in `Owners` and in the `owner-alias` filter.
pub(crate) const AMAZON: &str = "amazon";

/// The image state a client resolving a launch AMI asks for (the `state` filter).
pub(crate) const AVAILABLE: &str = "available";

/// One published variant to resolve: the name pattern a client matches on, and the
/// architecture that pattern names.
///
/// The patterns are the published name forms of the variants Amazon publishes a parameter for
/// under `/aws/service/ami-amazon-linux-latest/`, `/aws/service/ami-windows-latest/`,
/// `/aws/service/ecs/optimized-ami/`, `/aws/service/bottlerocket/` and `/aws/service/pcs/`
/// (Amazon EC2 User Guide, "Use a Systems Manager parameter instead of an AMI ID", and the
/// Amazon Linux 2023 User Guide's "Find an AMI" name forms
/// `amzn-ami[-minimal]-{hvm,pv}-<version>-x86_64-{ebs,gp2,s3}`,
/// `amzn2-ami[-minimal][-kernel-<kernel version>]-hvm-<version>-<architecture>-<root volume>`
/// and `al2023-ami[-minimal]-<version>-kernel-<kernel version>-<architecture>`) — not the
/// ids, names or dates of any one release, which are per-Region, per-release values.
///
/// Every row's newest match is launched: an image a client resolves this way is one it goes on
/// to run, whatever root device, virtualization or platform its family is published with.
pub(crate) const PUBLISHED_VARIANTS: &[(&str, &str)] = &[
    // Amazon Linux: the HVM EBS build, the minimal variant, and the paravirtual
    // instance-store-backed build, which is the family's oldest published form.
    ("amzn-ami-hvm-*-x86_64-ebs", "x86_64"),
    ("amzn-ami-minimal-hvm-*-x86_64-ebs", "x86_64"),
    ("amzn-ami-pv-*-x86_64-s3", "x86_64"),
    // Amazon Linux 2, the family a launch script resolves: the unqualified name in both
    // architectures, the kernel-pinned builds, and the minimal variants.
    ("amzn2-ami-hvm-*-x86_64-gp2", "x86_64"),
    ("amzn2-ami-hvm-*-arm64-gp2", "arm64"),
    ("amzn2-ami-kernel-5.10-hvm-*-x86_64-gp2", "x86_64"),
    ("amzn2-ami-kernel-4.14-hvm-*-x86_64-ebs", "x86_64"),
    ("amzn2-ami-minimal-hvm-*-x86_64-ebs", "x86_64"),
    ("amzn2-ami-minimal-kernel-5.10-hvm-*-arm64-gp2", "arm64"),
    // Amazon Linux 2023 and the Amazon Linux 2027 preview, whose names pin a kernel.
    ("al2023-ami-2023.*-kernel-6.1-x86_64", "x86_64"),
    ("al2023-ami-minimal-*-kernel-6.1-arm64", "arm64"),
    ("al2023-ami-*-kernel-6.18-arm64", "arm64"),
    ("al2027-preview-ami-*-kernel-7.2-x86_64", "x86_64"),
    // The container-host images: an accelerator build of the ECS-optimized Amazon Linux 2 and
    // Amazon Linux 2023 families, and a Bottlerocket build, whose name spells 64-bit Arm the
    // way that project does.
    ("amzn2-ami-ecs-gpu-hvm-*-x86_64-ebs", "x86_64"),
    ("al2023-ami-ecs-neuron-hvm-*-kernel-*-x86_64", "x86_64"),
    ("bottlerocket-aws-ecs-2-aarch64-*", "arm64"),
    // The AWS PCS sample image of a compute node group, and the Windows Server base image.
    ("aws-pcs-sample_ami-al2023-arm64-slurm-*", "arm64"),
    ("Windows_Server-2022-*", "x86_64"),
];

/// The documented ways one request can name the `amazon` owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerForm {
    /// The alias named in `Owners`, the member documented to take it.
    OwnersMember,
    /// The alias selected by the `owner-alias` filter instead.
    OwnerAliasFilter,
}

impl OwnerForm {
    /// How a case names this form.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OwnersMember => "Owners=amazon",
            Self::OwnerAliasFilter => "Filters owner-alias=amazon",
        }
    }
}

/// Both of them, as the rows every table below is read through.
pub(crate) const OWNER_FORMS: [OwnerForm; 2] =
    [OwnerForm::OwnersMember, OwnerForm::OwnerAliasFilter];

/// One image row of a response: the members these suites read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) image_id: String,
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) architecture: String,
    pub(crate) owner_alias: String,
    pub(crate) owner_id: String,
    pub(crate) public: String,
    pub(crate) creation_date: String,
    pub(crate) deprecation_time: Option<String>,
}

/// Every image a `DescribeImages` body reports.
fn images(case: &str, body: &str) -> Vec<Image> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "imageId").is_some())
        .map(|node| Image {
            image_id: child_text(node, "imageId").unwrap_or_default(),
            name: child_text(node, "name").unwrap_or_default(),
            state: child_text(node, "imageState").unwrap_or_default(),
            architecture: child_text(node, "architecture").unwrap_or_default(),
            owner_alias: child_text(node, "imageOwnerAlias").unwrap_or_default(),
            owner_id: child_text(node, "imageOwnerId").unwrap_or_default(),
            public: child_text(node, "isPublic").unwrap_or_default(),
            creation_date: child_text(node, "creationDate").unwrap_or_default(),
            deprecation_time: child_text(node, "deprecationTime"),
        })
        .collect()
}

/// The images `DescribeImages` reports for `params`, which must succeed.
pub(crate) async fn described(
    case: &str,
    router: &MultiServiceRouter,
    params: &[Param],
) -> Vec<Image> {
    let (status, body) = call(router, REGION, "DescribeImages", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with body {body}"
    );
    images(case, &body)
}

/// The request a client resolving an AMI by name sends: the `amazon` owner, the `available`
/// state, and a name pattern when the case names one.
pub(crate) fn resolve_by_name(form: OwnerForm, pattern: Option<&str>) -> Vec<Param> {
    let name_values = [pattern.unwrap_or_default()];
    let mut entries: Vec<(&str, &[&str])> = Vec::new();
    if form == OwnerForm::OwnerAliasFilter {
        entries.push(("owner-alias", &[AMAZON]));
    }
    if pattern.is_some() {
        entries.push(("name", &name_values));
    }
    entries.push(("state", &[AVAILABLE]));
    let mut params = filters(&entries);
    if form == OwnerForm::OwnersMember {
        params.extend(indexed("Owner", &[AMAZON]));
    }
    params
}

/// The newest of `reported` by `CreationDate`, which is the documented way to pick one.
///
/// Shared with [`super::published_distribution_catalog`], which resolves the catalog's
/// third-party distribution families the same way: one reading of "pick the newest match".
pub(crate) fn newest(case: &str, reported: &[Image]) -> Image {
    reported
        .iter()
        .max_by(|left, right| left.creation_date.cmp(&right.creation_date))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a non-empty report has a newest image"))
}

/// Launch `image_id` and report the `imageId` the instance is described with — the proof
/// that the id the catalog resolved to is an AMI this account can actually run.
///
/// Shared with [`super::published_distribution_catalog`], so every suite that resolves an
/// image out of the catalog proves it launches the one way.
pub(crate) async fn launch_and_read_back(
    case: &str,
    router: &MultiServiceRouter,
    image_id: &str,
) -> String {
    let mut params = vec![
        param("ImageId", image_id),
        param("MinCount", "1"),
        param("MaxCount", "1"),
    ];
    // An instance runs the machine code its AMI holds, so the launch names a type whose
    // architecture is this AMI's own — read off the AMI rather than written here.
    params.extend(super::ec2_query::launch_type_for_image(router, REGION, case, image_id).await);
    let (status, body) = call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances with the resolved AMI must succeed, got {status} with \
         body {body}"
    );
    let instance_id = elements(&body, "instanceId")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must mint an instance: {body}"));

    let (described_status, described_body) = call(
        router,
        REGION,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id.as_str()]),
    )
    .await;
    assert_eq!(
        described_status, 200,
        "case {case:?}: DescribeInstances must succeed, got {described_status} with body \
         {described_body}"
    );
    elements(&described_body, "imageId")
        .first()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the instance must report the AMI it ran: {described_body}")
        })
}

/// The name pattern a client writes for the variant `name` is a build of: the published
/// name with every version token — the ones carrying a `.`, which is where a release
/// number lives — replaced by EC2's `*` wildcard, so one pattern matches every build of
/// that variant whatever release it comes from.
fn variant_pattern(name: &str) -> String {
    name.split('-')
        .map(|token| {
            if token.contains('.') {
                "*"
            } else {
                token
            }
        })
        .collect::<Vec<&str>>()
        .join("-")
}

/// Every published variant resolves to at least one Amazon-owned, available, undeprecated
/// image with an id and a creation date — whichever documented way the request names that
/// owner — and the newest match launches.
#[tokio::test]
async fn every_published_variant_resolves_to_a_launchable_available_image() {
    let (router, _engine) = super::ec2_query::emulator();

    for (pattern, architecture) in PUBLISHED_VARIANTS {
        for form in OWNER_FORMS {
            let case = format!("{pattern} via {}", form.label());
            let reported = described(&case, &router, &resolve_by_name(form, Some(pattern))).await;
            assert!(
                !reported.is_empty(),
                "case {case:?}: expected at least one Amazon-published available image, got an \
                 empty set"
            );

            for image in &reported {
                assert!(
                    image.image_id.starts_with("ami-"),
                    "case {case:?}: every reported image carries an AMI id, got {image:?}"
                );
                assert_eq!(
                    image.state, AVAILABLE,
                    "case {case:?}: the state filter must hold, got {image:?}"
                );
                assert_eq!(
                    image.owner_alias, AMAZON,
                    "case {case:?}: the owner filter must hold, got {image:?}"
                );
                assert_eq!(
                    image.owner_id,
                    ec2::state::AMAZON_IMAGE_OWNER_ID,
                    "case {case:?}: that alias is Amazon's own publishing account, got \
                     {image:?}"
                );
                assert_eq!(
                    image.public, "true",
                    "case {case:?}: a published image is visible to every account, got \
                     {image:?}"
                );
                assert_eq!(
                    &image.architecture, architecture,
                    "case {case:?}: the pattern names that architecture, got {image:?}"
                );
                // A request that does not ask for deprecated images gets none: a reported
                // image either carries no deprecation time at all or carries one that has not
                // been reached yet, which is what a build of a family still in support does.
                if let Some(at) = image.deprecation_time.as_deref() {
                    let deprecates_at = as_timestamp(at).unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a deprecation time must read as an instant, got \
                             {image:?}"
                        )
                    });
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("the clock is after the epoch")
                        .as_secs_f64();
                    assert!(
                        deprecates_at > now,
                        "case {case:?}: a request that does not ask for deprecated images gets \
                         none, so any deprecation time it reports is still ahead, got {image:?}"
                    );
                }
                assert!(
                    as_timestamp(&image.creation_date).is_some(),
                    "case {case:?}: the creation date a caller sorts on must be an instant, got \
                     {image:?}"
                );
            }

            // The documented way to pick one: the newest `CreationDate` of the matches. It is
            // launched once per variant, the launch being about the id rather than about which
            // way the request named its owner.
            let picked = newest(&case, &reported);
            if form == OwnerForm::OwnersMember {
                let ran = launch_and_read_back(&case, &router, &picked.image_id).await;
                assert_eq!(
                    ran, picked.image_id,
                    "case {case:?}: the instance must report the AMI it was launched from, got \
                     {ran:?}"
                );
            }
        }
    }
}

/// The same rule read off the catalog itself rather than off the table above: every build
/// the store publishes as current is reported by a read of its own variant's name pattern,
/// so no published variant is left with deprecated builds only.
#[tokio::test]
async fn every_current_catalog_build_is_reported_under_its_own_name_pattern() {
    let (router, _engine) = super::ec2_query::emulator();

    let current: Vec<ec2::state::ImageSpec> = ec2::state::published_image_specs()
        .into_iter()
        .filter(|spec| spec.deprecation_time.is_none())
        .collect();
    assert!(
        !current.is_empty(),
        "the catalog must publish at least one current build to launch from"
    );

    for spec in &current {
        let case = spec.name.clone();
        let pattern = variant_pattern(&spec.name);
        let reported = described(
            &case,
            &router,
            &resolve_by_name(OwnerForm::OwnersMember, Some(&pattern)),
        )
        .await;
        assert!(
            reported.iter().any(|image| image.name == spec.name),
            "case {case:?}: the pattern {pattern:?} its own published name gives must \
             report it as an available image, got {reported:?}"
        );
    }
}

