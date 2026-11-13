// SPDX-License-Identifier: CC0-1.0

//! `ImageDetail$repositoryType`: the registry tier of the repository an image sits in,
//! reported on EVERY entry `DescribeImages` serves.
//!
//! The rule under test: a described image reports the tier of the repository that holds
//! it, so the member is on every entry of every page whatever the image is and however
//! the request selected it — it is a property of the repository, not of the manifest, of
//! the tag set or of the request. Its value is the one `RepositoryType` is modelled with
//! (`REPOSITORY_TYPE_VALUES`, i.e. the enum's value set from
//! `smithy_query(query="shape", shape="RepositoryType")`), which is the tier a repository
//! created in the account's default public registry is hosted in.
//!
//! Every row of the table below differs along what that rule generalises over: the form
//! of the repository name, how many images the repository holds, whether an image carries
//! tags, whether it is an image manifest or a manifest list, and which of the four ways a
//! caller can select entries (the whole repository, by digest, by tag, one page at a
//! time) asked for them. The boundary is pinned too: `ReferencedImageDetail`, the shape
//! `DescribeImageTags` reports a tag's manifest with, declares no such member
//! (`smithy_query(query="shape", shape="ReferencedImageDetail")`), so that reader must
//! not grow one.
//!
//! Everything goes through the front door a client uses, via the harness in
//! [`crate::repository_image_lifecycle`]: nothing reaches into the state struct.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::ops::types::REPOSITORY_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{given_manifest_blobs, image_manifest, manifest_list};
use crate::repository_image_lifecycle::{digest_of, ok, service, REGISTRY_ID};

/// The tier a repository of this registry is hosted in, stated here rather than read from
/// the service: it is the single value the model's `RepositoryType` enum declares, which
/// [`REPOSITORY_TYPE_VALUES`] carries and the case below cross-checks against.
const EXPECTED_TIER: &str = "FREE";

/// The config-blob size every manifest this suite builds carries. Nothing here asserts on
/// a reported image size, so one size serves every row; the documents themselves are the
/// shared builders' ([`crate::harness::image_manifest`] / [`manifest_list`]).
const CONFIG_SIZE: i64 = 7;

/// One image of a repository under test.
struct Image {
    /// What this row varies: the manifest kind, the layer count and the tag set.
    case: &'static str,
    manifest: String,
    /// The tags this image is pushed under, in push order; empty for an untagged push.
    tags: &'static [&'static str],
    /// The `ImageDetail` members the entry for this image must carry — the tier among
    /// them, and nothing gained or lost beside it.
    members: &'static [&'static str],
}

/// Every `ImageDetail` member an image manifest pushed with at least one tag reports.
const TAGGED_IMAGE_MEMBERS: &[&str] = &[
    "artifactMediaType",
    "imageDigest",
    "imageManifestMediaType",
    "imagePushedAt",
    "imageSizeInBytes",
    "imageTags",
    "registryId",
    "repositoryName",
    "repositoryType",
];

/// The same, for a manifest pushed without a tag: `imageTags` is a member the service has
/// nothing to state.
const UNTAGGED_IMAGE_MEMBERS: &[&str] = &[
    "artifactMediaType",
    "imageDigest",
    "imageManifestMediaType",
    "imagePushedAt",
    "imageSizeInBytes",
    "registryId",
    "repositoryName",
    "repositoryType",
];

/// The same, for a tagged manifest LIST: it references no config blob, so it states no
/// artifact media type.
const TAGGED_LIST_MEMBERS: &[&str] = &[
    "imageDigest",
    "imageManifestMediaType",
    "imagePushedAt",
    "imageSizeInBytes",
    "imageTags",
    "registryId",
    "repositoryName",
    "repositoryType",
];

/// One repository of the table: the `repositoryName` form it exercises and its images.
struct Case {
    case: &'static str,
    repository: &'static str,
    images: Vec<Image>,
}

/// Create one repository.
async fn repository(router: &MultiServiceRouter, name: &str, case: &str) {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": name}),
        case,
    )
    .await;
}

/// Push `manifest` into `repository_name` under each of `tags` (once, untagged, when
/// `tags` is empty), after putting every blob it names into the repository, and answer
/// its digest.
async fn push(
    router: &MultiServiceRouter,
    repository_name: &str,
    manifest: &str,
    tags: &[&str],
    case: &str,
) -> String {
    given_manifest_blobs(router, repository_name, manifest, case).await;
    if tags.is_empty() {
        ok(
            router,
            "PutImage",
            json!({"repositoryName": repository_name, "imageManifest": manifest}),
            case,
        )
        .await;
    }
    for tag in tags {
        ok(
            router,
            "PutImage",
            json!({
                "repositoryName": repository_name,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
            case,
        )
        .await;
    }
    digest_of(manifest)
}

/// The `imageDetails` list of one `DescribeImages` call.
async fn describe(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let body = ok(router, "DescribeImages", request, case).await;
    let details = body["imageDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageDetails list, got {body}"))
        .clone();
    let next = body["nextToken"].as_str().map(ToString::to_string);
    (details, next)
}

/// Assert that `entry` reports the repository's tier, and carries exactly `members`.
fn assert_entry(entry: &Value, repository: &str, members: &[&str], case: &str) {
    let actual = entry["repositoryType"].as_str();
    assert_eq!(
        actual,
        Some(EXPECTED_TIER),
        "case {case:?}: expected repositoryType {EXPECTED_TIER:?} on the entry, got {entry}"
    );
    assert!(
        REPOSITORY_TYPE_VALUES.contains(&EXPECTED_TIER),
        "case {case:?}: expected {EXPECTED_TIER:?} to be a modelled RepositoryType value, \
         got {REPOSITORY_TYPE_VALUES:?}"
    );
    assert_eq!(
        entry["repositoryName"].as_str(),
        Some(repository),
        "case {case:?}: expected the entry to name its repository, got {entry}"
    );
    assert_eq!(
        entry["registryId"].as_str(),
        Some(REGISTRY_ID),
        "case {case:?}: expected the entry to name this registry, got {entry}"
    );
    let reported: BTreeSet<&str> = entry
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ImageDetail object, got {entry}"))
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = members.iter().copied().collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected exactly the members {expected:?}, got {reported:?} in {entry}"
    );
}

/// The table: three repository-name forms, holding one to three images between them, with
/// tagged, multiply-tagged and untagged manifests and a manifest list.
fn cases() -> Vec<Case> {
    let child_one = image_manifest("tier-child-one", CONFIG_SIZE, &[11]);
    let child_two = image_manifest("tier-child-two", CONFIG_SIZE, &[12, 13]);
    let children = [digest_of(&child_one), digest_of(&child_two)];
    let index = manifest_list(&[children[0].as_str(), children[1].as_str()]);

    vec![
        Case {
            case: "the shortest name the pattern admits, one tagged image",
            // `repositoryName` is modelled `@length(2..205)`.
            repository: "ab",
            images: vec![Image {
                case: "one layer, one tag",
                manifest: image_manifest("tier-single", CONFIG_SIZE, &[209]),
                tags: &["tag-primary"],
                members: TAGGED_IMAGE_MEMBERS,
            }],
        },
        Case {
            case: "a namespaced name with every separator, three images",
            repository: "team/app.v1_beta-2",
            images: vec![
                Image {
                    case: "no layers at all, two tags",
                    manifest: image_manifest("tier-no-layers", CONFIG_SIZE, &[]),
                    tags: &["first", "second"],
                    members: TAGGED_IMAGE_MEMBERS,
                },
                Image {
                    case: "many layers, untagged",
                    manifest: image_manifest("tier-many-layers", CONFIG_SIZE, &[1, 2, 3, 4, 5]),
                    tags: &[],
                    members: UNTAGGED_IMAGE_MEMBERS,
                },
                Image {
                    case: "a layer past the 32-bit range, one tag",
                    manifest: image_manifest("tier-big-layer", CONFIG_SIZE, &[4_294_967_296]),
                    tags: &["big"],
                    members: TAGGED_IMAGE_MEMBERS,
                },
            ],
        },
        Case {
            case: "a manifest list beside the images it references",
            repository: "lists",
            images: vec![
                Image {
                    case: "the first child",
                    manifest: child_one,
                    tags: &["child-one"],
                    members: TAGGED_IMAGE_MEMBERS,
                },
                Image {
                    case: "the second child",
                    manifest: child_two,
                    tags: &["child-two"],
                    members: TAGGED_IMAGE_MEMBERS,
                },
                Image {
                    case: "the list itself, which references no config blob",
                    manifest: index,
                    tags: &["index"],
                    members: TAGGED_LIST_MEMBERS,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn every_described_image_reports_the_tier_of_the_repository_that_holds_it() {
    let router = service();

    for case in cases() {
        repository(&router, case.repository, case.case).await;
        let mut digests = Vec::new();
        for image in &case.images {
            digests.push(
                push(
                    &router,
                    case.repository,
                    &image.manifest,
                    image.tags,
                    image.case,
                )
                .await,
            );
        }

        // 1. The whole repository: one entry per stored manifest, each reporting the tier.
        let whole = format!("{}: the whole repository", case.case);
        let (details, next) = describe(
            &router,
            json!({"repositoryName": case.repository}),
            &whole,
        )
        .await;
        assert_eq!(
            details.len(),
            case.images.len(),
            "case {whole:?}: expected one entry per pushed manifest, got {details:?}"
        );
        for (image, digest) in case.images.iter().zip(&digests) {
            let entry = details
                .iter()
                .find(|entry| entry["imageDigest"].as_str() == Some(digest.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {whole:?}: expected an entry for {digest}, got {details:?}")
                });
            assert_entry(
                entry,
                case.repository,
                image.members,
                &format!("{whole}, {}", image.case),
            );
        }

        // 2. Selected by digest, every manifest named in one request.
        let by_digest = format!("{}: selected by digest", case.case);
        let (selected, _) = describe(
            &router,
            json!({
                "repositoryName": case.repository,
                "imageIds": digests
                    .iter()
                    .map(|digest| json!({"imageDigest": digest}))
                    .collect::<Vec<Value>>(),
            }),
            &by_digest,
        )
        .await;
        assert_eq!(
            selected.len(),
            case.images.len(),
            "case {by_digest:?}: expected one entry per named digest, got {selected:?}"
        );
        for entry in &selected {
            let digest = entry["imageDigest"].as_str().unwrap_or_default().to_string();
            let position = digests
                .iter()
                .position(|pushed| *pushed == digest)
                .unwrap_or_else(|| {
                    panic!("case {by_digest:?}: unexpected digest {digest} in {selected:?}")
                });
            assert_entry(
                entry,
                case.repository,
                case.images[position].members,
                &format!("{by_digest}, {}", case.images[position].case),
            );
        }

        // 3. Selected by tag: the tier is the repository's, so a tag-keyed selection
        //    reports the same value as a digest-keyed one.
        for image in &case.images {
            for tag in image.tags {
                let by_tag = format!("{}: selected by tag {tag:?}", case.case);
                let (tagged, _) = describe(
                    &router,
                    json!({
                        "repositoryName": case.repository,
                        "imageIds": [{"imageTag": tag}],
                    }),
                    &by_tag,
                )
                .await;
                assert_eq!(
                    tagged.len(),
                    1,
                    "case {by_tag:?}: expected the one tagged image, got {tagged:?}"
                );
                assert_entry(&tagged[0], case.repository, image.members, &by_tag);
            }
        }

        // 4. One entry per page: the member is on every page, not only the first.
        let paged = format!("{}: one entry per page", case.case);
        let mut token: Option<String> = None;
        let mut seen = 0usize;
        for _ in 0..=case.images.len() {
            let mut request = json!({"repositoryName": case.repository, "maxResults": 1});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (page, next_token) = describe(&router, request, &paged).await;
            assert!(
                page.len() <= 1,
                "case {paged:?}: expected at most one entry per page, got {page:?}"
            );
            for entry in &page {
                let digest = entry["imageDigest"].as_str().unwrap_or_default().to_string();
                let position = digests
                    .iter()
                    .position(|pushed| *pushed == digest)
                    .unwrap_or_else(|| {
                        panic!("case {paged:?}: unexpected digest {digest} in {page:?}")
                    });
                assert_entry(
                    entry,
                    case.repository,
                    case.images[position].members,
                    &format!("{paged}, {}", case.images[position].case),
                );
                seen += 1;
            }
            token = next_token;
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {paged:?}: expected the pages to run out, got a further token {token:?}"
        );
        assert_eq!(
            seen,
            case.images.len(),
            "case {paged:?}: expected every image to be paged through exactly once"
        );

        // 5. A read states the tier without changing it: the second read of the whole
        //    repository answers exactly what the first did.
        let (again, _) = describe(
            &router,
            json!({"repositoryName": case.repository}),
            &format!("{whole}, read again"),
        )
        .await;
        assert_eq!(
            again, details,
            "case {whole:?}: expected two reads to report the same entries, got {again:?} \
             after {details:?}"
        );
        assert!(
            next.is_none(),
            "case {whole:?}: expected a page holding every image to carry no token, got \
             {next:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_entries_of_describe_image_tags_report_no_repository_tier() {
    // `ReferencedImageDetail` — the shape `DescribeImageTags` reports a tag's manifest
    // with — declares imageDigest, imageSizeInBytes, imagePushedAt,
    // imageManifestMediaType and artifactMediaType and no member for the tier, so this
    // reader states nothing about it.
    const REFERENCED_MEMBERS: &[&str] = &[
        "artifactMediaType",
        "imageDigest",
        "imageManifestMediaType",
        "imagePushedAt",
        "imageSizeInBytes",
    ];

    let router = service();
    let case = "a tagged image, read through DescribeImageTags";
    repository(&router, "tag-entries", case).await;
    let manifest = image_manifest("tier-tag-entry", CONFIG_SIZE, &[42]);
    push(&router, "tag-entries", &manifest, &["only"], case).await;

    let body = ok(
        &router,
        "DescribeImageTags",
        json!({"repositoryName": "tag-entries"}),
        case,
    )
    .await;
    let entries = body["imageTagDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageTagDetails list, got {body}"))
        .clone();
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected the one tag entry, got {entries:?}"
    );
    let detail = &entries[0]["imageDetail"];
    let reported: BTreeSet<&str> = detail
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ReferencedImageDetail, got {detail}"))
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = REFERENCED_MEMBERS.iter().copied().collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected exactly the members {expected:?}, got {reported:?} in {detail}"
    );
}
