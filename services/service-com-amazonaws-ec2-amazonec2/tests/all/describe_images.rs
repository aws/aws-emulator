// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeImages`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeImages`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the AMIs the calling account can see, narrowed by the `ImageIds`, `Owners`,
//! `ExecutableUsers` and `Filters` the request names, with deprecated and disabled images
//! left out unless asked for, cut into `MaxResults`-sized pages. Every table row is a
//! distinct input along what that generalises over — which filter, which values, which
//! owner term, which ids, which page size — so the Amazon Linux image a bootstrap script
//! resolves by name is one row among many rather than the specification.

use std::collections::BTreeMap;

use ec2::ops::types::{
    ImageWatermark, InstanceTypeItem, InstanceTypeSpecification, ProductCode, StateReason, Tag,
};
use ec2::state::{
    AMAZON_IMAGE_OWNER_ID, ComAmazonawsEc2Amazonec2State as Ec2State, IMAGE_STATE_DISABLED,
    ImageSpec, image_is_free_tier_eligible, published_image_specs,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The image store is per account rather than
/// per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// Another account, which publishes one public image and keeps one private.
const PARTNER_ACCOUNT: &str = "111122223333";

/// The account a Marketplace appliance is published from.
const MARKETPLACE_ACCOUNT: &str = "679593333241";

/// The watermark attachment time one seeded image carries, as the
/// `image-watermark.watermark-creation-time` filter renders it.
const WATERMARK_TIME: &str = "2026-05-01T00:00:00Z";

/// The creation time of the image that watermark was attached from.
const WATERMARK_SOURCE_TIME: &str = "2025-11-02T09:15:00Z";

/// One `DescribeImages` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeImages", params).await
}

/// The image ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "imageId")
}

/// One `(key, value)` request parameter.
fn param(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// `IncludeDeprecated` and `IncludeDisabled` both on, so a table about FILTERS is not also
/// a table about the two defaults (which have their own test below).
fn include_everything() -> Vec<(String, String)> {
    vec![
        param("IncludeDeprecated", "true"),
        param("IncludeDisabled", "true"),
    ]
}

/// A tag, as the seeded images carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// A modelled timestamp parsed from an ISO 8601 UTC literal.
fn timestamp(value: &str) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("{value} must parse as a timestamp: {error}"))
}

/// One image row of a response: the members the assertions below read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageRow {
    image_id: String,
    name: String,
    creation_date: String,
    state: String,
    owner_id: String,
    owner_alias: String,
    public: String,
}

/// The text of `node`'s `name` child, or the empty string when it has none.
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    node.children()
        .find(|child| child.has_tag_name(name))
        .and_then(|child| child.text())
        .unwrap_or_default()
        .to_string()
}

/// Every image of a response body, as rows.
fn image_rows(body: &str) -> Vec<ImageRow> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| {
            node.is_element() && node.children().any(|child| child.has_tag_name("imageId"))
        })
        .map(|node| ImageRow {
            image_id: child_text(node, "imageId"),
            name: child_text(node, "name"),
            creation_date: child_text(node, "creationDate"),
            state: child_text(node, "imageState"),
            owner_id: child_text(node, "imageOwnerId"),
            owner_alias: child_text(node, "imageOwnerAlias"),
            public: child_text(node, "isPublic"),
        })
        .collect()
}

/// The images seeded beside the published catalog, written the way a register operation
/// would write them. They differ along every attribute the documented filters select on:
/// architecture, lifecycle state, deprecation, ownership, launch permission and tags.
fn seeded_specs(account: &str) -> Vec<ImageSpec> {
    vec![
        ImageSpec {
            name: "app-golden-2026-01-x86_64".to_string(),
            description: "The account's own golden image".to_string(),
            tags: vec![tag("Name", "golden"), tag("Owner", "TeamA")],
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "app-golden-2026-01-arm64".to_string(),
            description: "The account's own golden image, Graviton".to_string(),
            architecture: "arm64".to_string(),
            public: true,
            tags: vec![tag("Owner", "TeamB")],
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "legacy-i386-image".to_string(),
            description: "A paravirtual image from before HVM".to_string(),
            architecture: "i386".to_string(),
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "app-disabled-image".to_string(),
            description: "An image its owner disabled".to_string(),
            state: IMAGE_STATE_DISABLED.to_string(),
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "app-retired-image".to_string(),
            description: "An image whose deprecation time has passed".to_string(),
            deprecation_time: Some("2024-01-01T00:00:00.000Z".to_string()),
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "app-sunsetting-image".to_string(),
            description: "An image whose deprecation time is still ahead".to_string(),
            deprecation_time: Some("2999-12-31T23:59:59.000Z".to_string()),
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "partner-public-image".to_string(),
            description: "Another account's public image".to_string(),
            public: true,
            owner_id: PARTNER_ACCOUNT.to_string(),
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "partner-private-image".to_string(),
            description: "Another account's private image".to_string(),
            owner_id: PARTNER_ACCOUNT.to_string(),
            ..ImageSpec::default()
        },
        ImageSpec {
            name: "vendor-appliance-image".to_string(),
            description: "A Marketplace appliance".to_string(),
            public: true,
            owner_id: MARKETPLACE_ACCOUNT.to_string(),
            owner_alias: Some("aws-marketplace".to_string()),
            // The appliance is billed through the product code it is sold under, which is
            // what makes it the one seeded image that is NOT Free-Tier launchable.
            product_codes: vec![ProductCode {
                product_code_id: Some("abc123prodcode".to_string()),
                product_code_type: Some("marketplace".to_string()),
            }],
            ..ImageSpec::default()
        },
        // The account's own image, which this account is not the owner of — every spec
        // above that names no owner belongs to it.
        ImageSpec {
            name: "app-windows-image".to_string(),
            description: "The account's own Windows image".to_string(),
            platform: Some("Windows".to_string()),
            owner_id: account.to_string(),
            ..ImageSpec::default()
        },
    ]
}

/// What a seeded store holds: every image id by name, for the tables to name rows by.
struct Seeded {
    ids_by_name: BTreeMap<String, String>,
    /// The snapshot id of the Windows catalog image's root block device mapping.
    windows_snapshot_id: String,
}

impl Seeded {
    /// The ids of `names`, in the order the service reports them — image ids are the
    /// zero-padded hex of a monotonic counter, so sorting them is the store's own order.
    fn ids_for(&self, names: &[&str]) -> Vec<String> {
        let mut ids: Vec<String> = names
            .iter()
            .map(|name| {
                self.ids_by_name
                    .get(*name)
                    .unwrap_or_else(|| panic!("{name} must be a seeded image"))
                    .clone()
            })
            .collect();
        ids.sort();
        ids
    }

    /// The id of one image by name.
    fn id_for(&self, name: &str) -> String {
        self.ids_for(&[name]).remove(0)
    }
}

/// The names of the published catalog images, from the store's own catalog rather than a
/// copy of it, so the tables cannot drift from what a fresh account holds.
fn catalog_names() -> Vec<String> {
    published_image_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect()
}

/// The catalog images a request that asks for nothing extra must report: every published
/// image whose deprecation time has not passed (the ones that have need
/// `IncludeDeprecated`), read through the crate's one reading of that question
/// (`image_spec_is_deprecated`) — a build of a family still in support carries a deprecation
/// time that is still AHEAD, so it is reported without the flag.
fn undeprecated_catalog_names() -> Vec<String> {
    let now = std::time::SystemTime::now();
    published_image_specs()
        .into_iter()
        .filter(|spec| !ec2::state::image_spec_is_deprecated(spec, now))
        .map(|spec| spec.name)
        .collect()
}

/// The published names of the catalog's DEPRECATED images, in catalog order.
///
/// Read from the catalog rather than written out, because those images are dated relative
/// to the instant the catalog is seeded — so that the deprecated/not-deprecated contrast
/// always holds — and their published names carry that build date.
static DEPRECATED_CATALOG_NAMES: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| {
        let now = std::time::SystemTime::now();
        published_image_specs()
            .into_iter()
            .filter(|spec| ec2::state::image_spec_is_deprecated(spec, now))
            .map(|spec| spec.name)
            .collect()
    });

/// Every image a `free-tier-eligible` filter must select, read through the crate's one
/// reading of that question (`image_is_free_tier_eligible`) rather than written out, so the
/// row cannot drift as the catalog grows: every image this account can SEE whose own billing
/// adds nothing to the instance's, which is all of them but the Marketplace appliance.
fn free_tier_names(account: &str) -> Vec<String> {
    published_image_specs()
        .into_iter()
        .chain(seeded_specs(account))
        // Another account's private image is not visible to this one at all, so no filter
        // selects it.
        .filter(|spec| spec.public || spec.owner_id == account)
        .filter(|spec| image_is_free_tier_eligible(&spec.product_codes))
        .map(|spec| spec.name)
        .collect()
}

/// `names` as the borrowed slice the tables take.
fn borrowed(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

/// The published catalog's names, narrowed to the images one filter row selects.
///
/// Read off the catalog a fresh account holds rather than frozen into the row, so a
/// published variant the catalog gains later — the catalog publishes one image per public
/// Systems Manager alias, across every Amazon Linux family — is covered without rewriting
/// the table.
fn catalog_names_where(selects: impl Fn(&ImageSpec) -> bool) -> Vec<String> {
    published_image_specs()
        .into_iter()
        .filter(|spec| selects(spec))
        .map(|spec| spec.name)
        .collect()
}

/// `catalog` followed by the seeded names `extra`, as the one borrowed slice a row takes.
fn borrowed_with<'a>(catalog: &'a [String], extra: &[&'a str]) -> Vec<&'a str> {
    let mut all = borrowed(catalog);
    all.extend(extra);
    all
}

/// Write the seeded images into the engine's store, the way a register operation would,
/// and report the ids they were given. The members no [`ImageSpec`] carries — a state
/// reason, an instance-type specification, a watermark, a kernel and RAM disk — are written
/// onto the stored images here, so every documented filter has something to select on.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let account = engine::emulator_account_id().to_string();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for spec in seeded_specs(&account) {
        guard.insert_image(spec);
    }

    let ids_by_name: BTreeMap<String, String> = guard
        .images
        .values()
        .filter_map(|image| Some((image.name.clone()?, image.image_id.clone()?)))
        .collect();
    let id_of = |name: &str| {
        ids_by_name
            .get(name)
            .unwrap_or_else(|| panic!("{name} must be in the store"))
            .clone()
    };

    // The paravirtual image from before HVM: it is the one image whose root volume is not
    // deleted on termination, is encrypted, carries a kernel and a RAM disk, and runs on
    // instance store.
    if let Some(image) = guard.images.get_mut(&id_of("legacy-i386-image")) {
        image.kernel_id = Some("aki-0123456789abcdef0".to_string());
        image.ramdisk_id = Some("ari-0123456789abcdef0".to_string());
        image.virtualization_type = Some("paravirtual".to_string());
        image.hypervisor = Some("ovm".to_string());
        image.root_device_type = Some("instance-store".to_string());
        image.ena_support = Some(false);
        for mapping in image.block_device_mappings.iter_mut().flatten() {
            if let Some(ebs) = mapping.ebs.as_mut() {
                ebs.delete_on_termination = Some(false);
                ebs.encrypted = Some(true);
            }
        }
    }
    // The retired image records why it left `available`.
    if let Some(image) = guard.images.get_mut(&id_of("app-retired-image")) {
        image.state_reason = Some(StateReason {
            code: Some("Client.UserInitiated".to_string()),
            message: Some("Image deregistration requested by the owner".to_string()),
        });
    }
    // The image being sunset names the instance types its owner supports.
    if let Some(image) = guard.images.get_mut(&id_of("app-sunsetting-image")) {
        image.instance_type_specification = Some(InstanceTypeSpecification {
            supported_instance_types: Some(vec![
                InstanceTypeItem {
                    instance_type: Some("t3.micro".to_string()),
                },
                InstanceTypeItem {
                    instance_type: Some("t3.small".to_string()),
                },
            ]),
            unsupported_instance_types: Some(vec![InstanceTypeItem {
                instance_type: Some("m5.large".to_string()),
            }]),
        });
    }
    // The golden image was created from an instance, copied from another Region's image,
    // and carries a watermark.
    if let Some(image) = guard.images.get_mut(&id_of("app-golden-2026-01-x86_64")) {
        image.source_instance_id = Some("i-0123456789abcdef0".to_string());
        image.source_image_id = Some("ami-0fedcba987654321f".to_string());
        image.source_image_region = Some("eu-west-1".to_string());
        image.image_watermarks = Some(vec![ImageWatermark {
            watermark_key: Some(format!("{account}:approvedAmi")),
            source_image_region: Some("us-west-2".to_string()),
            source_image_id: Some("ami-0123456789abcdef0".to_string()),
            source_image_creation_time: Some(timestamp(WATERMARK_SOURCE_TIME)),
            watermark_creation_time: Some(timestamp(WATERMARK_TIME)),
        }]);
    }

    let windows_snapshot_id = guard
        .images
        .get(&id_of("Windows_Server-2022-English-Full-Base-2024.11.13"))
        .and_then(|image| {
            image
                .block_device_mappings
                .iter()
                .flatten()
                .next()
                .and_then(|mapping| mapping.ebs.as_ref())
                .and_then(|ebs| ebs.snapshot_id.clone())
        })
        .expect("the Windows catalog image has a root block device mapping");

    Seeded {
        ids_by_name,
        windows_snapshot_id,
    }
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account already sees the public images Amazon publishes — nothing in the API
/// creates them — and describing them is a READ: two calls report the same catalog and the
/// store is unchanged.
#[tokio::test]
async fn a_fresh_account_sees_the_published_images_and_describing_them_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let rows = image_rows(&first);
    let reported_names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    let mut expected_names = undeprecated_catalog_names();
    expected_names.sort();
    let mut sorted_names = reported_names.clone();
    sorted_names.sort();
    assert_eq!(
        sorted_names, expected_names,
        "fresh account: expected the published catalog, got {reported_names:?}"
    );

    for row in &rows {
        assert!(
            row.image_id.starts_with("ami-"),
            "fresh account: every image needs an AMI id, got {row:?}"
        );
        assert_eq!(
            row.state, "available",
            "fresh account: a published image is available, got {row:?}"
        );
        assert_eq!(
            row.owner_alias, "amazon",
            "fresh account: a published image carries the amazon alias, got {row:?}"
        );
        assert_eq!(
            row.owner_id, AMAZON_IMAGE_OWNER_ID,
            "fresh account: a published image is owned by Amazon, got {row:?}"
        );
        assert_eq!(
            row.public, "true",
            "fresh account: a published image is public, got {row:?}"
        );
        assert!(
            !row.creation_date.is_empty(),
            "fresh account: every image reports a creation date, got {row:?}"
        );
    }

    let (second_status, second) = describe(&router, &[]).await;
    assert_eq!(
        second_status, 200,
        "second read: expected HTTP 200, got {second_status} with body {second}"
    );
    assert_eq!(
        image_rows(&second),
        rows,
        "a read must not change what a later caller sees"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().images.len(),
        published_image_specs().len(),
        "describing images must not write to the store"
    );
}

/// Every image the store holds is reported, and an image written after a call shows up in
/// the next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn an_image_written_after_a_call_is_reported_by_the_next_one() {
    let (router, engine) = super::ec2_query::emulator();

    let before = described_ids(&router, "before the write", &[]).await;
    let late_id = engine.state::<Ec2State>().lock().insert_image(ImageSpec {
        name: "written-after-the-first-call".to_string(),
        ..ImageSpec::default()
    });
    let after = described_ids(&router, "after the write", &[]).await;
    let mut expected = before.clone();
    expected.push(late_id.clone());
    expected.sort();
    assert_eq!(
        after, expected,
        "after the write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Resolving an AMI by name, which is what a launch needs
// ---------------------------------------------------------------------------

/// The portable way to pick an AMI: describe the images of an owner whose name matches a
/// pattern and which are available, take the newest `CreationDate`, and launch it. Each
/// row is a different name pattern, and the id the newest match carries must be usable in
/// `RunInstances` and reported back by `DescribeInstances`.
#[tokio::test]
async fn the_newest_image_matching_a_name_pattern_can_be_launched() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the minimal and the standard Amazon Linux 2023 variants, both architectures,
    // an Amazon Linux 2 build and a Windows base image — with the published name the
    // newest build of each carries.
    let cases = [
        (
            "al2023-ami-minimal-*-x86_64",
            "al2023-ami-minimal-2023.8.20251104.0-kernel-6.18-x86_64",
        ),
        (
            "al2023-ami-2023.*-kernel-6.1-x86_64",
            "al2023-ami-2023.6.20241121.0-kernel-6.1-x86_64",
        ),
        (
            "al2023-ami-2023.*-arm64",
            "al2023-ami-2023.8.20251104.0-kernel-6.18-arm64",
        ),
        (
            "amzn2-ami-*-x86_64-gp2",
            "amzn2-ami-kernel-5.10-hvm-2.0.20241113.1-x86_64-gp2",
        ),
        (
            "Windows_Server-2022-*",
            "Windows_Server-2022-English-Full-Base-2024.11.13",
        ),
    ];

    for (pattern, expected_newest) in cases {
        let mut params = indexed("Owner", &["amazon"]);
        params.extend(filters(&[
            ("name", &[pattern]),
            ("state", &["available"]),
        ]));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {pattern:?}: expected HTTP 200, got {status} with body {body}"
        );
        let rows = image_rows(&body);
        assert!(
            !rows.is_empty(),
            "case {pattern:?}: expected a non-empty image set, got {body}"
        );
        for row in &rows {
            assert_eq!(
                row.state, "available",
                "case {pattern:?}: the state filter must hold, got {row:?}"
            );
        }
        let newest = rows
            .iter()
            .max_by(|left, right| left.creation_date.cmp(&right.creation_date))
            .expect("a non-empty set has a newest image");
        assert_eq!(
            newest.name, expected_newest,
            "case {pattern:?}: expected the newest match to be {expected_newest:?}, got {newest:?}"
        );

        // The id the newest match carries launches, and the instance reports it back. The
        // launch names a type whose architecture is the AMI's own: an instance runs the
        // machine code its AMI holds.
        let mut launch = vec![
            param("ImageId", newest.image_id.clone()),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ];
        launch.extend(
            super::ec2_query::launch_type_for_image(&router, REGION, pattern, &newest.image_id)
                .await,
        );
        let run = super::ec2_query::call(&router, REGION, "RunInstances", &launch).await;
        assert_eq!(
            run.0, 200,
            "case {pattern:?}: RunInstances with the resolved id must succeed, got {} {}",
            run.0, run.1
        );
        let instance_id = elements(&run.1, "instanceId")
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {pattern:?}: RunInstances must mint an instance"));

        let described = super::ec2_query::call(
            &router,
            REGION,
            "DescribeInstances",
            &indexed("InstanceId", &[instance_id.as_str()]),
        )
        .await;
        assert_eq!(
            described.0, 200,
            "case {pattern:?}: DescribeInstances must succeed, got {} {}",
            described.0, described.1
        );
        let reported = elements(&described.1, "imageId");
        assert_eq!(
            reported,
            vec![newest.image_id.clone()],
            "case {pattern:?}: the instance must report the AMI it was launched from, got \
             {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is case-sensitive with EC2's `*`/`?` wildcards.
#[tokio::test]
async fn each_documented_filter_selects_the_images_whose_attribute_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let catalog = catalog_names();
    let free_tier = free_tier_names(&account);
    // The catalog images each attribute-bound row selects, read off the catalog itself.
    let arm64_catalog = catalog_names_where(|spec| spec.architecture == "arm64");
    let gp2_catalog = catalog_names_where(|spec| spec.root_volume_type == "gp2");
    let uefi_catalog =
        catalog_names_where(|spec| spec.boot_mode.as_deref() == Some("uefi"));
    let legacy_bios_catalog =
        catalog_names_where(|spec| spec.boot_mode.as_deref() == Some("legacy-bios"));
    let minimal_catalog = catalog_names_where(|spec| spec.description.ends_with("Minimal"));
    let minimal_al2023_arm64 = catalog_names_where(|spec| {
        spec.name.starts_with("al2023-ami-minimal-") && spec.name.ends_with("-arm64")
    });
    let minimal_al2023_x86_64 = catalog_names_where(|spec| {
        spec.name.starts_with("al2023-ami-minimal-") && spec.name.ends_with("-x86_64")
    });
    let sda1_catalog = catalog_names_where(|spec| spec.root_device_name == "/dev/sda1");
    let instance_store_catalog =
        catalog_names_where(|spec| spec.root_device_type == "instance-store");
    let paravirtual_catalog =
        catalog_names_where(|spec| spec.virtualization_type == "paravirtual");
    let windows_snapshot = seeded.windows_snapshot_id.clone();
    let golden_location = format!("{account}/app-golden-2026-01-x86_64");
    let watermark_key = format!("{account}:approvedAmi");
    let fifth_catalog_id = seeded.id_for("al2023-ami-2023.5.20240903.0-kernel-6.1-x86_64");
    let windows_id = seeded.id_for("Windows_Server-2022-English-Full-Base-2024.11.13");

    // One row (or more) per filter name `DescribeImagesRequest$Filters` documents, plus
    // the variable-key `tag:<key>` form. `expected` names the images that must come back.
    let cases: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        (
            "architecture",
            vec!["arm64".into()],
            borrowed_with(&arm64_catalog, &["app-golden-2026-01-arm64"]),
        ),
        ("architecture", vec!["i386".into()], vec!["legacy-i386-image"]),
        // Two values for one filter: joined with OR.
        (
            "architecture",
            vec!["i386".into(), "arm64".into()],
            borrowed_with(
                &arm64_catalog,
                &["app-golden-2026-01-arm64", "legacy-i386-image"],
            ),
        ),
        // An architecture no stored image has: an empty set, not an error.
        ("architecture", vec!["x86_64_mac".into()], vec![]),
        (
            "block-device-mapping.delete-on-termination",
            vec!["false".into()],
            vec!["legacy-i386-image"],
        ),
        (
            // Both published Windows AMIs — the plain one and the NitroTPM-preconfigured
            // build — restore their root volume at the Windows root device name.
            "block-device-mapping.device-name",
            vec!["/dev/sda1".into()],
            vec![
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "TPM-Windows_Server-2022-English-Full-Base-2024.11.13",
            ],
        ),
        ("block-device-mapping.device-name", vec!["/dev/sdh".into()], vec![]),
        (
            "block-device-mapping.encrypted",
            vec!["true".into()],
            vec!["legacy-i386-image"],
        ),
        (
            "block-device-mapping.snapshot-id",
            vec![windows_snapshot.clone()],
            vec!["Windows_Server-2022-English-Full-Base-2024.11.13"],
        ),
        (
            // Both published Windows AMIs restore a 30 GiB root volume.
            "block-device-mapping.volume-size",
            vec!["30".into()],
            vec![
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "TPM-Windows_Server-2022-English-Full-Base-2024.11.13",
            ],
        ),
        (
            "block-device-mapping.volume-type",
            vec!["gp2".into()],
            borrowed(&gp2_catalog),
        ),
        ("boot-mode", vec!["uefi".into()], borrowed(&uefi_catalog)),
        (
            "boot-mode",
            vec!["legacy-bios".into()],
            borrowed(&legacy_bios_catalog),
        ),
        // "You can use a wildcard (*), for example, 2021-09-29T*, which matches an entire
        // day" (the `creation-date` filter).
        (
            "creation-date",
            vec!["2024-11-21T*".into()],
            vec![
                "al2023-ami-2023.6.20241121.0-kernel-6.1-x86_64",
                "al2023-ami-2023.6.20241121.0-kernel-6.1-arm64",
                "al2023-ami-minimal-2023.6.20241121.0-kernel-6.1-x86_64",
                "al2023-ami-minimal-2023.6.20241121.0-kernel-6.1-arm64",
            ],
        ),
        (
            // Both published Windows AMIs are builds of the same release, so they carry the
            // same `CreationDate`.
            "creation-date",
            vec!["2024-11-13T07:36:19.000Z".into()],
            vec![
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "TPM-Windows_Server-2022-English-Full-Base-2024.11.13",
            ],
        ),
        (
            "description",
            vec!["*Minimal".into()],
            borrowed(&minimal_catalog),
        ),
        (
            "ena-support",
            vec!["false".into()],
            borrowed_with(&paravirtual_catalog, &["legacy-i386-image"]),
        ),
        (
            "free-tier-eligible",
            vec!["true".into()],
            borrowed(&free_tier),
        ),
        // The one image billed through a product code of its own.
        (
            "free-tier-eligible",
            vec!["false".into()],
            vec!["vendor-appliance-image"],
        ),
        ("hypervisor", vec!["ovm".into()], vec!["legacy-i386-image"]),
        // No account declares Allowed AMIs criteria, so no image fails them.
        ("image-allowed", vec!["false".into()], vec![]),
        (
            "image-id",
            vec![fifth_catalog_id.clone()],
            vec!["al2023-ami-2023.5.20240903.0-kernel-6.1-x86_64"],
        ),
        (
            "image-id",
            vec![fifth_catalog_id.clone(), windows_id.clone()],
            vec![
                "al2023-ami-2023.5.20240903.0-kernel-6.1-x86_64",
                "Windows_Server-2022-English-Full-Base-2024.11.13",
            ],
        ),
        ("image-type", vec!["kernel".into()], vec![]),
        (
            "image-watermark.source-image-creation-time",
            vec![WATERMARK_SOURCE_TIME.into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "image-watermark.source-image-id",
            vec!["ami-0123456789abcdef0".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "image-watermark.source-image-region",
            vec!["us-west-2".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "image-watermark.watermark-creation-time",
            vec![WATERMARK_TIME.into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "image-watermark.watermark-key",
            vec![watermark_key.clone()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "instance-type-specification.supported-instance-type",
            vec!["t3.*".into()],
            vec!["app-sunsetting-image"],
        ),
        (
            "instance-type-specification.unsupported-instance-type",
            vec!["m5.large".into()],
            vec!["app-sunsetting-image"],
        ),
        (
            "is-public",
            vec!["false".into()],
            vec![
                "app-golden-2026-01-x86_64",
                "legacy-i386-image",
                "app-disabled-image",
                "app-retired-image",
                "app-sunsetting-image",
                "app-windows-image",
            ],
        ),
        (
            "kernel-id",
            vec!["aki-0123456789abcdef0".into()],
            vec!["legacy-i386-image"],
        ),
        (
            "manifest-location",
            vec!["amazon/al2023-ami-minimal-*-arm64".into()],
            borrowed(&minimal_al2023_arm64),
        ),
        (
            "manifest-location",
            vec![golden_location.clone()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "name",
            vec!["al2023-ami-minimal-*-x86_64".into()],
            borrowed(&minimal_al2023_x86_64),
        ),
        (
            "name",
            vec!["app-golden-2026-01-arm64".into()],
            vec!["app-golden-2026-01-arm64"],
        ),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("name", vec!["APP-GOLDEN-2026-01-ARM64".into()], vec![]),
        ("owner-alias", vec!["amazon".into()], borrowed(&catalog)),
        (
            "owner-alias",
            vec!["aws-marketplace".into()],
            vec!["vendor-appliance-image"],
        ),
        // The partner's PRIVATE image is not visible to this account, so filtering on the
        // partner's account id reports only its public one.
        (
            "owner-id",
            vec![PARTNER_ACCOUNT.into()],
            vec!["partner-public-image"],
        ),
        // The member carries the modelled `PlatformValues` value, the filter documents the
        // lowercase spelling, and either selects the Windows images.
        (
            "platform",
            vec!["windows".into()],
            vec![
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "TPM-Windows_Server-2022-English-Full-Base-2024.11.13",
                "app-windows-image",
            ],
        ),
        (
            "platform",
            vec!["Windows".into()],
            vec![
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "TPM-Windows_Server-2022-English-Full-Base-2024.11.13",
                "app-windows-image",
            ],
        ),
        (
            "product-code",
            vec!["abc123prodcode".into()],
            vec!["vendor-appliance-image"],
        ),
        (
            "product-code.type",
            vec!["marketplace".into()],
            vec!["vendor-appliance-image"],
        ),
        (
            // Amazon publishes a parameter per Windows AMI family, the NitroTPM-preconfigured
            // build included.
            "public-ssm-parameter-name",
            vec!["/aws/service/ami-windows-latest/*".into()],
            vec![
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "TPM-Windows_Server-2022-English-Full-Base-2024.11.13",
            ],
        ),
        (
            "ramdisk-id",
            vec!["ari-0123456789abcdef0".into()],
            vec!["legacy-i386-image"],
        ),
        (
            "root-device-name",
            vec!["/dev/sda1".into()],
            borrowed(&sda1_catalog),
        ),
        (
            "root-device-type",
            vec!["instance-store".into()],
            borrowed_with(&instance_store_catalog, &["legacy-i386-image"]),
        ),
        (
            "source-image-id",
            vec!["ami-0fedcba987654321f".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "source-image-region",
            vec!["eu-west-1".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "source-instance-id",
            vec!["i-0123456789abcdef0".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        ("sriov-net-support", vec!["advanced".into()], vec![]),
        ("state", vec!["disabled".into()], vec!["app-disabled-image"]),
        ("state", vec!["failed".into()], vec![]),
        (
            "state-reason-code",
            vec!["Client.UserInitiated".into()],
            vec!["app-retired-image"],
        ),
        (
            "state-reason-message",
            vec!["*requested by the owner".into()],
            vec!["app-retired-image"],
        ),
        (
            "tag-key",
            vec!["Owner".into()],
            vec!["app-golden-2026-01-x86_64", "app-golden-2026-01-arm64"],
        ),
        ("tag-key", vec!["Project".into()], vec![]),
        (
            "tag:Owner",
            vec!["TeamA".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        ("tag:Owner", vec!["teama".into()], vec![]),
        (
            "tag:Name",
            vec!["gold*".into()],
            vec!["app-golden-2026-01-x86_64"],
        ),
        (
            "virtualization-type",
            vec!["paravirtual".into()],
            borrowed_with(&paravirtual_catalog, &["legacy-i386-image"]),
        ),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("name", vec![], vec![]),
    ];

    for (name, values, expected_names) in cases {
        let case = format!("{name}={values:?}");
        let value_refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let mut params = include_everything();
        params.extend(filters(&[(name, &value_refs)]));
        let reported = described_ids(&router, &case, &params).await;
        let expected = seeded.ids_for(&expected_names);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected_names:?} ({expected:?}), got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the names of the images that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<&'a str>);

/// Several filters are joined with an AND: only the images matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let al2023_arm64 = catalog_names_where(|spec| {
        spec.name.starts_with("al2023-ami-") && spec.architecture == "arm64"
    });
    let legacy_bios_gp2 = catalog_names_where(|spec| {
        spec.boot_mode.as_deref() == Some("legacy-bios") && spec.root_volume_type == "gp2"
    });

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("name", &["al2023-ami-*"][..]),
                ("architecture", &["arm64"][..]),
            ],
            borrowed(&al2023_arm64),
        ),
        (
            vec![
                ("owner-alias", &["amazon"][..]),
                ("boot-mode", &["legacy-bios"][..]),
                ("block-device-mapping.volume-type", &["gp2"][..]),
            ],
            borrowed(&legacy_bios_gp2),
        ),
        // Each filter matches something, but no image matches both.
        (
            vec![
                ("architecture", &["i386"][..]),
                ("owner-alias", &["amazon"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_names) in cases {
        let case = format!("{entries:?}");
        let mut params = include_everything();
        params.extend(filters(&entries));
        let reported = described_ids(&router, &case, &params).await;
        let expected = seeded.ids_for(&expected_names);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected_names:?} ({expected:?}), got {reported:?}"
        );
    }
}

/// A filter name the operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a defined name, the underscore spelling, a defined name with
    // the wrong case, a filter of another operation, the tag filter without its key
    // separator, the tag filter with a separator but no key, and an empty name.
    let cases = [
        "image-name",
        "owner_id",
        "Architecture",
        "vpc-id",
        "tag",
        "tag:",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Owners and ExecutableUsers
// ---------------------------------------------------------------------------

/// `Owners` scopes the report to the images those owners published, by alias, by account
/// id, or by `self` for the calling account.
#[tokio::test]
async fn owners_scope_the_report_to_the_images_those_owners_published() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let catalog = catalog_names();

    // The account's own images, which `self` and its own account id both name.
    let own = vec![
        "app-golden-2026-01-x86_64",
        "app-golden-2026-01-arm64",
        "legacy-i386-image",
        "app-disabled-image",
        "app-retired-image",
        "app-sunsetting-image",
        "app-windows-image",
    ];
    let mut amazon_and_own: Vec<&str> = borrowed(&catalog);
    amazon_and_own.extend(own.iter().copied());

    let cases: Vec<(Vec<String>, Vec<&str>)> = vec![
        (vec!["amazon".into()], borrowed(&catalog)),
        (vec!["self".into()], own.clone()),
        (vec![account.clone()], own.clone()),
        (vec![PARTNER_ACCOUNT.into()], vec!["partner-public-image"]),
        (
            vec!["aws-marketplace".into()],
            vec!["vendor-appliance-image"],
        ),
        // No image is published under this alias.
        (vec!["aws-backup-vault".into()], vec![]),
        // Several owners: joined with OR.
        (vec!["amazon".into(), "self".into()], amazon_and_own),
        (
            vec![PARTNER_ACCOUNT.into(), "aws-marketplace".into()],
            vec!["partner-public-image", "vendor-appliance-image"],
        ),
        // An account with no images at all.
        (vec!["444455556666".into()], vec![]),
    ];

    for (owners, expected_names) in cases {
        let case = format!("Owners={owners:?}");
        let owner_refs: Vec<&str> = owners.iter().map(String::as_str).collect();
        let mut params = include_everything();
        params.extend(indexed("Owner", &owner_refs));
        let reported = described_ids(&router, &case, &params).await;
        let expected = seeded.ids_for(&expected_names);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected_names:?} ({expected:?}), got {reported:?}"
        );
    }
}

/// `ExecutableUsers` scopes the report to the images the named users may launch: `all` the
/// public ones, an account its own.
#[tokio::test]
async fn executable_users_scope_the_report_to_the_images_those_users_may_launch() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let catalog = catalog_names();

    let own = vec![
        "app-golden-2026-01-x86_64",
        "app-golden-2026-01-arm64",
        "legacy-i386-image",
        "app-disabled-image",
        "app-retired-image",
        "app-sunsetting-image",
        "app-windows-image",
    ];
    // Every image with public launch permissions that this account can see.
    let mut public: Vec<&str> = borrowed(&catalog);
    public.extend([
        "app-golden-2026-01-arm64",
        "partner-public-image",
        "vendor-appliance-image",
    ]);

    let cases: Vec<(Vec<String>, Vec<&str>)> = vec![
        (vec!["all".into()], public.clone()),
        (vec!["self".into()], own.clone()),
        (vec![account.clone()], own.clone()),
        (vec![PARTNER_ACCOUNT.into()], vec!["partner-public-image"]),
        (vec!["444455556666".into()], vec![]),
    ];

    for (users, expected_names) in cases {
        let case = format!("ExecutableUsers={users:?}");
        let user_refs: Vec<&str> = users.iter().map(String::as_str).collect();
        let mut params = include_everything();
        params.extend(indexed("ExecutableBy", &user_refs));
        let reported = described_ids(&router, &case, &params).await;
        let expected = seeded.ids_for(&expected_names);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected_names:?} ({expected:?}), got {reported:?}"
        );
    }
}

/// An `Owners` or `ExecutableUsers` term that names neither an account nor one of the
/// aliases the member documents is refused with `InvalidUserID.Malformed`.
#[tokio::test]
async fn an_owner_or_executable_user_that_names_no_account_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: free text, an account id one digit short and one too long, an id with a
    // non-digit, the empty term, the wrong case of an alias, a term valid only for the
    // OTHER member, and an alias of another service.
    let owner_cases = [
        "amazon-linux",
        "11112222333",
        "1111222233334",
        "11112222333a",
        "",
        "Amazon",
        "all",
        "aws-ec2",
    ];
    for term in owner_cases {
        let (status, body) = describe(&router, &indexed("Owner", &[term])).await;
        assert_eq!(
            status, 400,
            "case Owners={term:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidUserID.Malformed",
            "case Owners={term:?}: expected InvalidUserID.Malformed, got {body}"
        );
        assert!(
            elements(&body, "imageId").is_empty(),
            "case Owners={term:?}: a refused request reports no image, got {body}"
        );
    }

    let executable_cases = ["me", "11112222333", "", "amazon", "aws-marketplace", "All"];
    for term in executable_cases {
        let (status, body) = describe(&router, &indexed("ExecutableBy", &[term])).await;
        assert_eq!(
            status, 400,
            "case ExecutableUsers={term:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidUserID.Malformed",
            "case ExecutableUsers={term:?}: expected InvalidUserID.Malformed, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// ImageIds
// ---------------------------------------------------------------------------

/// `ImageIds` narrows the report to the ids named, in store order whatever order they were
/// asked for.
#[tokio::test]
async fn image_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count, in the order the ids are asked for, and in whether the image
    // named would have been reported without being named (the disabled and the deprecated
    // one would not).
    let cases: Vec<Vec<&str>> = vec![
        vec!["al2023-ami-2023.6.20241121.0-kernel-6.1-x86_64"],
        vec!["app-disabled-image"],
        vec!["app-retired-image"],
        vec![
            "Windows_Server-2022-English-Full-Base-2024.11.13",
            "legacy-i386-image",
        ],
        vec![
            "legacy-i386-image",
            "Windows_Server-2022-English-Full-Base-2024.11.13",
        ],
        vec![
            "partner-public-image",
            "vendor-appliance-image",
            "app-golden-2026-01-arm64",
        ],
    ];
    for asked in cases {
        let ids = seeded.ids_for(&asked);
        let case = format!("ImageIds={asked:?}");
        // Asked for in the order the row names them, which is not the store's order.
        let requested: Vec<String> = asked.iter().map(|name| seeded.id_for(name)).collect();
        let requested_refs: Vec<&str> = requested.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &indexed("ImageId", &requested_refs)).await;
        assert_eq!(
            reported, ids,
            "case {case:?}: expected {ids:?}, got {reported:?}"
        );
    }
}

/// An `ImageIds` entry the account can see no image for is refused: `InvalidAMIID.Malformed`
/// for something that is not an AMI id at all, `InvalidAMIID.NotFound` for a well-formed id
/// no visible image has — even alongside ids that do exist.
#[tokio::test]
async fn an_image_id_that_names_no_visible_image_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.id_for("app-golden-2026-01-x86_64");
    // The partner's private image exists in the store but is not available to this
    // account, so naming its id is as much "no such image" as naming an id nothing has.
    let invisible = seeded.id_for("partner-private-image");

    let not_found: Vec<Vec<String>> = vec![
        vec!["ami-0ffffffffffffffff".to_string()],
        vec!["ami-0abc1234".to_string()],
        vec![invisible.clone()],
        vec![known.clone(), "ami-0ffffffffffffffff".to_string()],
        vec!["ami-0ffffffffffffffff".to_string(), known.clone()],
    ];
    for ids in not_found {
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ImageIds={refs:?}");
        let (status, body) = describe(&router, &indexed("ImageId", &refs)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAMIID.NotFound",
            "case {case:?}: expected InvalidAMIID.NotFound, got {body}"
        );
        assert!(
            elements(&body, "imageId").is_empty(),
            "case {case:?}: a refused request reports no image, got {body}"
        );
    }

    // Rows: another resource's id, an id with a non-hex digit, one with the wrong number
    // of digits, an uppercase suffix, and free text.
    let malformed = [
        "i-0123456789abcdef0",
        "ami-0123456789abcdefg",
        "ami-0123456",
        "ami-0123456789ABCDEF0",
        "not-an-ami",
    ];
    for id in malformed {
        let (status, body) = describe(&router, &indexed("ImageId", &[id])).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAMIID.Malformed",
            "case {id:?}: expected InvalidAMIID.Malformed, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// IncludeDeprecated and IncludeDisabled
// ---------------------------------------------------------------------------

/// A deprecated image — one whose `DeprecationTime` has passed — and a disabled one are
/// left out unless the request asks for them; an image whose deprecation time is still
/// ahead is reported either way.
#[tokio::test]
async fn deprecated_and_disabled_images_are_reported_only_when_asked_for() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let deprecated = seeded.id_for("app-retired-image");
    let also_deprecated = seeded.ids_for(&borrowed(&DEPRECATED_CATALOG_NAMES));
    let disabled = seeded.id_for("app-disabled-image");
    let sunsetting = seeded.id_for("app-sunsetting-image");

    // Rows: the two flags in every combination, and what each must do to the four images
    // whose visibility they govern.
    let cases = [
        (None, None, false, false),
        (Some("false"), Some("false"), false, false),
        (Some("true"), None, true, false),
        (None, Some("true"), false, true),
        (Some("true"), Some("true"), true, true),
    ];
    for (include_deprecated, include_disabled, expect_deprecated, expect_disabled) in cases {
        let mut params = Vec::new();
        if let Some(value) = include_deprecated {
            params.push(param("IncludeDeprecated", value));
        }
        if let Some(value) = include_disabled {
            params.push(param("IncludeDisabled", value));
        }
        let case = format!("IncludeDeprecated={include_deprecated:?} IncludeDisabled={include_disabled:?}");
        let reported = described_ids(&router, &case, &params).await;
        assert_eq!(
            reported.contains(&deprecated),
            expect_deprecated,
            "case {case:?}: the deprecated image {deprecated:?} must be reported only when \
             asked for, got {reported:?}"
        );
        assert_eq!(
            also_deprecated
                .iter()
                .all(|image_id| reported.contains(image_id)),
            expect_deprecated,
            "case {case:?}: the deprecated catalog images {also_deprecated:?} follow the same \
             rule, got {reported:?}"
        );
        assert_eq!(
            reported.contains(&disabled),
            expect_disabled,
            "case {case:?}: the disabled image {disabled:?} must be reported only when asked \
             for, got {reported:?}"
        );
        assert!(
            reported.contains(&sunsetting),
            "case {case:?}: an image whose deprecation time is still ahead is not deprecated, \
             got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every reported image exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_image_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let whole = described_ids(&router, "every image", &include_everything()).await;
    assert!(
        whole.len() > 5,
        "the seeded store must hold enough images to page over, got {whole:?}"
    );

    // Rows: the smallest page, one that does not divide the set, one that divides it, and
    // the largest page the service serves.
    for max_results in [1, 2, 4, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = include_everything();
        params.push(param("MaxResults", max_results.to_string()));
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "imageId");
            assert!(
                page.len() <= max_results,
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = include_everything();
                    params.push(param("MaxResults", max_results.to_string()));
                    params.push(param("NextToken", token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected every image exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the filtered set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    // One kernel's minimal Amazon Linux 2023 builds: a filtered set of more than one page,
    // read off the catalog rather than written out.
    let minimal_kernel_6_1 = catalog_names_where(|spec| {
        spec.name.starts_with("al2023-ami-minimal-") && spec.name.contains("-kernel-6.1-")
    });
    let expected = seeded.ids_for(&borrowed(&minimal_kernel_6_1));
    assert_eq!(
        expected.len(),
        3,
        "the catalog must publish three builds for this page walk to span, got {expected:?}"
    );

    let mut params = filters(&[("name", &["al2023-ami-minimal-*-kernel-6.1-*"])]);
    params.push(param("MaxResults", "2"));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "first page: got {status} with body {body}");
    let first = elements(&body, "imageId");
    assert_eq!(
        first,
        expected[..2].to_vec(),
        "first page: expected the first two minimal images, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));

    let mut next = filters(&[("name", &["al2023-ami-minimal-*-kernel-6.1-*"])]);
    next.push(param("MaxResults", "2"));
    next.push(param("NextToken", token));
    let second = described_ids(&router, "second page", &next).await;
    assert_eq!(
        second,
        expected[2..].to_vec(),
        "second page: expected the remaining minimal image, got {second:?}"
    );
}

/// A `MaxResults` the model leaves unbounded above is SERVED, not refused: the page this
/// service answers is at most the largest it serves, and a size above that asks for that
/// page rather than for a refusal. A page of none is still refused.
#[tokio::test]
async fn max_results_above_the_page_this_service_serves_is_still_served() {
    let (router, engine) = super::ec2_query::emulator();
    let _seeded = seed(&engine);

    // Rows: a page of none (refused), the smallest page, the largest page this service
    // serves, one past it, and a page size far above it — the model declaring no `@range`
    // on `DescribeImagesRequest$MaxResults`, nothing above the ceiling is a refusal.
    let cases = [(0, 400), (1, 200), (1000, 200), (1001, 200), (100_000, 200)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &[param("MaxResults", max_results.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case MaxResults={max_results}: expected InvalidParameterValue, got {body}"
            );
            continue;
        }
        // A page size at or above the ceiling reports every image the store holds, and
        // carries no token: there is nothing left to page to.
        if max_results >= 1000 {
            let reported = elements(&body, "imageId");
            let every = described_ids(&router, "no page size", &[]).await;
            assert_eq!(
                reported, every,
                "case MaxResults={max_results}: expected every visible image {every:?}, got \
                 {reported:?}"
            );
            assert!(
                elements(&body, "nextToken").is_empty(),
                "case MaxResults={max_results}: a page that holds everything carries no \
                 token, got {body}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.id_for("app-golden-2026-01-x86_64");

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, that id hex-encoded, and a forged
    // token carrying the marker but not a matching checksum.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        known.clone(),
        ::hex::encode(&known),
        ::hex::encode(format!("ec2-1:0000000000000000:{known}")),
    ];
    for token in cases {
        let (status, body) = describe(&router, &[param("NextToken", token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "imageId").is_empty(),
        "DryRun=true: a dry run reports no image, got {body}"
    );

    // A dry run of a request that would have been REFUSED is refused with that reason
    // instead: the flag asks whether the request would have succeeded.
    let mut params = indexed("ImageId", &["ami-0ffffffffffffffff"]);
    params.push(param("DryRun", "true"));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "DryRun with an unknown id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidAMIID.NotFound",
        "DryRun with an unknown id: expected InvalidAMIID.NotFound, got {body}"
    );

    let reported = described_ids(&router, "DryRun=false", &[param("DryRun", "false")]).await;
    assert!(
        !reported.is_empty(),
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// What an AMI reports about the volume it restores, and when it is deprecated
// ---------------------------------------------------------------------------

/// The `<ebs>` block of the first block device mapping `body` reports.
fn root_ebs(case: &str, body: &str) -> String {
    let mapping = super::ec2_query::element(body, "blockDeviceMapping").unwrap_or_else(|| {
        panic!("case {case:?}: the image must report its block device mappings, got {body}")
    });
    super::ec2_query::element(&mapping, "ebs").unwrap_or_else(|| {
        panic!("case {case:?}: the root mapping restores an Amazon EBS volume, got {mapping}")
    })
}

/// Rule: an AMI's Amazon EBS block device mapping reports the performance the volume it
/// restores is created with — the figures the ONE volume type AWS documents a default for
/// provisions without being asked, and none for a type whose IOPS are provisioned per volume
/// or follow its size.
#[tokio::test]
async fn an_ami_reports_the_performance_the_root_volume_it_restores_is_created_with() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: the root volume type, which is what the rule generalises over — the type with a
    // documented default, the two types the published catalog's older families carry, and a
    // type whose IOPS are provisioned per volume.
    let rows = [
        ("gp3", Some(("3000", "125"))),
        ("gp2", None),
        ("standard", None),
        ("io2", None),
    ];

    for (volume_type, performance) in rows {
        let case = format!("a root volume of type {volume_type}");
        let image_id = engine.state::<Ec2State>().lock().insert_image(ImageSpec {
            name: format!("app-root-volume-{volume_type}"),
            root_volume_type: volume_type.to_string(),
            ..ImageSpec::default()
        });
        let (status, body) = describe(&router, &indexed("ImageId", &[image_id.as_str()])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let ebs = root_ebs(&case, &body);
        let reported = super::ec2_query::element(&ebs, "volumeType");
        assert_eq!(
            reported.as_deref(),
            Some(volume_type),
            "case {case:?}: the mapping restores a volume of that type, got {reported:?}"
        );
        let reported = (
            super::ec2_query::element(&ebs, "iops"),
            super::ec2_query::element(&ebs, "throughput"),
        );
        match performance {
            Some((iops, throughput)) => assert_eq!(
                (reported.0.as_deref(), reported.1.as_deref()),
                (Some(iops), Some(throughput)),
                "case {case:?}: expected the documented default of {iops} IOPS and \
                 {throughput} MB/s, got {reported:?}"
            ),
            None => assert_eq!(
                (reported.0.as_deref(), reported.1.as_deref()),
                (None, None),
                "case {case:?}: a type with no documented default reports neither figure, \
                 got {reported:?}"
            ),
        }
    }
}

/// Rule: a build of a published family still in standard support reports the deprecation time
/// of that support — an instant still ahead, so the build is reported without
/// `IncludeDeprecated` — beside the IMDS support and the root volume performance a client
/// chooses an AMI on.
#[tokio::test]
async fn a_published_build_of_a_supported_family_reports_when_it_is_deprecated() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);
    let now = std::time::SystemTime::now();

    // Rows: every published build whose family is still in support — read off the catalog
    // rather than written out, so a family the catalog gains later is covered too.
    let supported: Vec<ImageSpec> = published_image_specs()
        .into_iter()
        .filter(|spec| !ec2::state::image_spec_is_deprecated(spec, now))
        .filter(|spec| spec.deprecation_time.is_some())
        .collect();
    assert!(
        !supported.is_empty(),
        "the catalog must publish a build of a family that is still in support"
    );

    for spec in &supported {
        let case = spec.name.clone();
        let (status, body) = describe(&router, &filters(&[("name", &[case.as_str()])])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = elements(&body, "name");
        assert_eq!(
            reported,
            vec![case.clone()],
            "case {case:?}: a build whose deprecation time is still ahead is reported \
             without IncludeDeprecated, got {reported:?}"
        );
        let deprecation_time = super::ec2_query::element(&body, "deprecationTime")
            .unwrap_or_else(|| panic!("case {case:?}: the build reports when, got {body}"));
        let at = super::ec2_query::as_timestamp(&deprecation_time).unwrap_or_else(|| {
            panic!("case {case:?}: a deprecation time reads as an instant, got {deprecation_time:?}")
        });
        let seconds_now = now
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        assert!(
            at > seconds_now,
            "case {case:?}: the support of a family still in support ends ahead of now, got \
             {deprecation_time:?}"
        );
        // The members a client picks an AMI by: IMDSv2 is required by these families, and the
        // root volume it restores reports the performance it is created with.
        let reported = super::ec2_query::element(&body, "imdsSupport");
        assert_eq!(
            reported.as_deref(),
            Some("v2.0"),
            "case {case:?}: the family requires IMDSv2, got {reported:?}"
        );
        let ebs = root_ebs(&case, &body);
        let reported = (
            super::ec2_query::element(&ebs, "iops"),
            super::ec2_query::element(&ebs, "throughput"),
        );
        assert_eq!(
            (reported.0.as_deref(), reported.1.as_deref()),
            (Some("3000"), Some("125")),
            "case {case:?}: the published build's root volume reports the documented gp3 \
             default, got {reported:?}"
        );
    }
}
