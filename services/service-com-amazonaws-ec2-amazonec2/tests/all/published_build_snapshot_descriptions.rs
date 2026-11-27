// SPDX-License-Identifier: CC0-1.0

//! How the snapshots of the PUBLISHED catalogue describe themselves, driven over the wire the
//! service speaks (`ec2Query`) through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. A SNAPSHOT OF THE PUBLISHED CATALOGUE DESCRIBES ITSELF WITH THE NAME THE BUILD IT BACKS
//!    IS PUBLISHED UNDER. A published build is identified by that name — the variant, the
//!    architecture and the version are all in it — so `DescribeSnapshots` narrowed to the
//!    publisher and filtered on `description` with a build's published name selects exactly
//!    the snapshots of the builds whose names match, and each selected item reports the full
//!    snapshot projection. That is how a client finds the snapshots of one published build,
//!    and the expectation of every row is READ OFF `DescribeImages` rather than written here;
//! 2. A name no published build carries selects NOTHING — HTTP 200 with an empty set, which is
//!    the same answer `DescribeImages` gives for that name, so neither read invents a build;
//! 3. THE CATALOGUE PUBLISHES THE BOTTLEROCKET NODE IMAGES: one build per variant that project
//!    builds a container-host image for — each supported Kubernetes minor, with and without
//!    the accelerator drivers, in both architectures, beside its Amazon ECS variant — each
//!    resolvable by its published name and each backed by a snapshot of the inventory.
//!
//! Each table row is a distinct input along what the rules generalise over: which family's
//! published name form the filter carries, which wildcard it uses, which variant and
//! architecture of a family is asked for, and whether any build carries the name at all — so
//! no row is the specification.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state::{
    BYTES_PER_GIB, IMAGE_OWNER_ALIAS_AMAZON, SNAPSHOT_PROGRESS_COMPLETED,
    SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER, SNAPSHOT_STATE_COMPLETED, SNAPSHOT_STORAGE_TIER_STANDARD,
    SNAPSHOT_TRANSFER_TYPE_STANDARD,
};

use super::describe_snapshots::{described_snapshots, ReportedSnapshot};
use super::ec2_query::{as_timestamp, filters, indexed};
use super::wire::{child, child_text, document, param, set_items, Param};

/// The Region every call here is signed for; the published catalogue is per account.
const REGION: &str = "us-east-1";

/// One `action` call signed for that Region.
async fn op(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One published build, as `DescribeImages` reports it: the name it is published under, the
/// architecture it is built for, and the snapshot each of its Amazon EBS devices restores from.
#[derive(Debug, Clone)]
struct PublishedBuild {
    name: String,
    architecture: String,
    snapshot_ids: Vec<String>,
}

/// Every build of the publisher's catalogue whose `Image$Name` matches `pattern`.
///
/// `IncludeDeprecated`, because a deprecated build is left out of a describe by default ("if
/// `true`, all deprecated AMIs are included in the response",
/// `DescribeImagesRequest$IncludeDeprecated`) while the snapshot it restores from is a snapshot
/// of the inventory like any other.
async fn published_builds(
    router: &MultiServiceRouter,
    case: &str,
    pattern: &str,
) -> Vec<PublishedBuild> {
    let mut params = filters(&[("name", &[pattern])]);
    params.extend(indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]));
    params.push(param("IncludeDeprecated", "true"));
    let (status, body) = op(router, "DescribeImages", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "imageId").is_some())
        .map(|node| PublishedBuild {
            name: child_text(node, "name").unwrap_or_default(),
            architecture: child_text(node, "architecture").unwrap_or_default(),
            snapshot_ids: set_items(node, "blockDeviceMapping")
                .into_iter()
                .filter_map(|mapping| child(mapping, "ebs"))
                .filter_map(|ebs| child_text(ebs, "snapshotId"))
                .collect(),
        })
        .collect()
}

/// The snapshots of the publisher's inventory whose `description` matches `pattern`.
async fn snapshots_described_by(
    router: &MultiServiceRouter,
    case: &str,
    pattern: &str,
) -> Vec<ReportedSnapshot> {
    let mut params = filters(&[("description", &[pattern])]);
    params.extend(indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]));
    let (status, body) = op(router, "DescribeSnapshots", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body {body}"
    );
    described_snapshots(case, &body)
}

/// The inventory's record of one snapshot, resolved by id.
async fn snapshot_by_id(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
) -> Vec<ReportedSnapshot> {
    let (status, body) = op(
        router,
        "DescribeSnapshots",
        &indexed("SnapshotId", &[snapshot_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body {body}"
    );
    described_snapshots(case, &body)
}

/// `(snapshot id, the description it is expected to carry)` pairs, sorted so a row compares
/// against a SET rather than against one reporting order.
fn sorted_pairs(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    pairs.sort();
    pairs
}

/// The published name forms a client filters on, one per family the catalogue publishes.
///
/// Source: the `Image$Name` forms of the variants the catalogue publishes
/// (`published_image_specs` in the crate's state: the Amazon Linux families, the Windows
/// Server base image, the third-party distributions AWS curates, the AWS PCS sample images and
/// the container-host builds). The last row carries EC2's other wildcard — "`?` matches
/// exactly one character" ("List and filter using the CLI and API", the guide
/// `Filter$Values` links to) — so the rule is read with both wildcard forms.
const PUBLISHED_NAME_PATTERNS: &[&str] = &[
    "bottlerocket-aws-k8s-1.34-nvidia-aarch64-*",
    "bottlerocket-aws-k8s-1.33-x86_64-*",
    "bottlerocket-aws-k8s-1.32-nvidia-x86_64-*",
    "bottlerocket-aws-ecs-2-aarch64-*",
    "al2023-ami-2023.*-kernel-6.1-x86_64",
    "amzn2-ami-hvm-*-arm64-gp2",
    "Windows_Server-2022-English-Full-Base-*",
    "RHEL-10.0.0_HVM-*",
    "suse-sles-16-0-*",
    "aws-pcs-sample_ami-al2023-*",
    "bottlerocket-aws-k8s-1.3?-x86_64-*",
];

/// RULE 1: the `description` filter resolves the snapshots of exactly the published builds
/// whose names match it, and each reported snapshot describes itself with that build's name.
#[tokio::test]
async fn the_snapshots_of_a_published_build_carry_its_published_name() {
    let (router, _engine) = emulator();

    for pattern in PUBLISHED_NAME_PATTERNS.iter().copied() {
        let builds = published_builds(&router, pattern, pattern).await;
        assert!(
            !builds.is_empty(),
            "case {pattern:?}: the catalogue publishes a build of this family, got none"
        );
        // What the inventory has to report for this pattern, read off the images themselves:
        // one snapshot per Amazon EBS device of each matching build, described by the name
        // that build is published under.
        let expected = sorted_pairs(
            builds
                .iter()
                .flat_map(|build| {
                    build
                        .snapshot_ids
                        .iter()
                        .map(|snapshot_id| (snapshot_id.clone(), build.name.clone()))
                })
                .collect(),
        );
        assert!(
            !expected.is_empty(),
            "case {pattern:?}: an EBS-backed build restores its devices from snapshots, got \
             {builds:?}"
        );

        let reported = snapshots_described_by(&router, pattern, pattern).await;
        let actual = sorted_pairs(
            reported
                .iter()
                .map(|snapshot| {
                    (
                        snapshot.member("snapshotId").to_string(),
                        snapshot.member("description").to_string(),
                    )
                })
                .collect(),
        );
        assert_eq!(
            actual, expected,
            "case {pattern:?}: expected the snapshots of the builds whose published names match, \
             each described by its build's name, got {actual:?}"
        );

        for snapshot in &reported {
            let case = format!("{pattern} / {}", snapshot.member("snapshotId"));
            for (member, expected) in [
                ("status", SNAPSHOT_STATE_COMPLETED),
                ("progress", SNAPSHOT_PROGRESS_COMPLETED),
                ("storageTier", SNAPSHOT_STORAGE_TIER_STANDARD),
                ("transferType", SNAPSHOT_TRANSFER_TYPE_STANDARD),
                ("volumeId", SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER),
                ("ownerAlias", IMAGE_OWNER_ALIAS_AMAZON),
                ("encrypted", "false"),
            ] {
                assert_eq!(
                    snapshot.member(member),
                    expected,
                    "case {case:?}: expected <{member}> {expected:?}, got {:?}",
                    snapshot.member(member)
                );
            }
            assert!(
                !snapshot.member("ownerId").is_empty(),
                "case {case:?}: a published snapshot names the account that owns it, got {:?}",
                snapshot.member("ownerId")
            );
            for member in ["startTime", "completionTime"] {
                assert!(
                    as_timestamp(snapshot.member(member)).is_some(),
                    "case {case:?}: expected an instant in <{member}>, got {:?}",
                    snapshot.member(member)
                );
            }
            let volume_size: i64 = snapshot.member("volumeSize").parse().unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: expected a volume size in GiB, got {:?}",
                    snapshot.member("volumeSize")
                )
            });
            assert!(
                volume_size > 0,
                "case {case:?}: a published build restores a device of some size, got \
                 {volume_size}"
            );
            assert_eq!(
                snapshot.member("fullSnapshotSizeInBytes"),
                (volume_size * BYTES_PER_GIB).to_string(),
                "case {case:?}: the full size is the {volume_size} GiB device in bytes, got {:?}",
                snapshot.member("fullSnapshotSizeInBytes")
            );
        }
    }
}

/// The names no build of the catalogue carries: a Kubernetes minor no variant is published
/// for, a family nothing publishes at all, and a published variant's name WITHOUT the version
/// every build's name ends with — which is a description no snapshot carries even though the
/// variant is published.
const UNPUBLISHED_NAMES: &[&str] = &[
    "bottlerocket-aws-k8s-9.99-nvidia-aarch64-*",
    "no-such-published-build-*",
    "bottlerocket-aws-k8s-1.34-nvidia-aarch64",
];

/// RULE 2: a name no published build carries selects no snapshot, and no image either.
#[tokio::test]
async fn a_name_no_published_build_carries_selects_nothing() {
    let (router, _engine) = emulator();

    for pattern in UNPUBLISHED_NAMES.iter().copied() {
        let builds = published_builds(&router, pattern, pattern).await;
        assert!(
            builds.is_empty(),
            "case {pattern:?}: no build of the catalogue carries this name, got {builds:?}"
        );
        let reported = snapshots_described_by(&router, pattern, pattern).await;
        assert!(
            reported.is_empty(),
            "case {pattern:?}: expected no snapshot for a name nothing publishes, got {:?}",
            reported
                .iter()
                .map(|snapshot| snapshot.member("snapshotId").to_string())
                .collect::<Vec<String>>()
        );
    }
}

/// The Bottlerocket node images the catalogue publishes: `(the variant, the architecture its
/// published name spells, the architecture EC2 reports)`.
///
/// Source: the paths the project's images are published under in Parameter Store
/// (`/aws/service/bottlerocket/<variant>/<architecture>/latest/image_id`) — one per variant a
/// container host is launched from: each supported Kubernetes minor with and without the
/// accelerator drivers, and the Amazon ECS variant. The name spells 64-bit Arm `aarch64` the
/// way that project does, while `Image$Architecture` is EC2's own `arm64`.
const BOTTLEROCKET_VARIANTS: &[(&str, &str, &str)] = &[
    ("aws-k8s-1.32", "x86_64", "x86_64"),
    ("aws-k8s-1.32", "aarch64", "arm64"),
    ("aws-k8s-1.32-nvidia", "x86_64", "x86_64"),
    ("aws-k8s-1.32-nvidia", "aarch64", "arm64"),
    ("aws-k8s-1.33", "x86_64", "x86_64"),
    ("aws-k8s-1.33", "aarch64", "arm64"),
    ("aws-k8s-1.33-nvidia", "x86_64", "x86_64"),
    ("aws-k8s-1.33-nvidia", "aarch64", "arm64"),
    ("aws-k8s-1.34", "x86_64", "x86_64"),
    ("aws-k8s-1.34", "aarch64", "arm64"),
    ("aws-k8s-1.34-nvidia", "x86_64", "x86_64"),
    ("aws-k8s-1.34-nvidia", "aarch64", "arm64"),
    ("aws-ecs-2", "x86_64", "x86_64"),
    ("aws-ecs-2", "aarch64", "arm64"),
];

/// RULE 3: every Bottlerocket variant the catalogue publishes resolves to one build, and that
/// build's snapshot is in the inventory under the build's own name.
#[tokio::test]
async fn the_catalogue_publishes_a_build_of_every_bottlerocket_variant() {
    let (router, _engine) = emulator();

    for &(variant, name_architecture, reported_architecture) in BOTTLEROCKET_VARIANTS {
        let prefix = format!("bottlerocket-{variant}-{name_architecture}-");
        let pattern = format!("{prefix}*");
        let builds = published_builds(&router, &pattern, &pattern).await;
        assert_eq!(
            builds.len(),
            1,
            "case {pattern:?}: the catalogue publishes one current build of a variant, got {:?}",
            builds
                .iter()
                .map(|build| build.name.clone())
                .collect::<Vec<String>>()
        );
        let build = &builds[0];
        assert!(
            build.name.starts_with(&prefix),
            "case {pattern:?}: expected a name published as {prefix:?}…, got {:?}",
            build.name
        );
        assert_eq!(
            build.architecture, reported_architecture,
            "case {pattern:?}: expected the architecture {reported_architecture:?}, got {:?}",
            build.architecture
        );
        assert_eq!(
            build.snapshot_ids.len(),
            1,
            "case {pattern:?}: the build restores its root device from one snapshot, got {:?}",
            build.snapshot_ids
        );

        // The snapshot behind the build is one the inventory resolves BY ID too, and it
        // describes itself with the build's name there as well — so the two reads of one
        // snapshot cannot disagree.
        let snapshot_id = &build.snapshot_ids[0];
        let reported = snapshot_by_id(&router, &pattern, snapshot_id).await;
        assert_eq!(
            reported.len(),
            1,
            "case {pattern:?}: the inventory resolves {snapshot_id:?}, got {} snapshots",
            reported.len()
        );
        assert_eq!(
            reported[0].member("description"),
            build.name,
            "case {pattern:?}: expected the snapshot of the build to be described by its name, \
             got {:?}",
            reported[0].member("description")
        );
    }
}
