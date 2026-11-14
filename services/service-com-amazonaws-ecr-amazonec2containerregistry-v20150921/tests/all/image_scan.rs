// SPDX-License-Identifier: CC0-1.0

//! Basic image scanning, end to end over the wire.
//!
//! The rule under test: `StartImageScan` records ONE basic scan on the stored image its
//! identifier resolves to, which the start admits QUEUED (`PENDING`) and the clock then
//! carries into its run ([`state::IMAGE_SCAN_QUEUE_DELAY`]) and on to the TERMINAL status
//! that run came to ([`state::IMAGE_SCAN_DELAY`]) — the outcome of examining that image
//! (`state::basic_scan_outcome`), which
//! for an image whose layers carry no operating system to identify is `FAILED` with the
//! unsupported-image description and never a clean `COMPLETE`;
//! `DescribeImageScanFindings` derives its whole answer from that one record and changes
//! nothing, the findings (and the severity counts over them) are reported once a scan that
//! examined the image has completed, an image carrying no scan is `ScanNotFoundException`
//! while an
//! identifier naming no stored image is `ImageNotFoundException`, a second start inside
//! [`state::BASIC_SCAN_INTERVAL`] is `LimitExceededException`, an image in Amazon ECR
//! archive is `ImageArchivedException`, and an image whose manifest declares no
//! filesystem layer is `UnsupportedImageTypeException`. `DescribeImages` reports no scan
//! at all: basic scanning's new version does not use its `imageScanStatus` /
//! `imageScanFindingsSummary` members.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it
//! exercises routing, deserialization, the handlers, the persisted state and
//! serialization — the same path a client takes.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, emulator, emulator_with_engine, given_layer, layer_digest, ok, ok_until, refused,
    REGISTRY_ID,
};

use ecr::ops::types::{Attribute, ImageScanFinding, ImageScanStatus, FINDING_SEVERITY_VALUES};
use ecr::state;

/// The manifest forms this registry stores, as `state::is_scannable_image_manifest`
/// reads them: the Docker image manifest V2 schema 2 and the OCI image manifest declare
/// their filesystem layers as `layers`, the older V2 schema 1 as `fsLayers`.
const DOCKER_SCHEMA_2: &str = "docker image manifest V2 schema 2";
/// See [`DOCKER_SCHEMA_2`].
const OCI_MANIFEST: &str = "OCI image manifest";
/// See [`DOCKER_SCHEMA_2`].
const DOCKER_SCHEMA_1: &str = "docker image manifest V2 schema 1";

/// How a request addresses the image: by its tag, by its digest, or by both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    Tag,
    Digest,
    Both,
}

/// One row of the scan table.
struct ScanCase<'a> {
    /// What the row is about.
    case: &'a str,
    /// The repository the row pushes its image into — its own, so no row depends on
    /// another.
    repository: &'a str,
    /// Which manifest form the image is pushed in.
    form: &'a str,
    /// How many filesystem layers that manifest declares.
    layers: usize,
    /// The tag the image is pushed under.
    tag: &'a str,
    /// How the start and the reads address it.
    address: Address,
}

/// Deterministic blob bytes: `len` bytes that depend on `seed`, so no two blobs of one
/// case carry the same content and every layer has its own digest.
fn blob_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + seed * 97 + 13) % 256).unwrap_or_default())
        .collect()
}

/// Create the repository `name`.
async fn given_repository(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
}

/// Push `manifest` into `repository`, answering the digest the registry addresses it by.
async fn push_manifest(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    manifest: &str,
    tag: Option<&str>,
) -> String {
    let mut request = json!({"repositoryName": repository, "imageManifest": manifest});
    if let Some(tag) = tag {
        request["imageTag"] = json!(tag);
    }
    ok(router, case, "PutImage", &request).await;
    layer_digest(manifest.as_bytes())
}

/// Push an image of `form` with `layers` filesystem layers into `repository`, uploading
/// every blob it references first — a manifest cannot be pushed before its blobs are in
/// the repository — and answer the digest the registry addresses it by.
///
/// `salt` distinguishes one image's blobs from another's in the same repository: a blob
/// is addressed by the digest of its bytes, so two images of one repository have to carry
/// different ones.
async fn given_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    form: &str,
    layers: usize,
    salt: usize,
    tag: Option<&str>,
) -> String {
    let mut layer_digests: Vec<String> = Vec::new();
    for index in 0..layers {
        let bytes = blob_bytes(repository.len() + salt * 13 + index, 64 + index * 7);
        layer_digests.push(given_layer(router, case, repository, &bytes).await);
    }

    let manifest = if form == DOCKER_SCHEMA_1 {
        json!({
            "schemaVersion": 1,
            "name": repository,
            "tag": tag.unwrap_or_default(),
            "fsLayers": layer_digests
                .iter()
                .map(|digest| json!({"blobSum": digest}))
                .collect::<Vec<Value>>(),
        })
    } else {
        let config = given_layer(
            router,
            case,
            repository,
            &blob_bytes(repository.len() + salt * 13 + 97, 73),
        )
        .await;
        let (manifest_media, config_media, layer_media) = if form == OCI_MANIFEST {
            (
                "application/vnd.oci.image.manifest.v1+json",
                "application/vnd.oci.image.config.v1+json",
                "application/vnd.oci.image.layer.v1.tar+gzip",
            )
        } else {
            (
                "application/vnd.docker.distribution.manifest.v2+json",
                "application/vnd.docker.container.image.v1+json",
                "application/vnd.docker.image.rootfs.diff.tar.gzip",
            )
        };
        json!({
            "schemaVersion": 2,
            "mediaType": manifest_media,
            "config": {"mediaType": config_media, "size": 73, "digest": config},
            "layers": layer_digests
                .iter()
                .map(|digest| json!({
                    "mediaType": layer_media,
                    "size": 64,
                    "digest": digest,
                }))
                .collect::<Vec<Value>>(),
        })
    };
    let manifest = serde_json::to_string(&manifest).expect("the test manifest serialises");
    push_manifest(router, case, repository, &manifest, tag).await
}

/// The `imageId` a request sends for `address`.
fn addressed(address: Address, digest: &str, tag: &str) -> Value {
    match address {
        Address::Tag => json!({"imageTag": tag}),
        Address::Digest => json!({"imageDigest": digest}),
        Address::Both => json!({"imageDigest": digest, "imageTag": tag}),
    }
}

/// The `imageId` an answer to that request reports: the stored image's own digest, and
/// the tag only where the request named one.
fn reported(address: Address, digest: &str, tag: &str) -> Value {
    match address {
        Address::Digest => json!({"imageDigest": digest}),
        Address::Tag | Address::Both => json!({"imageDigest": digest, "imageTag": tag}),
    }
}

/// Read the scan of one image.
async fn findings(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    image_id: &Value,
) -> Value {
    ok(
        router,
        case,
        "DescribeImageScanFindings",
        &json!({"repositoryName": repository, "imageId": image_id}),
    )
    .await
}

/// Whether the scan an answer reports has ENDED: a run is over at either of the terminal
/// statuses a basic scan reaches, and which one it reached is what an assertion judges.
fn is_terminal(answer: &Value) -> bool {
    [state::SCAN_STATUS_COMPLETE, state::SCAN_STATUS_FAILED]
        .iter()
        .any(|status| at(answer, "/imageScanStatus/status") == json!(status))
}

/// A started scan is admitted `PENDING`, reaches its terminal status on the clock, and is
/// then read back — unchanged by the reading — by both operations that report an image's
/// scan.
///
/// The rows differ along what the rule generalises over: the manifest form the image was
/// pushed in, how many filesystem layers it declares, and how the request addresses the
/// image (by tag, by digest, by both).
#[tokio::test]
async fn a_started_scan_is_reported_until_the_clock_completes_it() {
    let cases = vec![
        ScanCase {
            case: "a docker schema 2 image addressed by tag",
            repository: "scan-docker-by-tag",
            form: DOCKER_SCHEMA_2,
            layers: 1,
            tag: "v1",
            address: Address::Tag,
        },
        ScanCase {
            case: "a docker schema 2 image addressed by digest",
            repository: "scan-docker-by-digest",
            form: DOCKER_SCHEMA_2,
            layers: 3,
            tag: "latest",
            address: Address::Digest,
        },
        ScanCase {
            case: "an OCI image addressed by digest and tag",
            repository: "scan-oci-by-both",
            form: OCI_MANIFEST,
            layers: 2,
            tag: "release.2026",
            address: Address::Both,
        },
        ScanCase {
            case: "an OCI image of many layers addressed by tag",
            repository: "scan-oci-many-layers",
            form: OCI_MANIFEST,
            layers: 7,
            tag: "many",
            address: Address::Tag,
        },
        ScanCase {
            case: "a schema 1 image addressed by digest",
            repository: "scan-schema-1",
            form: DOCKER_SCHEMA_1,
            layers: 2,
            tag: "v0",
            address: Address::Digest,
        },
    ];

    let router = emulator();
    for row in cases {
        let ScanCase {
            case,
            repository,
            form,
            layers,
            tag,
            address,
        } = row;
        given_repository(&router, case, repository).await;
        let digest = given_image(&router, case, repository, form, layers, 0, Some(tag)).await;
        let image_id = addressed(address, &digest, tag);

        // Before anything is started the image carries no scan — a different answer from
        // the one an identifier naming no stored image gets.
        refused(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": image_id}),
            "ScanNotFoundException",
        )
        .await;

        let started = ok(
            &router,
            case,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": image_id}),
        )
        .await;
        assert_eq!(
            at(&started, "/registryId"),
            json!(REGISTRY_ID),
            "case {case:?}: the start names this registry, got {:?}",
            at(&started, "/registryId")
        );
        assert_eq!(
            at(&started, "/repositoryName"),
            json!(repository),
            "case {case:?}: the start names the repository scanned, got {:?}",
            at(&started, "/repositoryName")
        );
        assert_eq!(
            at(&started, "/imageId"),
            reported(address, &digest, tag),
            "case {case:?}: the start reports the stored image's own digest, got {:?}",
            at(&started, "/imageId")
        );
        // The start ADMITS the scan: the record it writes is queued, so what the start
        // reports is the queued state and not a run it did not perform.
        assert_eq!(
            at(&started, "/imageScanStatus/status"),
            json!(state::SCAN_STATUS_PENDING),
            "case {case:?}: a scan that has just been admitted is queued, got {:?}",
            at(&started, "/imageScanStatus")
        );

        // A read straight after the start meets the one record the start wrote, still
        // queued: nothing but the clock moves a run on, and it owes the move out of the
        // queue a span away. No findings either — they are readable once a run that
        // examined the image has completed.
        let running = findings(&router, case, repository, &image_id).await;
        assert_eq!(
            at(&running, "/imageScanStatus/status"),
            json!(state::SCAN_STATUS_PENDING),
            "case {case:?}: the read meets the queued run the start admitted, got {:?}",
            at(&running, "/imageScanStatus")
        );
        assert_eq!(
            at(&running, "/imageScanFindings"),
            Value::Null,
            "case {case:?}: a running scan has no findings to report, got {:?}",
            at(&running, "/imageScanFindings")
        );

        // A running scan honours the pagination members too: a token this service never
        // issued addresses no page of it either.
        refused(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({
                "repositoryName": repository,
                "imageId": image_id,
                "nextToken": "not-a-token",
            }),
            "InvalidParameterException",
        )
        .await;

        // The terminal status is the clock's to owe, so the suite looks again until the
        // run has ended rather than waiting out a span of wall clock. The poll waits for
        // TERMINALITY alone, so which status it ended at is judged by the assertion below.
        let completed = ok_until(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": image_id}),
            is_terminal,
        )
        .await;
        assert_eq!(
            at(&completed, "/imageScanStatus"),
            json!({
                "status": state::SCAN_STATUS_FAILED,
                "description": state::UNSUPPORTED_IMAGE_SCAN_DESCRIPTION,
            }),
            "case {case:?}: past the owed completion the scan reports the outcome of \
             examining this image, got {:?}",
            at(&completed, "/imageScanStatus")
        );
        assert_eq!(
            at(&completed, "/imageId"),
            reported(address, &digest, tag),
            "case {case:?}: the read reports the image it scanned, got {:?}",
            at(&completed, "/imageId")
        );
        // The layers these images are built on are no filesystem archive, so a basic
        // scan of them identified no operating system to examine: the run reports that
        // outcome and no findings block, which would state a scan of content nothing
        // read.
        assert_eq!(
            at(&completed, "/imageScanFindings"),
            Value::Null,
            "case {case:?}: a run that examined nothing reports no findings, got {:?}",
            at(&completed, "/imageScanFindings")
        );
        assert_eq!(
            at(&completed, "/nextToken"),
            Value::Null,
            "case {case:?}: with no findings reported no token follows them, got {:?}",
            at(&completed, "/nextToken")
        );

        // Reading changes nothing a later caller sees: the second read is the first.
        let again = findings(&router, case, repository, &image_id).await;
        assert_eq!(
            again, completed,
            "case {case:?}: a second read reports the same scan, got {again:?}"
        );

        // The image detail does NOT carry the scan: "The new version of Amazon ECR Basic
        // Scanning doesn't use the ImageDetail$imageScanFindingsSummary and
        // ImageDetail$imageScanStatus attributes from the API response to return scan
        // results. Use the DescribeImageScanFindings API instead." (`DescribeImages`),
        // and this registry is on that version — `BASIC_SCAN_TYPE_VERSION` is
        // `AWS_NATIVE`.
        let described = ok(
            &router,
            case,
            "DescribeImages",
            &json!({"repositoryName": repository, "imageIds": [image_id]}),
        )
        .await;
        assert_eq!(
            at(&described, "/imageDetails/0/imageDigest"),
            json!(digest),
            "case {case:?}: the image detail is the scanned image's, got {described:?}"
        );
        for member in ["imageScanStatus", "imageScanFindingsSummary"] {
            assert_eq!(
                at(&described, &format!("/imageDetails/0/{member}")),
                Value::Null,
                "case {case:?}: basic scanning does not report {member} in an image \
                 detail, got {:?}",
                at(&described, &format!("/imageDetails/0/{member}"))
            );
        }

        // "A basic image scan can only be started once per 24 hours on an individual
        // image": the second start is refused, and it leaves the recorded scan alone.
        refused(
            &router,
            case,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": image_id}),
            "LimitExceededException",
        )
        .await;
        let after_refusal = findings(&router, case, repository, &image_id).await;
        assert_eq!(
            after_refusal, completed,
            "case {case:?}: a refused second start changes no scan, got {after_refusal:?}"
        );
    }
}

/// An image whose manifest declares no filesystem layer is of a type a basic scan cannot
/// examine, and the refusal records nothing.
///
/// The rows differ along what declares no layer: a Docker manifest list and an OCI image
/// index (both reference other manifests), a manifest whose `layers` list is empty, and a
/// document that is not an image manifest at all.
#[tokio::test]
async fn an_image_that_declares_no_filesystem_layer_cannot_be_scanned() {
    let child = format!("sha256:{}", "a".repeat(64));
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a docker manifest list",
            "unscannable-manifest-list",
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.docker.distribution.manifest.list.v2+json",
                "manifests": [{
                    "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
                    "size": 7,
                    "digest": child,
                    "platform": {"architecture": "amd64", "os": "linux"},
                }],
            }),
        ),
        (
            "an OCI image index",
            "unscannable-oci-index",
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.index.v1+json",
                "manifests": [{
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "size": 11,
                    "digest": child,
                }],
            }),
        ),
        (
            "a manifest declaring an empty layer list",
            "unscannable-no-layers",
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "layers": [],
            }),
        ),
        (
            "a document that is not an image manifest",
            "unscannable-not-a-manifest",
            json!({"schemaVersion": 2, "image": "neither one thing nor the other"}),
        ),
    ];

    let router = emulator();
    for (case, repository, document) in cases {
        given_repository(&router, case, repository).await;
        let manifest = serde_json::to_string(&document).expect("the test manifest serialises");
        let digest = push_manifest(&router, case, repository, &manifest, Some("v1")).await;
        let image_id = json!({"imageDigest": digest});

        refused(
            &router,
            case,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": image_id}),
            "UnsupportedImageTypeException",
        )
        .await;

        // Nothing was recorded, so the image still carries no scan.
        refused(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": image_id}),
            "ScanNotFoundException",
        )
        .await;
    }
}

/// A scan call the registry cannot honour is refused with the modelled error for what it
/// could not resolve, and the two operations refuse the same request the same way —
/// except where only one of them models the error.
///
/// The rows differ along what cannot be resolved: the repository, the registry, the
/// digest, the tag, a digest and tag that disagree, an identifier naming neither, and a
/// malformed digest.
#[tokio::test]
async fn a_scan_call_the_registry_cannot_resolve_is_refused() {
    let router = emulator();
    let repository = "scan-refusals";
    given_repository(&router, "refusals", repository).await;
    let scanned = given_image(
        &router,
        "refusals",
        repository,
        DOCKER_SCHEMA_2,
        1,
        0,
        Some("scanned"),
    )
    .await;
    let unscanned = given_image(
        &router,
        "refusals",
        repository,
        OCI_MANIFEST,
        1,
        1,
        Some("unscanned"),
    )
    .await;
    ok(
        &router,
        "refusals",
        "StartImageScan",
        &json!({"repositoryName": repository, "imageId": {"imageDigest": scanned}}),
    )
    .await;

    let absent_digest = format!("sha256:{}", "f".repeat(64));
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository this registry has not got",
            json!({"repositoryName": "no-such-repo", "imageId": {"imageDigest": scanned}}),
            "RepositoryNotFoundException",
        ),
        (
            "another account's registry",
            json!({
                "repositoryName": repository,
                "registryId": "123456789012",
                "imageId": {"imageDigest": scanned},
            }),
            "InvalidParameterException",
        ),
        (
            "a digest this repository holds no image for",
            json!({"repositoryName": repository, "imageId": {"imageDigest": absent_digest}}),
            "ImageNotFoundException",
        ),
        (
            "a tag nothing in this repository carries",
            json!({"repositoryName": repository, "imageId": {"imageTag": "nowhere"}}),
            "ImageNotFoundException",
        ),
        (
            "a digest and a tag that name different images",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": scanned, "imageTag": "unscanned"},
            }),
            "ImageNotFoundException",
        ),
        (
            "an identifier naming neither a digest nor a tag",
            json!({"repositoryName": repository, "imageId": {}}),
            "InvalidParameterException",
        ),
        (
            "a digest that is not a content digest",
            json!({"repositoryName": repository, "imageId": {"imageDigest": "not-a-digest"}}),
            "InvalidParameterException",
        ),
    ];

    for (case, request, expected) in cases {
        for operation in ["StartImageScan", "DescribeImageScanFindings"] {
            refused(&router, case, operation, &request, expected).await;
        }
    }

    // A scan belongs to the image it was started on: the other image of the same
    // repository still carries none.
    refused(
        &router,
        "the image nothing scanned",
        "DescribeImageScanFindings",
        &json!({"repositoryName": repository, "imageId": {"imageDigest": unscanned}}),
        "ScanNotFoundException",
    )
    .await;

    // And the same digest in another repository is another image, with its own (absent)
    // scan.
    let elsewhere = "scan-refusals-elsewhere";
    given_repository(&router, "elsewhere", elsewhere).await;
    refused(
        &router,
        "the same digest in another repository",
        "DescribeImageScanFindings",
        &json!({"repositoryName": elsewhere, "imageId": {"imageDigest": scanned}}),
        "ImageNotFoundException",
    )
    .await;

    // Deleting the image drops its scan with it: the digest no longer addresses an image
    // at all.
    ok(
        &router,
        "deleted",
        "BatchDeleteImage",
        &json!({"repositoryName": repository, "imageIds": [{"imageDigest": scanned}]}),
    )
    .await;
    refused(
        &router,
        "the deleted image",
        "DescribeImageScanFindings",
        &json!({"repositoryName": repository, "imageId": {"imageDigest": scanned}}),
        "ImageNotFoundException",
    )
    .await;
}

/// An image in Amazon ECR archive cannot be scanned, and nor can one whose restore has
/// not completed; once the restore the clock owes falls due the same start succeeds.
///
/// The refusal rows differ along which archive state the image is in: `ARCHIVED`, and
/// `ACTIVATING` while a restore is in flight.
#[tokio::test]
async fn an_archived_image_cannot_be_scanned_until_it_is_restored() {
    let router = emulator();
    let repository = "scan-archived";
    given_repository(&router, "archived", repository).await;
    let digest = given_image(
        &router,
        "archived",
        repository,
        DOCKER_SCHEMA_2,
        1,
        0,
        Some("v1"),
    )
    .await;
    let image_id = json!({"imageDigest": digest});

    let archive_states: Vec<(&str, &str, &str)> = vec![
        ("in archive", "ARCHIVE", state::ARCHIVED_IMAGE_STATUS),
        (
            "restoring, before the restore is due",
            "STANDARD",
            state::ACTIVATING_IMAGE_STATUS,
        ),
    ];

    for (case, target, expected_status) in archive_states {
        let moved = ok(
            &router,
            case,
            "UpdateImageStorageClass",
            &json!({
                "repositoryName": repository,
                "imageId": image_id,
                "targetStorageClass": target,
            }),
        )
        .await;
        assert_eq!(
            at(&moved, "/imageStatus"),
            json!(expected_status),
            "case {case:?}: expected the image to report {expected_status}, got {:?}",
            at(&moved, "/imageStatus")
        );

        refused(
            &router,
            case,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": image_id}),
            "ImageArchivedException",
        )
        .await;
        // The refusal records nothing, so the image still carries no scan.
        refused(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": image_id}),
            "ScanNotFoundException",
        )
        .await;
    }

    // Past the restore the clock owed, the image is in standard storage again and the
    // same start is honoured. The restore is the clock's to owe, so the suite looks again
    // until the image reports itself restored.
    ok_until(
        &router,
        "restored",
        "DescribeImages",
        &json!({
            "repositoryName": repository,
            "imageIds": [image_id],
            "filter": {"imageStatus": "ANY"},
        }),
        |answer| {
            at(answer, "/imageDetails/0/imageStatus") == json!(state::ACTIVE_IMAGE_STATUS)
        },
    )
    .await;
    let started = ok(
        &router,
        "restored",
        "StartImageScan",
        &json!({"repositoryName": repository, "imageId": image_id}),
    )
    .await;
    assert_eq!(
        at(&started, "/imageScanStatus/status"),
        json!(state::SCAN_STATUS_PENDING),
        "case \"restored\": the restored image's scan is admitted queued, got {:?}",
        at(&started, "/imageScanStatus")
    );

    let completed = ok_until(
        &router,
        "restored",
        "DescribeImageScanFindings",
        &json!({"repositoryName": repository, "imageId": image_id}),
        is_terminal,
    )
    .await;
    assert_eq!(
        at(&completed, "/imageScanStatus/status"),
        json!(state::SCAN_STATUS_FAILED),
        "case \"restored\": the scan reaches its terminal status on the clock, got {:?}",
        at(&completed, "/imageScanStatus")
    );
}

/// One finding of a seeded scan: `CVE-2026-<index>` with `severity`. The pages are keyed
/// by a finding's index in the run, so this order is the order they are reported in.
fn finding(index: usize, severity: &str) -> ImageScanFinding {
    ImageScanFinding {
        name: Some(format!("CVE-2026-{index:04}")),
        description: Some(format!("the {severity} finding at index {index}")),
        uri: Some(format!("https://example.invalid/cve/2026-{index:04}")),
        severity: Some(severity.to_string()),
        attributes: Some(vec![Attribute {
            key: "package_name".to_string(),
            value: Some(format!("package-{index}")),
        }]),
    }
}

/// Seed `findings` as the completed scan of one stored image, straight into the store the
/// handlers read.
///
/// A basic scan of these images identifies no operating system to examine — their layers
/// carry no filesystem archive (`state::basic_scan_outcome`) — and no vulnerability
/// source is reachable from this process, so a run that carries FINDINGS is a record the
/// start path does not produce, and the paging and the severity counting over it are
/// exercised from the same store instead. Every other case here goes through the wire.
fn seed_findings(
    engine: &engine::InMemoryEngine,
    repository: &str,
    digest: &str,
    findings: Vec<ImageScanFinding>,
) {
    let state = state::service_state(engine).expect("the ecr store is reachable");
    let mut store = state.lock();
    let image = store
        .repositories
        .get_mut(repository)
        .expect("the seeded repository was created over the wire")
        .images
        .get_mut(digest)
        .expect("the seeded image was pushed over the wire");
    let now = SystemTime::now();
    image.scan = Some(state::ImageScan {
        started_at: now,
        // Both owed transitions are already due, so the scan has reached its terminal
        // status.
        running_due_at: now,
        complete_due_at: now,
        // A run that examined the image: the status whose findings are a statement about
        // it, and therefore the one whose findings are readable.
        terminal: ImageScanStatus {
            status: Some(state::SCAN_STATUS_COMPLETE.to_string()),
            description: Some(state::SCAN_COMPLETE_DESCRIPTION.to_string()),
        },
        findings,
    });
}

/// A completed scan's findings are paged, and every page reports the severity counts of
/// the WHOLE scan.
///
/// The rows differ along the page size asked for: none at all, one finding at a time, a
/// page that does not divide the list, exactly the list, and more than the list. The
/// severity catalog is the model's own `FindingSeverity` value set, which includes the
/// `UNDEFINED` value no example cites.
#[tokio::test]
async fn findings_are_paged_and_counted_by_severity() {
    let (router, engine) = emulator_with_engine();
    let repository = "scan-findings";
    given_repository(&router, "findings", repository).await;
    let digest = given_image(
        &router,
        "findings",
        repository,
        OCI_MANIFEST,
        1,
        0,
        Some("findings"),
    )
    .await;

    // One finding per modelled severity, plus a second of the first severity, so at
    // least one count is above one.
    let mut severities: Vec<&str> = FINDING_SEVERITY_VALUES.to_vec();
    severities.push(FINDING_SEVERITY_VALUES[0]);
    let seeded: Vec<ImageScanFinding> = severities
        .iter()
        .enumerate()
        .map(|(index, severity)| finding(index, severity))
        .collect();
    let names: Vec<String> = seeded
        .iter()
        .map(|finding| finding.name.clone().unwrap_or_default())
        .collect();
    let mut expected_counts: BTreeMap<String, usize> = BTreeMap::new();
    for severity in &severities {
        *expected_counts.entry((*severity).to_string()).or_default() += 1;
    }
    let expected_counts: Value = expected_counts
        .into_iter()
        .map(|(severity, count)| (severity, json!(count)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    seed_findings(&engine, repository, &digest, seeded.clone());

    let page_sizes: Vec<(&str, Option<i64>)> = vec![
        ("no page size asked for", None),
        ("one finding a page", Some(1)),
        ("two findings a page", Some(2)),
        ("a page that does not divide the list", Some(3)),
        ("exactly the list", Some(i64::try_from(seeded.len()).unwrap_or(i64::MAX))),
        ("more than the list", Some(1000)),
    ];

    for (case, max_results) in page_sizes {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request =
                json!({"repositoryName": repository, "imageId": {"imageDigest": digest}});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, case, "DescribeImageScanFindings", &request).await;
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            assert_eq!(
                at(&page, "/imageScanStatus/status"),
                json!(state::SCAN_STATUS_COMPLETE),
                "case {case:?}: the seeded scan has completed, got {:?}",
                at(&page, "/imageScanStatus")
            );
            assert_eq!(
                at(&page, "/imageScanFindings/findingSeverityCounts"),
                expected_counts,
                "case {case:?}: every page counts the whole scan's findings, got {:?}",
                at(&page, "/imageScanFindings/findingSeverityCounts")
            );
            let reported = at(&page, "/imageScanFindings/findings");
            let reported = reported.as_array().cloned().unwrap_or_default();
            if let Some(max) = max_results {
                assert!(
                    i64::try_from(reported.len()).unwrap_or(i64::MAX) <= max,
                    "case {case:?}: a page holds at most the {max} findings asked for, got {}",
                    reported.len()
                );
            }
            for entry in &reported {
                collected.push(entry["name"].as_str().unwrap_or_default().to_string());
            }
            match at(&page, "/nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, names,
            "case {case:?}: paging reports every finding once, in order, got {collected:?}"
        );
    }

    // The whole finding is reported as it is stored, member for member.
    let whole = findings(
        &router,
        "the whole finding",
        repository,
        &json!({"imageDigest": digest}),
    )
    .await;
    let first = at(&whole, "/imageScanFindings/findings/0");
    assert_eq!(
        first,
        json!({
            "name": names[0],
            "description": format!("the {} finding at index 0", severities[0]),
            "uri": "https://example.invalid/cve/2026-0000",
            "severity": severities[0],
            "attributes": [{"key": "package_name", "value": "package-0"}],
        }),
        "the finding is reported as it is stored, got {first:?}"
    );

    // A token this service never issued addresses no page.
    refused(
        &router,
        "a token that was never issued",
        "DescribeImageScanFindings",
        &json!({
            "repositoryName": repository,
            "imageId": {"imageDigest": digest},
            "nextToken": "not-a-token",
        }),
        "InvalidParameterException",
    )
    .await;
}
