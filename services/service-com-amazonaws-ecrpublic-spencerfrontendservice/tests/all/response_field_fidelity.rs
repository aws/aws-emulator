// SPDX-License-Identifier: CC0-1.0

//! Three response-fidelity rules of the ECR Public surface, driven end to end over the
//! wire through the harness in [`crate::repository_image_lifecycle`].
//!
//! 1. `ImageDetail$imageSizeInBytes` is the size of the content a manifest REFERENCES —
//!    the total over the DISTINCT blobs the image is made of, its config blob and its
//!    layer blobs, and for a manifest list the max over the images it references — never
//!    the size of the manifest document itself. A blob's size is the one its descriptor
//!    declares, else the byte count the registry recorded for the blob it stored. Source:
//!    the member's own doc, via
//!    `smithy_query(query="doc", shape="ImageDetail", member="imageSizeInBytes")`:
//!    "The size, in bytes, of the image in the repository. If the image is a manifest
//!    list, this is the max size of all manifests in the list". A list's size is a
//!    statement about other images in the repository, so it follows them as they are
//!    pushed and deleted.
//! 2. `BatchDeleteImage` reports what it removed FULLY identified: one `ImageIdentifier`
//!    per digest-and-tag pair the delete detached, and a digest-only identifier when the
//!    image it removed carried no tag.
//! 3. A response omits a structure member that is neither `@required` nor `@default` and
//!    that the service has not set — `ImageDetail$imageTags` for a manifest that carries
//!    no tag, and `RepositoryCatalogData$marketplaceCertified`, which nothing a caller can
//!    do makes the service state — rather than writing a zero value, so "no statement"
//!    stays distinguishable from "an empty list" and from "false".
//!
//! Each case names itself and the actual value in its assertion, and every table row
//! differs along what its rule generalises over: the number and size of layers, the
//! ordering and resolvability of a manifest list's children, the number of tags on a
//! deleted image and the way the request names it, and which catalog members a create
//! supplies.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::MAX_MANIFEST_LIST_ENTRIES;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{descriptor, given_manifest_blobs, image_manifest, manifest_list};
use crate::repository_image_lifecycle::{call, digest_of, ok, service};

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

/// Push `manifest` under `repository_name` with an optional tag, after putting every layer
/// blob it names into the repository.
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

/// The one `imageDetails` entry for `digest`.
async fn detail_of(
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

/// One image manifest, and the `imageSizeInBytes` the blobs its descriptors name add up
/// to.
struct SizeCase {
    case: &'static str,
    manifest: String,
    expected: Option<i64>,
}

#[tokio::test]
async fn image_size_in_bytes_totals_the_blobs_the_manifest_references() {
    let router = service();
    repository(&router, "sizes/images").await;

    let cases = [
        SizeCase {
            case: "one layer",
            manifest: image_manifest("one-layer", 7, &[209]),
            expected: Some(216),
        },
        SizeCase {
            case: "three layers of different sizes",
            manifest: image_manifest("three-layers", 7, &[1, 1024, 65_536]),
            expected: Some(66_568),
        },
        SizeCase {
            case: "a config blob far larger than the layers",
            manifest: image_manifest("fat-config", 10_000_000, &[12, 30]),
            expected: Some(10_000_042),
        },
        SizeCase {
            // No layer content at all: the image is its config blob, so that is its size.
            case: "no layers at all",
            manifest: image_manifest("no-layers", 7, &[]),
            expected: Some(7),
        },
        SizeCase {
            case: "a layer past the 32-bit range, since the member is a long",
            manifest: image_manifest("big-layer", 7, &[4_294_967_296, 1]),
            expected: Some(4_294_967_304),
        },
        SizeCase {
            case: "a docker v2 manifest, not an oci one",
            manifest: json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
                "config": descriptor(
                    "application/vnd.docker.container.image.v1+json",
                    "docker-config",
                    5_000,
                ),
                "layers": [
                    descriptor(
                        "application/vnd.docker.image.rootfs.diff.tar.gzip",
                        "docker-layer",
                        777,
                    ),
                ],
            })
            .to_string(),
            expected: Some(5_777),
        },
        SizeCase {
            // A document that references no sized content says nothing about an image
            // size, and `imageSizeInBytes` is neither required nor defaulted.
            case: "a schema 1 manifest, whose fsLayers carry no sizes",
            manifest: json!({
                "schemaVersion": 1,
                "name": "legacy",
                "fsLayers": [{"blobSum": "sha256:0"}],
            })
            .to_string(),
            expected: None,
        },
        SizeCase {
            // A descriptor's size is the byte count of its blob, so a value below zero is
            // no byte count at all: what the document does not state about a blob, the
            // registry's own count of the blob it stored does — here the 22 bytes of the
            // layer body `negative-layer-layer-0` the push uploaded.
            case: "a layer size that is not a byte count, counted by the registry instead",
            manifest: image_manifest("negative-layer", 7, &[-1_000, 50]),
            expected: Some(7 + "negative-layer-layer-0".len() as i64 + 50),
        },
        SizeCase {
            // The config descriptor is one of the blobs the total is over, so the same
            // reading holds for it: the registry counted `negative-config-config`.
            case: "a config size that is not a byte count, counted by the registry instead",
            manifest: image_manifest("negative-config", -1, &[50]),
            expected: Some("negative-config-config".len() as i64 + 50),
        },
        SizeCase {
            // The addends come from the request, so the total is computed with checked
            // arithmetic: a total no `long` can represent is reported as unknown, never
            // wrapped to a negative size, and never a panic.
            case: "layer sizes whose total no long can represent",
            manifest: image_manifest("overflowing-layers", 7, &[i64::MAX, i64::MAX]),
            expected: None,
        },
        SizeCase {
            // A descriptor naming no blob at all names nothing the registry could have
            // counted, so the total it belongs to is unknown rather than understated.
            case: "a layer descriptor that declares neither a size nor a digest",
            manifest: json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "layers": [
                    {"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip"},
                    descriptor(
                        "application/vnd.oci.image.layer.v1.tar+gzip",
                        "sized-layer",
                        5,
                    ),
                ],
            })
            .to_string(),
            expected: None,
        },
        SizeCase {
            // A JSON document that references neither layers nor manifests states the
            // size of nothing. (A body that is not JSON at all never reaches this
            // question: `PutImage` refuses it — see `tests/all/json_document_members.rs`.)
            case: "a json document that references nothing",
            manifest: json!({"schemaVersion": 2, "note": "not a manifest at all"}).to_string(),
            expected: None,
        },
    ];

    for (index, size_case) in cases.iter().enumerate() {
        let case = size_case.case;
        let digest = push(
            &router,
            "sizes/images",
            &size_case.manifest,
            Some(&format!("v{index}")),
            case,
        )
        .await;
        let detail = detail_of(&router, "sizes/images", &digest, case).await;
        assert_eq!(
            detail["imageSizeInBytes"].as_i64(),
            size_case.expected,
            "case {case:?}: expected imageSizeInBytes {:?}, got {}",
            size_case.expected,
            detail["imageSizeInBytes"]
        );
        if let Some(expected) = size_case.expected {
            assert_ne!(
                expected, size_case.manifest.len() as i64,
                "case {case:?}: the row must distinguish the referenced blob total from \
                 the manifest's own byte length"
            );
        }
    }
}

#[tokio::test]
async fn a_manifest_list_reports_the_max_size_of_the_images_it_references() {
    let router = service();
    repository(&router, "sizes/lists").await;

    // Three single-platform images, pushed first, as a client pushes a manifest list's
    // children before the list itself. Each child's own size is its config blob plus its
    // layers: 17, 507 and 78 bytes.
    let small = image_manifest("small", 7, &[10]);
    let large = image_manifest("large", 7, &[500]);
    let middle = image_manifest("middle", 7, &[70, 1]);
    let small_digest = push(&router, "sizes/lists", &small, Some("small"), "children").await;
    let large_digest = push(&router, "sizes/lists", &large, Some("large"), "children").await;
    let middle_digest = push(&router, "sizes/lists", &middle, Some("middle"), "children").await;
    let absent_digest = format!("sha256:{}", "a".repeat(64));

    // Each row is a different set, ordering and resolvability of referenced children:
    // the answer is the max over the children that exist, never the first, the last, or
    // the sum.
    let rows: Vec<(&str, Vec<&str>, Option<i64>)> = vec![
        (
            "largest child last",
            vec![
                small_digest.as_str(),
                middle_digest.as_str(),
                large_digest.as_str(),
            ],
            Some(507),
        ),
        (
            "largest child first",
            vec![
                large_digest.as_str(),
                middle_digest.as_str(),
                small_digest.as_str(),
            ],
            Some(507),
        ),
        (
            "a subset that excludes the largest child",
            vec![small_digest.as_str(), middle_digest.as_str()],
            Some(78),
        ),
        ("a single child", vec![small_digest.as_str()], Some(17)),
        (
            "no child this registry holds",
            vec![absent_digest.as_str()],
            None,
        ),
        ("an empty list", vec![], None),
    ];

    for (index, (case, children, expected)) in rows.into_iter().enumerate() {
        let list = manifest_list(&children);
        let digest = push(
            &router,
            "sizes/lists",
            &list,
            Some(&format!("list-{index}")),
            case,
        )
        .await;
        let detail = detail_of(&router, "sizes/lists", &digest, case).await;
        assert_eq!(
            detail["imageSizeInBytes"].as_i64(),
            expected,
            "case {case:?}: expected imageSizeInBytes {expected:?}, got {}",
            detail["imageSizeInBytes"]
        );
    }

    // A list may name as many children as the service reads, and no more: at the bound
    // it is read, one past it the document states no size this service reports rather
    // than a max over part of its entries. Source: `state::MAX_MANIFEST_LIST_ENTRIES`.
    let padding: Vec<String> = (0..MAX_MANIFEST_LIST_ENTRIES)
        .map(|entry| format!("sha256:{entry:064x}"))
        .collect();
    for (case, entry_count, expected) in [
        ("entries exactly at the bound", MAX_MANIFEST_LIST_ENTRIES, Some(507)),
        ("one entry past the bound", MAX_MANIFEST_LIST_ENTRIES + 1, None),
    ] {
        // The held child is the last entry, so an implementation reading only a prefix
        // would answer differently from one reading the whole document.
        let mut children: Vec<&str> = padding
            .iter()
            .take(entry_count - 1)
            .map(String::as_str)
            .collect();
        children.push(large_digest.as_str());
        assert_eq!(
            children.len(),
            entry_count,
            "case {case:?}: the row must name {entry_count} children"
        );
        let list = manifest_list(&children);
        let digest = push(&router, "sizes/lists", &list, Some(case), case).await;
        let detail = detail_of(&router, "sizes/lists", &digest, case).await;
        assert_eq!(
            detail["imageSizeInBytes"].as_i64(),
            expected,
            "case {case:?}: expected imageSizeInBytes {expected:?}, got {}",
            detail["imageSizeInBytes"]
        );
    }

    // Naming one child repeatedly is naming it once.
    let repeated = manifest_list(&[
        small_digest.as_str(),
        small_digest.as_str(),
        small_digest.as_str(),
    ]);
    let repeated_digest = push(
        &router,
        "sizes/lists",
        &repeated,
        Some("repeated"),
        "the same child named three times",
    )
    .await;
    let detail = detail_of(
        &router,
        "sizes/lists",
        &repeated_digest,
        "the same child named three times",
    )
    .await;
    assert_eq!(
        detail["imageSizeInBytes"].as_i64(),
        Some(17),
        "case {:?}: expected the child's own size, got {detail}",
        "the same child named three times"
    );
}

#[tokio::test]
async fn a_manifest_list_follows_the_children_it_references() {
    let router = service();
    repository(&router, "sizes/order").await;

    // A list pushed BEFORE its children: it references images the repository does not
    // hold yet, so it states no size.
    let small = image_manifest("ordered-small", 7, &[10]);
    let large = image_manifest("ordered-large", 7, &[640]);
    let small_digest = digest_of(&small);
    let large_digest = digest_of(&large);
    let list = manifest_list(&[small_digest.as_str(), large_digest.as_str()]);
    let list_digest = push(&router, "sizes/order", &list, Some("index"), "list first").await;
    let before = detail_of(&router, "sizes/order", &list_digest, "list first").await;
    assert_eq!(
        before["imageSizeInBytes"].as_i64(),
        None,
        "case {:?}: expected no size while no referenced image is held, got {before}",
        "list first"
    );

    // Each child that arrives is accounted for as it arrives.
    for (case, manifest, tag, expected) in [
        ("the smaller child arrives", &small, "small", Some(17)),
        ("the larger child arrives", &large, "large", Some(647)),
    ] {
        push(&router, "sizes/order", manifest, Some(tag), case).await;
        let detail = detail_of(&router, "sizes/order", &list_digest, case).await;
        assert_eq!(
            detail["imageSizeInBytes"].as_i64(),
            expected,
            "case {case:?}: expected imageSizeInBytes {expected:?}, got {}",
            detail["imageSizeInBytes"]
        );
    }

    // And a child that leaves stops counting: deleting the largest leaves the next.
    ok(
        &router,
        "BatchDeleteImage",
        json!({
            "repositoryName": "sizes/order",
            "imageIds": [{"imageDigest": large_digest}],
        }),
        "the larger child leaves",
    )
    .await;
    let after = detail_of(&router, "sizes/order", &list_digest, "the larger child leaves").await;
    assert_eq!(
        after["imageSizeInBytes"].as_i64(),
        Some(17),
        "case {:?}: expected the remaining child's size, got {after}",
        "the larger child leaves"
    );
}

/// One `BatchDeleteImage` call, and the digest-and-tag pairs it must report.
struct DeleteCase {
    case: &'static str,
    /// The tags the stored image carries before the delete.
    tags: Vec<&'static str>,
    /// Whether the request names the image by digest.
    by_digest: bool,
    /// The tag the request names, if any.
    by_tag: Option<&'static str>,
    /// The tags the reported identifiers must carry; empty means one identifier with no
    /// `imageTag` at all.
    reported_tags: Vec<&'static str>,
}

#[tokio::test]
async fn batch_delete_image_reports_every_digest_and_tag_pair_it_removed() {
    let cases = [
        DeleteCase {
            case: "one tag, deleted by digest",
            tags: vec!["probe-tag"],
            by_digest: true,
            by_tag: None,
            reported_tags: vec!["probe-tag"],
        },
        DeleteCase {
            case: "three tags, deleted by digest",
            tags: vec!["alpha", "beta", "gamma"],
            by_digest: true,
            by_tag: None,
            reported_tags: vec!["alpha", "beta", "gamma"],
        },
        DeleteCase {
            case: "no tag at all, deleted by digest",
            tags: vec![],
            by_digest: true,
            by_tag: None,
            reported_tags: vec![],
        },
        DeleteCase {
            case: "two tags, one of them deleted by tag",
            tags: vec!["keep", "remove"],
            by_digest: false,
            by_tag: Some("remove"),
            reported_tags: vec!["remove"],
        },
        DeleteCase {
            case: "the last tag, deleted by tag",
            tags: vec!["only"],
            by_digest: false,
            by_tag: Some("only"),
            reported_tags: vec!["only"],
        },
        DeleteCase {
            case: "two tags, named by digest and one tag together",
            tags: vec!["first", "second"],
            by_digest: true,
            by_tag: Some("second"),
            reported_tags: vec!["first", "second"],
        },
    ];

    for (index, delete_case) in cases.iter().enumerate() {
        let router = service();
        let case = delete_case.case;
        let repository_name = format!("deletes/case{index:02}");
        repository(&router, &repository_name).await;

        let manifest = image_manifest(case, 7, &[11, 22]);
        let digest = digest_of(&manifest);
        if delete_case.tags.is_empty() {
            push(&router, &repository_name, &manifest, None, case).await;
        } else {
            for tag in &delete_case.tags {
                push(&router, &repository_name, &manifest, Some(tag), case).await;
            }
        }

        let mut requested = json!({});
        if delete_case.by_digest {
            requested["imageDigest"] = json!(digest);
        }
        if let Some(tag) = delete_case.by_tag {
            requested["imageTag"] = json!(tag);
        }
        let reply = ok(
            &router,
            "BatchDeleteImage",
            json!({"repositoryName": repository_name, "imageIds": [requested]}),
            case,
        )
        .await;

        let reported = reply["imageIds"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected imageIds, got {reply}"))
            .clone();
        assert_eq!(
            reported.len(),
            delete_case.reported_tags.len().max(1),
            "case {case:?}: expected one identifier per removed digest-and-tag pair, \
             got {reply}"
        );
        for identifier in &reported {
            assert_eq!(
                identifier["imageDigest"],
                json!(digest),
                "case {case:?}: every reported identifier carries the digest, got \
                 {identifier}"
            );
        }
        let reported_tags: BTreeSet<String> = reported
            .iter()
            .filter_map(|identifier| identifier["imageTag"].as_str())
            .map(ToString::to_string)
            .collect();
        let expected_tags: BTreeSet<String> = delete_case
            .reported_tags
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the removed tags {expected_tags:?} to be reported, \
             got {reported_tags:?} in {reply}"
        );
        if delete_case.reported_tags.is_empty() {
            assert_eq!(
                reported[0]["imageTag"],
                Value::Null,
                "case {case:?}: an untagged image is reported by digest alone, got {reply}"
            );
        }
        assert_eq!(
            reply["failures"],
            json!([]),
            "case {case:?}: expected no failures, got {reply}"
        );

        // What was reported is what is gone: a tag the delete did not detach still
        // resolves, and one it did no longer does.
        let surviving: Vec<&str> = delete_case
            .tags
            .iter()
            .copied()
            .filter(|tag| !delete_case.reported_tags.contains(tag))
            .collect();
        for tag in &surviving {
            let described = ok(
                &router,
                "DescribeImages",
                json!({
                    "repositoryName": repository_name,
                    "imageIds": [{"imageTag": tag}],
                }),
                case,
            )
            .await;
            assert_eq!(
                described["imageDetails"][0]["imageDigest"],
                json!(digest),
                "case {case:?}: expected the tag {tag:?} the delete did not report to \
                 still resolve, got {described}"
            );
        }
        for tag in &delete_case.reported_tags {
            let (status, body) = call(
                &router,
                "DescribeImages",
                json!({
                    "repositoryName": repository_name,
                    "imageIds": [{"imageTag": tag}],
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the reported tag {tag:?} to be gone, got \
                 {status} with {body}"
            );
        }
        if surviving.is_empty() {
            let left = ok(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository_name}),
                case,
            )
            .await;
            assert_eq!(
                left["imageDetails"],
                json!([]),
                "case {case:?}: expected the image itself to be gone, got {left}"
            );
        }
    }
}

#[tokio::test]
async fn an_image_detail_omits_the_tag_list_of_a_manifest_that_carries_no_tag() {
    let router = service();
    repository(&router, "tags/omitted").await;

    // One row per tag set a stored manifest can have: pushed with no tag at all, pushed
    // with one, pushed with several, and left untagged again because another push moved
    // its last tag away.
    let untagged = image_manifest("untagged", 7, &[3]);
    let untagged_digest = push(&router, "tags/omitted", &untagged, None, "no tag at all").await;
    let detail = detail_of(&router, "tags/omitted", &untagged_digest, "no tag at all").await;
    assert!(
        detail.get("imageTags").is_none(),
        "case {:?}: expected no imageTags member at all, got {detail}",
        "no tag at all"
    );

    // The same manifest, now pushed with a tag: the member appears, so the omission is
    // about an image with no tags and not about dropping the member.
    push(
        &router,
        "tags/omitted",
        &untagged,
        Some("first"),
        "one tag",
    )
    .await;
    let detail = detail_of(&router, "tags/omitted", &untagged_digest, "one tag").await;
    assert_eq!(
        detail["imageTags"],
        json!(["first"]),
        "case {:?}: expected the one tag to be reported, got {detail}",
        "one tag"
    );
    push(
        &router,
        "tags/omitted",
        &untagged,
        Some("second"),
        "two tags",
    )
    .await;
    let detail = detail_of(&router, "tags/omitted", &untagged_digest, "two tags").await;
    assert_eq!(
        detail["imageTags"],
        json!(["first", "second"]),
        "case {:?}: expected both tags to be reported, got {detail}",
        "two tags"
    );

    // A tag is mutable, so pushing both of them onto another manifest leaves the first
    // one with no tag: the member goes away again.
    let other = image_manifest("other", 7, &[4]);
    for tag in ["first", "second"] {
        push(&router, "tags/omitted", &other, Some(tag), "the tags move").await;
    }
    let detail = detail_of(&router, "tags/omitted", &untagged_digest, "the tags move").await;
    assert!(
        detail.get("imageTags").is_none(),
        "case {:?}: expected the member to go away with the last tag, got {detail}",
        "the tags move"
    );
}

#[tokio::test]
async fn catalog_data_omits_the_certification_member_the_service_never_set() {
    let router = service();

    // One row per shape of catalog data a create can supply: none at all, one member,
    // every member, and a logo payload (which is the one member the service derives).
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no catalogData member", None),
        ("an empty catalogData", Some(json!({}))),
        (
            "one text member",
            Some(json!({"description": "a short description"})),
        ),
        (
            "every member a caller may set",
            Some(json!({
                "description": "described",
                "architectures": ["x86-64"],
                "operatingSystems": ["Linux"],
                "aboutText": "about",
                "usageText": "usage",
            })),
        ),
        (
            // `logoImageBlob` travels as base64 on awsJson1_1; this is the byte 0x01.
            "a logo payload",
            Some(json!({"logoImageBlob": "AQ=="})),
        ),
    ];

    for (index, (case, catalog_data)) in rows.into_iter().enumerate() {
        let repository_name = format!("catalog/case{index:02}");
        let mut body = json!({"repositoryName": repository_name});
        if let Some(catalog_data) = catalog_data.clone() {
            body["catalogData"] = catalog_data;
        }
        let created = ok(&router, "CreateRepository", body, case).await;

        let catalog = created["catalogData"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a catalogData object, got {created}"));
        assert!(
            !catalog.contains_key("marketplaceCertified"),
            "case {case:?}: expected no marketplaceCertified key at all, got {:?}",
            created["catalogData"]
        );

        // The members the caller did set are still reported, so the omission is about an
        // unset member and not about dropping catalog data.
        if let Some(Value::Object(requested)) = catalog_data {
            for (member, value) in requested {
                if member == "logoImageBlob" {
                    assert!(
                        catalog
                            .get("logoUrl")
                            .and_then(Value::as_str)
                            .is_some_and(|url| !url.is_empty()),
                        "case {case:?}: expected a logo URL derived from the payload, \
                         got {:?}",
                        created["catalogData"]
                    );
                    continue;
                }
                assert_eq!(
                    catalog.get(&member),
                    Some(&value),
                    "case {case:?}: expected the stored {member} to be reported, got {:?}",
                    created["catalogData"]
                );
            }
        }
    }
}
