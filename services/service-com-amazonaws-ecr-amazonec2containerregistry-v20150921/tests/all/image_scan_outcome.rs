// SPDX-License-Identifier: CC0-1.0

//! Where a basic image scan ENDS, and how the two scan refusals word themselves — end to
//! end over the wire.
//!
//! The rules under test:
//!
//! 1. A scan's terminal status is the outcome of examining the image it was started on: it
//!    reaches `COMPLETE`, the status whose findings are a statement about the image, only
//!    where the run identified the operating system and package manager whose packages it
//!    reports on, and otherwise ends `FAILED` with the unsupported-image description. The
//!    images here are built on layers whose bytes are no filesystem archive, so no
//!    operating system is identifiable in them and no scan of one ends in a clean success
//!    a CI gate would read as a pass. (The other direction — a filesystem a scan does
//!    identify — is `crate::image_scan_filesystem`'s.)
//! 2. `ImageArchivedException` and `ScanNotFoundException` interpolate the request: the
//!    `imageId` as it was sent, the repository it named and the registry that holds it.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it exercises
//! routing, deserialization, the handlers, the persisted state and serialization.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, call, emulator, emulator_with_engine, given_layer, ok, ok_until, REGISTRY_ID};

use ecr::ops::types::ImageScanStatus;
use ecr::state;

/// The `description` Amazon ECR reports beside the terminal status of a run whose image it
/// could not scan, spelled out here rather than imported so a change to the constant the
/// handler reads fails this suite.
const UNSUPPORTED_IMAGE_DESCRIPTION: &str = "UnsupportedImageError: The image is not supported.";

/// The manifest member each image form declares its filesystem layers in: the Docker image
/// manifest V2 schema 2 and the OCI image manifest use `layers` (descriptors), the older V2
/// schema 1 `fsLayers` (`blobSum` digests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// A Docker image manifest V2 schema 2.
    DockerV2,
    /// An OCI image manifest.
    Oci,
    /// A Docker image manifest V2 schema 1.
    DockerV1,
}

/// One row of the outcome table.
struct ScanCase<'a> {
    /// What the row is about.
    case: &'a str,
    /// The repository it pushes its image into — its own, so no row depends on another.
    repository: &'a str,
    /// The image it pushes there.
    image: Spec<'a>,
}

/// The image one case pushes: which schema it is spelled in, how much it declares, and
/// how it is addressable.
struct Spec<'a> {
    /// Makes this image's blobs distinct from another's in the same repository: a blob is
    /// addressed by the digest of its bytes.
    salt: &'a str,
    /// The manifest form the image is pushed in.
    form: Form,
    /// How many filesystem layers that manifest declares.
    layers: usize,
    /// Whether the manifest carries an image configuration descriptor.
    configured: bool,
    /// The tag it is pushed under, when a case addresses it by one.
    tag: Option<&'a str>,
}

/// Push `spec`'s image into `repository`, uploading every blob it references first, and
/// answer the digest the registry addresses it by.
async fn given_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    spec: &Spec<'_>,
) -> String {
    let &Spec {
        salt,
        form,
        layers,
        configured,
        tag,
    } = spec;
    let mut digests: Vec<String> = Vec::new();
    for index in 0..layers {
        let content = format!("{repository}-{salt}-layer-{index}").into_bytes();
        digests.push(given_layer(router, case, repository, &content).await);
    }
    let manifest = match form {
        Form::DockerV1 => json!({
            "schemaVersion": 1,
            "name": repository,
            "fsLayers": digests
                .iter()
                .map(|digest| json!({"blobSum": digest}))
                .collect::<Vec<Value>>(),
        }),
        _ => {
            let (manifest_media, config_media, layer_media) = if form == Form::Oci {
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
            let mut document = json!({
                "schemaVersion": 2,
                "mediaType": manifest_media,
                "layers": digests
                    .iter()
                    .map(|digest| json!({"mediaType": layer_media, "digest": digest, "size": 16}))
                    .collect::<Vec<Value>>(),
            });
            if configured {
                let content = format!("{repository}-{salt}-config").into_bytes();
                let config = given_layer(router, case, repository, &content).await;
                document["config"] =
                    json!({"mediaType": config_media, "digest": config, "size": content.len()});
            }
            document
        }
    };
    let manifest = manifest.to_string();
    let mut request = json!({"repositoryName": repository, "imageManifest": manifest});
    if let Some(tag) = tag {
        request["imageTag"] = json!(tag);
    }
    ok(router, case, "PutImage", &request).await;
    state::manifest_digest(&manifest)
}

/// Read the scan of one image.
async fn findings(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Value {
    ok(
        router,
        case,
        "DescribeImageScanFindings",
        &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
    )
    .await
}

/// A started scan ends at the outcome of examining its image: for an image whose operating
/// system nothing identified, the unsupported-image status with no findings block — never a
/// clean `COMPLETE`.
///
/// The rows differ along what the rule generalises over: the manifest form the image was
/// pushed in, how many filesystem layers it declares, and whether it carries an image
/// configuration descriptor. The report's own case is a single-layer image with a
/// configuration and no operating system.
#[tokio::test]
async fn a_scan_ends_at_the_outcome_of_examining_its_image() {
    let cases = vec![
        ScanCase {
            case: "a single-layer docker v2 image with a configuration",
            repository: "outcome-docker-one",
            image: Spec {
                salt: "one",
                form: Form::DockerV2,
                layers: 1,
                configured: true,
                tag: None,
            },
        },
        ScanCase {
            case: "a three-layer docker v2 image",
            repository: "outcome-docker-three",
            image: Spec {
                salt: "three",
                form: Form::DockerV2,
                layers: 3,
                configured: true,
                tag: None,
            },
        },
        ScanCase {
            case: "an OCI image with no configuration descriptor",
            repository: "outcome-oci-bare",
            image: Spec {
                salt: "bare",
                form: Form::Oci,
                layers: 2,
                configured: false,
                tag: Some("bare"),
            },
        },
        ScanCase {
            case: "a V2 schema 1 image, whose layers are fsLayers",
            repository: "outcome-schema-one",
            image: Spec {
                salt: "schema-one",
                form: Form::DockerV1,
                layers: 1,
                configured: false,
                tag: None,
            },
        },
    ];

    let expected = json!({
        "status": state::SCAN_STATUS_FAILED,
        "description": UNSUPPORTED_IMAGE_DESCRIPTION,
    });

    for ScanCase {
        case,
        repository,
        image,
    } in cases
    {
        let router = emulator();
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let digest = given_image(&router, case, repository, &image).await;

        let started = ok(
            &router,
            case,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
        )
        .await;
        assert_eq!(
            at(&started, "/imageScanStatus/status"),
            json!(state::SCAN_STATUS_PENDING),
            "case {case:?}: the run is queued when it is admitted, got {:?}",
            at(&started, "/imageScanStatus")
        );

        // The terminal status is the clock's to owe, so the suite looks again until the
        // run has ended rather than waiting out a span of wall clock. The poll waits for
        // TERMINALITY alone, so the status it ended at is judged by the assertion below.
        let ended = ok_until(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
            |answer| {
                [state::SCAN_STATUS_COMPLETE, state::SCAN_STATUS_FAILED]
                    .iter()
                    .any(|status| at(answer, "/imageScanStatus/status") == json!(status))
            },
        )
        .await;
        assert_eq!(
            at(&ended, "/imageScanStatus"),
            expected,
            "case {case:?}: expected the terminal status {expected:?}, got {:?}",
            at(&ended, "/imageScanStatus")
        );
        assert_eq!(
            at(&ended, "/imageScanFindings"),
            Value::Null,
            "case {case:?}: a run that examined nothing reports no findings, got {:?}",
            at(&ended, "/imageScanFindings")
        );
        // The terminal status is the record's own, so reading it again answers the same
        // thing and the read wrote nothing.
        let again = findings(&router, case, repository, &digest).await;
        assert_eq!(
            again, ended,
            "case {case:?}: a second read reports the same run, got {again:?}"
        );
    }
}

/// The other direction of the same rule: a run that DID examine its image reports the
/// status whose findings are readable, and the read reports the findings of that run.
///
/// The findings of a run are placed in the same store the handlers read, because no
/// vulnerability source is reachable from this process and so no started run reports a
/// finding of its own — which is also what shows the status and the findings are the
/// RECORD's and not a constant either way.
#[tokio::test]
async fn a_run_that_examined_its_image_reports_the_status_whose_findings_are_readable() {
    let (router, engine) = emulator_with_engine();
    let repository = "examined";
    let case = "a run that examined the image";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let digest = given_image(
        &router,
        case,
        repository,
        &Spec {
            salt: "examined",
            form: Form::Oci,
            layers: 1,
            configured: true,
            tag: None,
        },
    )
    .await;
    ok(
        &router,
        case,
        "StartImageScan",
        &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
    )
    .await;

    {
        let state = state::service_state(&*engine).expect("the ecr store is reachable");
        let mut store = state.lock();
        let image = store
            .repositories
            .get_mut(repository)
            .expect("the repository was created over the wire")
            .images
            .get_mut(&digest)
            .expect("the image was pushed over the wire");
        let scan = image.scan.as_mut().expect("the scan was started over the wire");
        scan.running_due_at = SystemTime::now();
        scan.complete_due_at = SystemTime::now();
        scan.terminal = ImageScanStatus {
            status: Some(state::SCAN_STATUS_COMPLETE.to_string()),
            description: Some(state::SCAN_COMPLETE_DESCRIPTION.to_string()),
        };
    }

    let ended = findings(&router, case, repository, &digest).await;
    assert_eq!(
        at(&ended, "/imageScanStatus"),
        json!({
            "status": state::SCAN_STATUS_COMPLETE,
            "description": state::SCAN_COMPLETE_DESCRIPTION,
        }),
        "case {case:?}: expected the run's own terminal status, got {:?}",
        at(&ended, "/imageScanStatus")
    );
    assert_eq!(
        at(&ended, "/imageScanFindings/findings"),
        json!([]),
        "case {case:?}: the findings of that run are readable, got {:?}",
        at(&ended, "/imageScanFindings/findings")
    );
    assert!(
        at(&ended, "/imageScanFindings/imageScanCompletedAt").is_number(),
        "case {case:?}: a run that completed reports when it did, got {:?}",
        at(&ended, "/imageScanFindings/imageScanCompletedAt")
    );
}

/// Both scan refusals are built from the request: the identifier exactly as it was sent —
/// an absent member as `null` — the repository it named, and the registry that holds it.
///
/// The rows differ along which members the identifier carries (a digest, a tag, both) and
/// which repository is named, because those are what the two sentences interpolate. The
/// report's own cases are the digest-only rows.
#[tokio::test]
async fn the_archived_and_missing_scan_refusals_interpolate_the_request() {
    let router = emulator();
    let repository = "refusal-messages";
    let case = "seed";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    // One image that stays in standard storage and carries no scan, and one in Amazon ECR
    // archive, so both refusals are reachable from the same repository. Each carries a tag,
    // so a row may address it by tag as well as by digest.
    let unscanned = given_image(
        &router,
        case,
        repository,
        &Spec {
            salt: "unscanned",
            form: Form::DockerV2,
            layers: 1,
            configured: true,
            tag: Some("warm"),
        },
    )
    .await;
    let archived = given_image(
        &router,
        case,
        repository,
        &Spec {
            salt: "archived",
            form: Form::Oci,
            layers: 1,
            configured: true,
            tag: Some("cold"),
        },
    )
    .await;
    ok(
        &router,
        case,
        "UpdateImageStorageClass",
        &json!({
            "repositoryName": repository,
            "imageId": {"imageDigest": archived},
            "targetStorageClass": "ARCHIVE",
        }),
    )
    .await;

    // The three forms an `ImageIdentifier` takes, each resolving to the image it names:
    // (case, what the request sends, the digest and the tag the message must spell).
    let identifiers = |digest: &str, tag: &str| -> Vec<(String, Value, String, String)> {
        vec![
            (
                "by digest alone".to_string(),
                json!({"imageDigest": digest}),
                digest.to_string(),
                "null".to_string(),
            ),
            (
                "by digest and tag together".to_string(),
                json!({"imageDigest": digest, "imageTag": tag}),
                digest.to_string(),
                tag.to_string(),
            ),
            (
                "by tag alone".to_string(),
                json!({"imageTag": tag}),
                "null".to_string(),
                tag.to_string(),
            ),
        ]
    };

    for (case, image_id, digest, tag) in identifiers(&archived, "cold") {
        let answer = call(
            &router,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": image_id}),
        )
        .await;
        assert_eq!(
            answer.error_code(),
            "ImageArchivedException",
            "case {case:?}: expected ImageArchivedException, got {:?}",
            answer.body
        );
        let expected = format!(
            "The image with imageId {{imageDigest:'{digest}', imageTag:'{tag}'}} is \
             archived within the repository with name '{repository}' in the registry with \
             id '{REGISTRY_ID}'"
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.message()
        );
    }

    for (case, image_id, digest, tag) in identifiers(&unscanned, "warm") {
        let answer = call(
            &router,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": image_id}),
        )
        .await;
        assert_eq!(
            answer.error_code(),
            "ScanNotFoundException",
            "case {case:?}: expected ScanNotFoundException, got {:?}",
            answer.body
        );
        let expected = format!(
            "Image scan does not exist for the image with '{{imageDigest:'{digest}', \
             imageTag:'{tag}'}}' in the repository with name '{repository}' in the \
             registry with id '{REGISTRY_ID}'"
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.message()
        );
    }
}
