// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppStream's image catalog read over the wire.
//!
//! Every case drives the real awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness, so what is exercised is
//! the dispatch → deserialize → handle → serialize walk.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * `DescribeImages` reports the images this account can see from the stored catalog —
//!   the base images AWS publishes — so a client can enumerate them and pick one for a
//!   fleet or an image builder;
//! * every image reports the ARN `image/<name>` in the Region the request was signed for,
//!   under this emulator's account, and a platform, state, visibility and instance
//!   families the model admits;
//! * a request naming images reports exactly those, whether it names them by name or by
//!   ARN, and a name or ARN addressing none answers `ResourceNotFoundException`;
//! * naming both the names and the ARNs is an incorrect combination of parameters;
//! * `Type` narrows the report to the images of that visibility;
//! * a walk of the catalog reports every image exactly once, whatever page size is asked
//!   for, and the read decides its page size through the one shared rule;
//! * the read changes nothing: two identical reads answer identically.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::refusal_messages::{not_found_sentence, refusal};
use crate::stack_fleet_lifecycle::Emulator;

/// The account every emulator ARN and every refusal sentence names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The resource words AWS's refusal sentences use for the resources these rows address —
/// the sentence names the kind the ARN's `resource-type` segment carries.
const IMAGE_KIND: &str = "image";
const FLEET_KIND: &str = "fleet";
const UNTYPED_KIND: &str = "resource";

/// The `VisibilityType` values the model admits (`VISIBILITY_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`, from `smithy_query(query="shape",
/// shape="VisibilityType")`: PRIVATE | PUBLIC | SHARED).
const PUBLIC: &str = "PUBLIC";
const PRIVATE: &str = "PRIVATE";
const SHARED: &str = "SHARED";

/// The `ImageState` of an image that can be launched from (`IMAGE_STATE_VALUES` in
/// `src/ops/types/shared_types.rs`).
const AVAILABLE: &str = "AVAILABLE";

/// The `ImageType` of an AWS-published image (`IMAGE_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`: BYOL | CUSTOM | NATIVE).
const NATIVE: &str = "NATIVE";

/// Every operating system this API models an image platform for
/// (`smithy_query(query="shape", shape="PlatformType")`, generated as
/// `PLATFORM_TYPE_VALUES` in `src/ops/types/shared_types.rs`).
const PLATFORM_TYPES: &[&str] = &[
    "AMAZON_LINUX2",
    "RHEL8",
    "ROCKY_LINUX8",
    "UBUNTU_PRO_2404",
    "WINDOWS",
    "WINDOWS_SERVER_2016",
    "WINDOWS_SERVER_2019",
    "WINDOWS_SERVER_2022",
    "WINDOWS_SERVER_2025",
];

/// The instance families an image may state support for, as the model's
/// `Image$SupportedInstanceFamilies` doc enumerates them
/// (`smithy_query(query="doc", shape="Image", member="SupportedInstanceFamilies")`).
const INSTANCE_FAMILIES: &[&str] = &[
    "General Purpose",
    "Compute Optimized",
    "Memory Optimized",
    "Graphics G4",
    "Graphics G5",
    "Graphics G6",
];

/// The largest page this read answers with: `DescribeImages`'s `MaxResults` targets
/// `DescribeImagesMaxResults`, which the model bounds at 25
/// (`smithy_query(query="operation", operation="DescribeImages")`: `range 0..25`).
const MAX_PAGE: usize = 25;

/// The ARN an image of this name carries: AppStream's `image` resource type, in the
/// Region the request was signed for.
fn image_arn(region: &str, name: &str) -> String {
    format!("arn:aws:appstream:{region}:{ACCOUNT}:image/{name}")
}

/// One `DescribeImages` call that must succeed: its `Images` and its `NextToken`.
async fn describe(emulator: &Emulator, case: &str, request: Value) -> (Vec<Value>, Option<Value>) {
    let payload = emulator.ok(case, "DescribeImages", request).await;
    let images = payload
        .get("Images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeImages must report Images, got {payload}"));
    (images, payload.get("NextToken").cloned())
}

/// The `Name` of each reported image, in the order they were reported.
fn reported_names(images: &[Value]) -> Vec<String> {
    images
        .iter()
        .map(|image| {
            image["Name"]
                .as_str()
                .unwrap_or("<no Name>")
                .to_string()
        })
        .collect()
}

/// Every image the catalog holds, read through the front door with no filters.
async fn catalog(emulator: &Emulator) -> Vec<Value> {
    let (images, next_token) = describe(emulator, "the whole catalog", json!({})).await;
    assert!(
        next_token.is_none(),
        "case \"the whole catalog\": the shipped catalog fits one page, got a NextToken"
    );
    images
}

#[tokio::test]
async fn the_catalog_is_reported_with_the_members_a_client_picks_an_image_by() {
    // Rows: the Region the request is signed for. The catalog is the same in each, and
    // every image reports that Region's own ARN.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let images = catalog(&emulator).await;
        assert!(
            images.len() >= 2,
            "case {region:?}: the account must see the published base images, got {}",
            images.len()
        );

        for image in &images {
            let name = image["Name"].as_str().unwrap_or("<no Name>").to_string();
            let case = format!("{region} {name}");

            let expected_arn = image_arn(region, &name);
            assert_eq!(
                image["Arn"].as_str().unwrap_or_default(),
                expected_arn,
                "case {case:?}: expected Arn {expected_arn:?}, got {image}"
            );
            assert_eq!(
                image["State"].as_str().unwrap_or_default(),
                AVAILABLE,
                "case {case:?}: a published image is launchable, got {image}"
            );
            assert_eq!(
                image["Visibility"].as_str().unwrap_or_default(),
                PUBLIC,
                "case {case:?}: an AWS-published image is public, got {image}"
            );
            assert_eq!(
                image["ImageBuilderSupported"].as_bool(),
                Some(true),
                "case {case:?}: an image builder launches from a base image, got {image}"
            );
            assert_eq!(
                image["ImageType"].as_str().unwrap_or_default(),
                NATIVE,
                "case {case:?}: an AWS-provided image's type is native, got {image}"
            );

            let platform = image["Platform"].as_str().unwrap_or_default();
            assert!(
                PLATFORM_TYPES.contains(&platform),
                "case {case:?}: expected a modelled platform, got {platform:?}"
            );

            let families = image["SupportedInstanceFamilies"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !families.is_empty(),
                "case {case:?}: an image states the instance families it runs on, got {image}"
            );
            for family in &families {
                let family = family.as_str().unwrap_or_default();
                assert!(
                    INSTANCE_FAMILIES.contains(&family),
                    "case {case:?}: expected a documented instance family, got {family:?}"
                );
            }

            for member in ["DisplayName", "Description"] {
                let text = image[member].as_str().unwrap_or_default();
                assert!(
                    !text.is_empty(),
                    "case {case:?}: {member} must describe the image, got {image}"
                );
            }
            for member in ["CreatedTime", "PublicBaseImageReleasedDate"] {
                let instant = image[member].as_f64().unwrap_or_default();
                assert!(
                    instant > 0.0,
                    "case {case:?}: {member} must carry the day the image was released, \
                     got {image}"
                );
            }

            // An account's own activity fills these; nothing produced a published image
            // here and nothing has gone wrong with it, so they are absent rather than
            // present-and-empty.
            for member in ["ImageBuilderName", "BaseImageArn", "ImageErrors", "Applications"] {
                assert!(
                    image.get(member).is_none(),
                    "case {case:?}: {member} belongs to an account's own image, got {image}"
                );
            }

            // The name is one a fleet or image builder can be launched with: `ImageName`'s
            // pattern is `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`
            // (`smithy_query(query="operation", operation="CreateFleet")`).
            assert!(
                (1..=101).contains(&name.len())
                    && name.starts_with(|first: char| first.is_ascii_alphanumeric())
                    && name
                        .chars()
                        .all(|letter| letter.is_ascii_alphanumeric()
                            || ['_', '.', '-'].contains(&letter)),
                "case {case:?}: the name must be one a fleet can name, got {name:?}"
            );
        }

        let names = reported_names(&images);
        let unique: BTreeSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "case {region:?}: no image may be reported twice, got {names:?}"
        );
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "case {region:?}: a page walks the catalog in a stable order, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_read_reports_exactly_the_images_the_request_names() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let all: Vec<String> = reported_names(&catalog(&emulator).await);

    // Rows: how many images the request names and in what form — one, two, the whole
    // catalog, the same name twice, and the names in reverse order. The answer is the
    // images named, each once, in the catalog's stable order.
    let mut selections: Vec<Vec<String>> = vec![
        vec![all[0].clone()],
        vec![all[all.len() - 1].clone()],
        vec![all[0].clone(), all[1].clone()],
        vec![all[1].clone(), all[0].clone()],
        vec![all[0].clone(), all[0].clone()],
        all.clone(),
    ];
    selections.push(all.iter().rev().cloned().collect());

    for selection in selections {
        let expected: Vec<String> = {
            let wanted: BTreeSet<&String> = selection.iter().collect();
            all.iter()
                .filter(|name| wanted.contains(name))
                .cloned()
                .collect()
        };

        let case = format!("Names {selection:?}");
        let (images, _) = describe(&emulator, &case, json!({ "Names": selection })).await;
        assert_eq!(
            reported_names(&images),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported_names(&images)
        );

        let arns: Vec<String> = selection
            .iter()
            .map(|name| image_arn(region, name))
            .collect();
        let case = format!("Arns {arns:?}");
        let (images, _) = describe(&emulator, &case, json!({ "Arns": arns })).await;
        assert_eq!(
            reported_names(&images),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported_names(&images)
        );
    }
}

#[tokio::test]
async fn a_name_or_arn_addressing_no_image_is_refused_with_awss_sentence() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let stored = reported_names(&catalog(&emulator).await)[0].clone();

    // Rows: names that address no image, spanning length, character classes and case. The
    // last is the longest name the `Name` pattern admits (101 characters).
    for name in [
        "absent".to_string(),
        "Mixed.Case_name-9".to_string(),
        "0123456789".to_string(),
        stored.to_lowercase(),
        format!("z{}", "9".repeat(100)),
    ] {
        if name == stored {
            continue;
        }
        let case = format!("Names [{name:?}]");
        let message = refusal(
            &emulator,
            &case,
            "DescribeImages",
            json!({ "Names": [name.clone()] }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_sentence(IMAGE_KIND, &name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }

    // Rows: ARNs that address no image of this account in this Region — an image name
    // nothing holds, this Region's image named in ANOTHER Region, another account, another
    // resource type, and an ARN whose resource part names no type at all. Each is refused
    // with the kind its own ARN carries.
    let rows: Vec<(String, &str, String)> = vec![
        (
            image_arn(region, "absent"),
            IMAGE_KIND,
            "absent".to_string(),
        ),
        (
            image_arn("us-west-2", &stored),
            IMAGE_KIND,
            stored.clone(),
        ),
        (
            format!("arn:aws:appstream:{region}:999999999999:image/{stored}"),
            IMAGE_KIND,
            stored.clone(),
        ),
        (
            format!("arn:aws:appstream:{region}:{ACCOUNT}:fleet/{stored}"),
            FLEET_KIND,
            stored.clone(),
        ),
        (
            format!("arn:aws:appstream:{region}:{ACCOUNT}:{stored}"),
            UNTYPED_KIND,
            stored.clone(),
        ),
    ];

    for (arn, kind, named) in rows {
        let case = format!("Arns [{arn:?}]");
        let message = refusal(
            &emulator,
            &case,
            "DescribeImages",
            json!({ "Arns": [arn.clone()] }),
            400,
            "ResourceNotFoundException",
        )
        .await;
        let expected = not_found_sentence(kind, &named);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn naming_both_the_names_and_the_arns_is_an_incorrect_parameter_combination() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let all = reported_names(&catalog(&emulator).await);

    // Rows: both members carried, with and without a `Type`, and whether the two address
    // the same image or different ones — the combination is refused however it is filled.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the same image by name and by ARN",
            json!({ "Names": [all[0].clone()], "Arns": [image_arn(region, &all[0])] }),
        ),
        (
            "different images by name and by ARN",
            json!({ "Names": [all[0].clone()], "Arns": [image_arn(region, &all[1])] }),
        ),
        (
            "both, narrowed by Type",
            json!({
                "Names": [all[0].clone()],
                "Arns": [image_arn(region, &all[1])],
                "Type": PUBLIC,
            }),
        ),
        (
            "a name and an ARN that address nothing",
            json!({ "Names": ["absent"], "Arns": [image_arn(region, "absent")] }),
        ),
    ];

    for (case, request) in rows {
        let message = refusal(
            &emulator,
            case,
            "DescribeImages",
            request,
            400,
            "InvalidParameterCombinationException",
        )
        .await;
        let expected = "You must specify either the image names or the image ARNs, not both.";
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }

    // An empty list on one side names no image, so it is not a second selector: the
    // request is answered from the side that does name images.
    let (images, _) = describe(
        &emulator,
        "an empty Arns list beside Names",
        json!({ "Names": [all[0].clone()], "Arns": [] }),
    )
    .await;
    assert_eq!(
        reported_names(&images),
        vec![all[0].clone()],
        "case \"an empty Arns list beside Names\": expected the named image, got {images:?}"
    );
}

#[tokio::test]
async fn the_type_member_narrows_the_report_to_one_visibility() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let all = reported_names(&catalog(&emulator).await);

    // Rows: each visibility the model admits, asked for on its own and beside a `Names`
    // selector. The published catalog is public, so only that visibility reports anything.
    for visibility in [PUBLIC, PRIVATE, SHARED] {
        let expected: Vec<String> = if visibility == PUBLIC {
            all.clone()
        } else {
            Vec::new()
        };

        let case = format!("Type {visibility}");
        let (images, _) = describe(&emulator, &case, json!({ "Type": visibility })).await;
        assert_eq!(
            reported_names(&images),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported_names(&images)
        );

        let case = format!("Type {visibility} with Names");
        let (images, _) = describe(
            &emulator,
            &case,
            json!({ "Names": [all[0].clone()], "Type": visibility }),
        )
        .await;
        let expected = if visibility == PUBLIC {
            vec![all[0].clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported_names(&images),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported_names(&images)
        );
    }
}

#[tokio::test]
async fn a_walk_of_the_catalog_reports_every_image_exactly_once() {
    let emulator = Emulator::in_region("us-east-1");
    let all: BTreeSet<String> = reported_names(&catalog(&emulator).await)
        .into_iter()
        .collect();

    // Rows: every page size from the smallest a read can answer with up to the largest the
    // model admits. Each walk must report every image exactly once.
    for page_size in [1_usize, 2, 3, 5, 8, MAX_PAGE] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<Value> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = next_token.clone() {
                request["NextToken"] = token;
            }
            let (page, token) = describe(&emulator, &case, request).await;
            let ceiling = page_size.min(MAX_PAGE);
            assert!(
                !page.is_empty() && page.len() <= ceiling,
                "case {case:?}: a page holds between one and {ceiling} images, got {}",
                page.len()
            );
            seen.extend(reported_names(&page));
            next_token = token;
            if next_token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: no image may be reported twice, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all,
            "case {case:?}: the walk must report every image, got {seen:?}"
        );
    }

    // Each published bound of the page size, and one past it: the model bounds
    // `MaxResults` at `range 0..25` (`DescribeImagesMaxResults`). The top bound is
    // answered; a page of fewer than one record is not a size this read can answer with,
    // and it states the shared page-size rule the way every other read of this service
    // does; one past the top is refused with the sentence that states the bound.
    let case = format!("MaxResults {MAX_PAGE}");
    let (page, _) = describe(&emulator, &case, json!({ "MaxResults": MAX_PAGE })).await;
    assert!(
        page.len() <= MAX_PAGE,
        "case {case:?}: a page holds at most {MAX_PAGE} images, got {}",
        page.len()
    );

    let message = refusal(
        &emulator,
        "MaxResults 0",
        "DescribeImages",
        json!({ "MaxResults": 0 }),
        400,
        "ValidationException",
    )
    .await;
    assert_eq!(
        message, "MaxResults must be at least 1, was 0",
        "case \"MaxResults 0\": expected the shared page-size sentence, got {message:?}"
    );

    let message = refusal(
        &emulator,
        "MaxResults past the ceiling",
        "DescribeImages",
        json!({ "MaxResults": MAX_PAGE + 1 }),
        400,
        "ValidationException",
    )
    .await;
    assert!(
        message.contains(&MAX_PAGE.to_string()),
        "case \"MaxResults past the ceiling\": the refusal must state the bound {MAX_PAGE}, \
         got {message:?}"
    );

    // A token this service did not mint resumes nothing.
    for token in ["not-a-token", "zz", "7a7"] {
        let case = format!("NextToken {token:?}");
        refusal(
            &emulator,
            &case,
            "DescribeImages",
            json!({ "NextToken": token }),
            400,
            "ValidationException",
        )
        .await;
    }
}

#[tokio::test]
async fn the_read_changes_nothing_a_later_caller_sees() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let all = reported_names(&catalog(&emulator).await);

    // Rows: the forms of this read. Each is asked twice, with the other forms in between,
    // and must answer identically both times.
    let requests: Vec<(&str, Value)> = vec![
        ("no filters", json!({})),
        ("by name", json!({ "Names": [all[0].clone()] })),
        ("by ARN", json!({ "Arns": [image_arn(region, &all[1])] })),
        ("by visibility", json!({ "Type": PUBLIC })),
        ("one image at a time", json!({ "MaxResults": 1 })),
    ];

    let first: Vec<Value> = {
        let mut answers = Vec::new();
        for (case, request) in &requests {
            answers.push(emulator.ok(case, "DescribeImages", request.clone()).await);
        }
        answers
    };
    for (index, (case, request)) in requests.iter().enumerate() {
        let again = emulator.ok(case, "DescribeImages", request.clone()).await;
        assert_eq!(
            again, first[index],
            "case {case:?}: a read must answer identically, got {again} then {}",
            first[index]
        );
    }
}

#[tokio::test]
async fn a_published_image_can_be_launched_by_a_fleet() {
    let region = "us-east-1";
    let emulator = Emulator::in_region(region);
    let images = catalog(&emulator).await;

    // Rows: the published images a client might pick, each named to a fleet by name and by
    // ARN — the read is what lets a client choose one, so what it reports must be what a
    // fleet can be created with and read back.
    for (index, image) in images.iter().enumerate().take(3) {
        let name = image["Name"].as_str().unwrap_or_default().to_string();
        let arn = image["Arn"].as_str().unwrap_or_default().to_string();

        let case = format!("fleet on image {name:?} by name");
        let fleet_name = format!("fleet-by-name-{index}");
        let created = emulator
            .ok(
                &case,
                "CreateFleet",
                json!({
                    "Name": fleet_name,
                    "InstanceType": "stream.standard.small",
                    "ImageName": name,
                    "ComputeCapacity": { "DesiredInstances": 1 },
                }),
            )
            .await;
        assert_eq!(
            created["Fleet"]["ImageName"].as_str().unwrap_or_default(),
            name,
            "case {case:?}: the fleet must report the image it was created on, got {created}"
        );
        let read = emulator
            .ok(&case, "DescribeFleets", json!({ "Names": [fleet_name] }))
            .await;
        assert_eq!(
            read["Fleets"][0]["ImageName"].as_str().unwrap_or_default(),
            name,
            "case {case:?}: a later read must report the same image, got {read}"
        );

        let case = format!("fleet on image {name:?} by ARN");
        let fleet_name = format!("fleet-by-arn-{index}");
        emulator
            .ok(
                &case,
                "CreateFleet",
                json!({
                    "Name": fleet_name,
                    "InstanceType": "stream.standard.small",
                    "ImageArn": arn,
                    "ComputeCapacity": { "DesiredInstances": 1 },
                }),
            )
            .await;
        let read = emulator
            .ok(&case, "DescribeFleets", json!({ "Names": [fleet_name] }))
            .await;
        assert_eq!(
            read["Fleets"][0]["ImageArn"].as_str().unwrap_or_default(),
            arn,
            "case {case:?}: a later read must report the same image ARN, got {read}"
        );
    }
}
