// SPDX-License-Identifier: CC0-1.0

//! `DescribeImageTags`: the image TAGS a repository holds, driven end to end over the
//! wire through the harness in [`crate::repository_image_lifecycle`].
//!
//! The rule every case here asserts: a paginated tag reader reports exactly one entry per
//! tag the repository currently holds — in tag order, each entry naming the tag, when the
//! push that wrote it created it, and the stored manifest that tag points at — so a
//! manifest carrying several tags appears once per tag, an untagged manifest does not
//! appear at all, and the reader itself changes nothing. Source: the model, via
//! `smithy_query(query="shape", shape="ImageTagDetail,ReferencedImageDetail")` —
//! `imageTag` is "the tag that's associated with the image", `createdAt` is "the time
//! stamp that indicates when the image tag was created", and `imageDetail` is "an object
//! that describes the details of an image" whose members are the manifest-describing half
//! of `ImageDetail` (`imageDigest`, `imageSizeInBytes`, `imagePushedAt`,
//! `imageManifestMediaType`, `artifactMediaType`).
//!
//! Each table row differs along what the rule generalises over: how many tags a manifest
//! carries, how many manifests a repository holds, the form and length of a tag (both
//! published `ImageTag` bounds), the manifest shape the tag points at, and the page size
//! the tag set is walked at.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{given_manifest_blobs, manifest_list};
use crate::repository_image_lifecycle::{digest_of, ok, refused, service, REGISTRY_ID};

/// Create one repository, named for the case.
async fn repository(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": name}),
        name,
    )
    .await;
}

/// Push `manifest` under `repository_name`, optionally tagged, and return its digest.
///
/// Every layer blob the manifest names is put into the repository first: a manifest is
/// only committed over blobs the repository already holds.
async fn push(
    router: &MultiServiceRouter,
    repository_name: &str,
    manifest: &str,
    tag: Option<&str>,
    case: &str,
) -> String {
    given_manifest_blobs(router, repository_name, manifest, case).await;
    let mut body = json!({"repositoryName": repository_name, "imageManifest": manifest});
    if let Some(tag) = tag {
        body["imageTag"] = json!(tag);
    }
    ok(router, "PutImage", body, case).await;
    digest_of(manifest)
}

/// Every `imageTagDetails` entry of `repository_name`, walked to exhaustion at
/// `max_results` (unpaged when `None`), in the order the pages reported them.
async fn tag_details(
    router: &MultiServiceRouter,
    repository_name: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut body = json!({"repositoryName": repository_name});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let reply = ok(router, "DescribeImageTags", body, case).await;
        let page = reply["imageTagDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an imageTagDetails array, {reply}"))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                page.len() as i64 <= max_results,
                "case {case:?}: expected at most {max_results} entries on a page, got {}",
                page.len()
            );
        }
        entries.extend(page);
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match reply["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    entries
}

/// The tags an entry list names, in the order it reported them.
fn tags_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["imageTag"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The one `imageDetails` entry `DescribeImages` reports for `digest`.
async fn image_detail_of(
    router: &MultiServiceRouter,
    repository_name: &str,
    digest: &str,
    case: &str,
) -> Value {
    let described = ok(
        router,
        "DescribeImages",
        json!({
            "repositoryName": repository_name,
            "imageIds": [{"imageDigest": digest}],
        }),
        case,
    )
    .await;
    described["imageDetails"][0].clone()
}

/// The members `ReferencedImageDetail` shares with `ImageDetail`: what the two readers
/// must agree on for one stored manifest.
const SHARED_IMAGE_MEMBERS: [&str; 5] = [
    "imageDigest",
    "imageSizeInBytes",
    "imagePushedAt",
    "imageManifestMediaType",
    "artifactMediaType",
];

/// The config-blob size every manifest this suite builds carries: an image's reported size
/// is the total of the blobs its descriptors name, so every row's expectation carries that
/// config blob too. The document itself is the shared builder's
/// ([`crate::harness::image_manifest`]).
const CONFIG_SIZE: i64 = 9_999;

/// An image manifest referencing `layer_sizes` layer blobs, over this suite's config blob.
fn image_manifest(name: &str, layer_sizes: &[i64]) -> String {
    crate::harness::image_manifest(name, CONFIG_SIZE, layer_sizes)
}

/// A manifest document that references neither layers nor child manifests, so nothing
/// stored states a size for it.
fn opaque_manifest(name: &str) -> String {
    json!({"schemaVersion": 2, "annotations": {"name": name}}).to_string()
}

/// One repository's tag set: the tags each pushed manifest carries, in push order.
struct TagSetCase {
    case: &'static str,
    repository: &'static str,
    images: Vec<Vec<String>>,
}

/// Every tag-set shape the reader generalises over, including both published `ImageTag`
/// length bounds (len 1..300, from `smithy_query(query="shape", shape="ImageTag")`) and
/// tag forms the model's unconstrained-pattern `ImageTag` admits.
fn tag_set_cases() -> Vec<TagSetCase> {
    vec![
        TagSetCase {
            case: "no image at all",
            repository: "team/none",
            images: vec![],
        },
        TagSetCase {
            case: "one untagged manifest",
            repository: "team/untagged",
            images: vec![vec![]],
        },
        TagSetCase {
            case: "one manifest, one tag",
            repository: "team/single",
            images: vec![vec!["latest".to_string()]],
        },
        TagSetCase {
            case: "one manifest, three tags",
            repository: "team/multitag",
            images: vec![vec![
                "latest".to_string(),
                "v1".to_string(),
                "v1.0.0".to_string(),
            ]],
        },
        TagSetCase {
            case: "four manifests, one tag each",
            repository: "team/fanout",
            images: vec![
                vec!["a".to_string()],
                vec!["b".to_string()],
                vec!["c".to_string()],
                vec!["d".to_string()],
            ],
        },
        TagSetCase {
            case: "tagged and untagged manifests side by side",
            repository: "team/mixed",
            images: vec![
                vec!["alpha".to_string()],
                vec![],
                vec!["beta".to_string(), "gamma".to_string()],
                vec![],
            ],
        },
        TagSetCase {
            case: "both published ImageTag length bounds",
            repository: "team/bounds",
            images: vec![vec!["x".to_string(), "y".repeat(300)]],
        },
        TagSetCase {
            case: "tag forms the unconstrained ImageTag pattern admits",
            repository: "team/forms",
            images: vec![vec![
                "1.0.0-alpha+build.1".to_string(),
                "UPPER_Case".to_string(),
                "sha-abc.def_ghi".to_string(),
                "release/2026".to_string(),
                "ünïcode".to_string(),
            ]],
        },
    ]
}

#[tokio::test]
async fn describe_image_tags_reports_one_entry_per_tag_whatever_the_tag_set_shape() {
    let router = service();

    for TagSetCase {
        case,
        repository: name,
        images,
    } in tag_set_cases()
    {
        repository(&router, name).await;

        // Push one manifest per row, carrying the row's tags; a manifest with no tag is
        // pushed untagged.
        let mut tag_to_digest: Vec<(String, String)> = Vec::new();
        let mut untagged_digests: Vec<String> = Vec::new();
        for (index, tags) in images.iter().enumerate() {
            let manifest = image_manifest(&format!("{name}-{index}"), &[(index as i64 + 1) * 10]);
            if tags.is_empty() {
                untagged_digests.push(push(&router, name, &manifest, None, case).await);
                continue;
            }
            for tag in tags {
                let digest = push(&router, name, &manifest, Some(tag), case).await;
                tag_to_digest.push((tag.clone(), digest));
            }
        }

        let entries = tag_details(&router, name, None, case).await;

        // One entry per tag, in tag order — a manifest with three tags is reported three
        // times, and an untagged manifest not at all.
        let mut expected: Vec<String> = tag_to_digest.iter().map(|(tag, _)| tag.clone()).collect();
        expected.sort();
        assert_eq!(
            tags_of(&entries),
            expected,
            "case {case:?}: expected one entry per tag in tag order, got {entries:?}"
        );

        // Every entry's `imageDetail` is the manifest that tag points at, reported
        // exactly as `DescribeImages` reports the same manifest.
        for (tag, digest) in &tag_to_digest {
            let entry = entries
                .iter()
                .find(|entry| entry["imageTag"].as_str() == Some(tag.as_str()))
                .unwrap_or_else(|| panic!("case {case:?}: expected an entry for {tag:?}"));
            let detail = &entry["imageDetail"];
            assert_eq!(
                detail["imageDigest"].as_str(),
                Some(digest.as_str()),
                "case {case:?}: expected tag {tag:?} to name the manifest it was pushed \
                 with, got {detail}"
            );
            let described = image_detail_of(&router, name, digest, case).await;
            for member in SHARED_IMAGE_MEMBERS {
                assert_eq!(
                    detail[member], described[member],
                    "case {case:?}: expected {member} of tag {tag:?} to match what \
                     DescribeImages reports for the same manifest, got {detail} vs {described}"
                );
            }
            // A tag cannot have been created before the manifest it names was pushed.
            let created_at = entry["createdAt"].as_f64().unwrap_or_default();
            let pushed_at = detail["imagePushedAt"].as_f64().unwrap_or_default();
            assert!(
                created_at > 0.0 && created_at >= pushed_at,
                "case {case:?}: expected tag {tag:?} to carry a createdAt at or after its \
                 manifest's imagePushedAt ({pushed_at}), got {created_at}"
            );
        }

        // An untagged manifest is in the image listing but in no tag entry.
        for digest in &untagged_digests {
            let described = image_detail_of(&router, name, digest, case).await;
            assert_eq!(
                described["imageDigest"].as_str(),
                Some(digest.as_str()),
                "case {case:?}: expected the untagged manifest to be stored, got {described}"
            );
            assert!(
                entries
                    .iter()
                    .all(|entry| entry["imageDetail"]["imageDigest"].as_str()
                        != Some(digest.as_str())),
                "case {case:?}: expected no tag entry for the untagged manifest {digest}, \
                 got {entries:?}"
            );
        }
    }
}

/// One tagged manifest, and the `imageSizeInBytes` its referenced content states.
struct SizeCase {
    case: &'static str,
    tag: &'static str,
    manifest: String,
    expected: Option<i64>,
}

#[tokio::test]
async fn a_tag_entry_reports_the_stored_manifest_and_two_reads_report_the_same_stamps() {
    let router = service();
    let name = "team/sizes";
    repository(&router, name).await;

    let one_layer = image_manifest("one", &[7]);
    let three_layers = image_manifest("three", &[1, 2, 3]);
    let no_layers = image_manifest("none", &[]);
    let opaque = opaque_manifest("opaque");
    // A manifest list is "the max size of all manifests in the list"
    // (`ReferencedImageDetail$imageSizeInBytes`), over the children stored beside it.
    let list = manifest_list(&[&digest_of(&one_layer), &digest_of(&three_layers)]);

    let cases = vec![
        SizeCase {
            case: "one layer",
            tag: "one",
            manifest: one_layer.clone(),
            expected: Some(10_006),
        },
        SizeCase {
            case: "three layers add up",
            tag: "three",
            manifest: three_layers.clone(),
            expected: Some(10_005),
        },
        SizeCase {
            case: "an empty layers array references no layer content",
            tag: "empty",
            manifest: no_layers,
            expected: Some(9_999),
        },
        SizeCase {
            case: "a manifest stating no referenced content states no size",
            tag: "opaque",
            manifest: opaque,
            expected: None,
        },
        SizeCase {
            case: "a manifest list is the max over its children",
            tag: "list",
            manifest: list,
            expected: Some(10_006),
        },
    ];

    for SizeCase {
        case, tag, manifest, ..
    } in &cases
    {
        push(&router, name, manifest, Some(tag), case).await;
    }

    let first = tag_details(&router, name, None, "first read").await;
    for SizeCase {
        case,
        tag,
        manifest,
        expected,
    } in &cases
    {
        let entry = first
            .iter()
            .find(|entry| entry["imageTag"].as_str() == Some(*tag))
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry for {tag:?}"));
        let detail = &entry["imageDetail"];
        assert_eq!(
            detail["imageDigest"].as_str(),
            Some(digest_of(manifest).as_str()),
            "case {case:?}: expected the pushed manifest's digest, got {detail}"
        );
        assert_eq!(
            detail["imageSizeInBytes"].as_i64(),
            *expected,
            "case {case:?}: expected imageSizeInBytes {expected:?}, got {detail}"
        );
        // The media types are the manifest's own: whatever its document declared, and
        // nothing invented for a document that declares none.
        let declared: Value = serde_json::from_str(manifest).expect("the manifest is JSON");
        assert_eq!(
            detail["imageManifestMediaType"],
            declared["mediaType"],
            "case {case:?}: expected the manifest's own mediaType, got {detail}"
        );
        assert_eq!(
            detail["artifactMediaType"],
            declared["config"]["mediaType"],
            "case {case:?}: expected the config blob's mediaType, got {detail}"
        );
    }

    // A read is a read: the second call reports byte-for-byte what the first did, so
    // neither `createdAt` nor `imagePushedAt` moves because somebody looked.
    let second = tag_details(&router, name, None, "second read").await;
    assert_eq!(
        first, second,
        "expected two reads of one repository to report the same tag details, got \
         {first:?} then {second:?}"
    );
}

#[tokio::test]
async fn pagination_partitions_the_tag_set_exactly_at_every_page_size() {
    let router = service();
    let name = "team/paged";
    repository(&router, name).await;

    // Eleven tags over three manifests, so a page boundary can fall inside one
    // manifest's tags as well as between manifests.
    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for index in 0..11 {
        let manifest = image_manifest(&format!("paged-{}", index % 3), &[(index + 1) as i64]);
        let tag = format!("v{index:02}");
        push(&router, name, &manifest, Some(&tag), "seed").await;
        all_tags.insert(tag);
    }
    let expected: Vec<String> = all_tags.iter().cloned().collect();

    // Every page size from 1 to one past the set size: the sizes that divide the set
    // exactly and the ones that leave a short final page.
    for page_size in 1..=12 {
        let case = format!("maxResults={page_size}");
        let walked = tags_of(&tag_details(&router, name, Some(page_size), &case).await);
        let unique: BTreeSet<String> = walked.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {case:?}: expected no tag twice across pages, got {walked:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every tag exactly once, in tag order, got {walked:?}"
        );
    }

    // `maxResults` is modelled `range 1..1000`: both published bounds are served, and one
    // past either is refused.
    for (case, max_results) in [("minimum", 1), ("maximum", 1000)] {
        ok(
            &router,
            "DescribeImageTags",
            json!({"repositoryName": name, "maxResults": max_results}),
            case,
        )
        .await;
    }
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1001)] {
        refused(
            &router,
            "DescribeImageTags",
            json!({"repositoryName": name, "maxResults": max_results}),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // A token this service did not mint is an invalid parameter, not an empty page.
    for case in ["not-hex", "zz", "ff"] {
        refused(
            &router,
            "DescribeImageTags",
            json!({"repositoryName": name, "nextToken": case}),
            "InvalidParameterException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_tag_entries_follow_every_push_and_delete() {
    let router = service();
    let name = "team/lifecycle";
    repository(&router, name).await;

    let first = image_manifest("first", &[11]);
    let second = image_manifest("second", &[22]);
    let first_digest = push(&router, name, &first, Some("latest"), "first push").await;
    push(&router, name, &first, Some("v1"), "second tag").await;

    let before = tag_details(&router, name, None, "before the move").await;
    assert_eq!(
        tags_of(&before),
        vec!["latest".to_string(), "v1".to_string()],
        "expected both tags of the one manifest, got {before:?}"
    );

    // Re-pushing an existing tag onto another manifest MOVES it: the entry then names
    // the new manifest, and the tag the old manifest keeps is untouched.
    let second_digest = push(&router, name, &second, Some("latest"), "move the tag").await;
    let moved = tag_details(&router, name, None, "after the move").await;
    assert_eq!(
        tags_of(&moved),
        vec!["latest".to_string(), "v1".to_string()],
        "expected the tag set to be unchanged by a move, got {moved:?}"
    );
    assert_eq!(
        moved[0]["imageDetail"]["imageDigest"].as_str(),
        Some(second_digest.as_str()),
        "expected the moved tag to name the manifest it was re-pushed with, got {moved:?}"
    );
    assert_eq!(
        moved[1]["imageDetail"]["imageDigest"].as_str(),
        Some(first_digest.as_str()),
        "expected the untouched tag to keep naming its manifest, got {moved:?}"
    );

    // Deleting by tag removes exactly that tag's entry.
    ok(
        &router,
        "BatchDeleteImage",
        json!({"repositoryName": name, "imageIds": [{"imageTag": "v1"}]}),
        "delete by tag",
    )
    .await;
    let after_tag_delete = tag_details(&router, name, None, "after deleting one tag").await;
    assert_eq!(
        tags_of(&after_tag_delete),
        vec!["latest".to_string()],
        "expected the deleted tag to be gone, got {after_tag_delete:?}"
    );

    // Deleting by digest removes the manifest and every tag that went with it.
    ok(
        &router,
        "BatchDeleteImage",
        json!({
            "repositoryName": name,
            "imageIds": [{"imageDigest": second_digest}],
        }),
        "delete by digest",
    )
    .await;
    let after_digest_delete = tag_details(&router, name, None, "after deleting the manifest").await;
    assert!(
        after_digest_delete.is_empty(),
        "expected no tag entry once every manifest is deleted, got {after_digest_delete:?}"
    );

    // A repository that holds no tag reports an empty list, not a failure; naming this
    // account's own registry changes nothing, and a repository that was never created is
    // not found.
    let own_registry = ok(
        &router,
        "DescribeImageTags",
        json!({"repositoryName": name, "registryId": REGISTRY_ID}),
        "own registry",
    )
    .await;
    assert_eq!(
        own_registry["imageTagDetails"],
        json!([]),
        "expected an empty imageTagDetails for a repository with no tag, got {own_registry}"
    );
    refused(
        &router,
        "DescribeImageTags",
        json!({"repositoryName": name, "registryId": "123456789012"}),
        "InvalidParameterException",
        "another account's registry",
    )
    .await;
    refused(
        &router,
        "DescribeImageTags",
        json!({"repositoryName": "team/never-created"}),
        "RepositoryNotFoundException",
        "a repository that was never created",
    )
    .await;
}
