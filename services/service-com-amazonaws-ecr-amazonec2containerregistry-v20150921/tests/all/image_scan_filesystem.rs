// SPDX-License-Identifier: CC0-1.0

//! What a basic image scan makes of the image's FILESYSTEM — end to end over the wire.
//!
//! The rule under test: a basic scan's terminal status is a function of the scanned
//! image's content. A run reaches `COMPLETE` — the status whose findings block, with its
//! completion instant and its per-severity counts, is readable — exactly where the layers
//! the image's manifest declares carry, between them, an operating-system release file
//! and the installed-package database of a package manager; an image missing either is
//! one basic scanning does not support, and the run ends `FAILED` with the
//! unsupported-image description and no findings block. Both outcomes are reachable, and
//! neither is the default.
//!
//! The rows differ along what the rule generalises over: which release file and which
//! package database the filesystem carries, how the layer's bytes are encoded (gzipped
//! tar, plain tar, bytes that are no archive), whether the two sit in one layer or in
//! different ones, and whether the image carries one of the two or neither. The report's
//! own case is the first row.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it
//! exercises routing, deserialization, the handlers, the persisted state and
//! serialization — the same path a client takes.

use std::io::Write;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, emulator, given_layer, ok, ok_until};

use ecr::state;

/// The `description` Amazon ECR reports beside the terminal status of a run whose image it
/// could not scan, spelled out here rather than imported so a change to the constant the
/// handler reads fails this suite.
const UNSUPPORTED_IMAGE_DESCRIPTION: &str = "UnsupportedImageError: The image is not supported.";

/// The `description` of a run that examined the image and finished, spelled out here for
/// the same reason as [`UNSUPPORTED_IMAGE_DESCRIPTION`].
const COMPLETE_DESCRIPTION: &str = "The scan was completed successfully.";

/// An `os-release` file identifying Alpine Linux, in the format of the os-release
/// specification the file's own `ID=` field comes from.
const ALPINE_OS_RELEASE: &str =
    "NAME=\"Alpine Linux\"\nID=alpine\nVERSION_ID=3.18.4\nPRETTY_NAME=\"Alpine Linux v3.18\"\n";

/// The files one test layer carries: a path inside the image's filesystem, and the
/// content at it.
type Files<'a> = &'a [(&'a str, &'a str)];

/// One layer of a test image, in the form the registry receives its bytes in.
enum Layer<'a> {
    /// A gzip-compressed tar: what both image formats' `...tar+gzip` / `...tar.gzip`
    /// layer media types carry.
    Gzipped(Files<'a>),
    /// A plain tar, which an image declaring an uncompressed layer pushes.
    Plain(Files<'a>),
    /// Bytes that are not an archive at all.
    Opaque(&'a str),
}

impl Layer<'_> {
    /// The bytes a client uploads for this layer.
    fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Gzipped(files) => {
                let mut encoder = ::flate2::write::GzEncoder::new(
                    Vec::new(),
                    ::flate2::Compression::default(),
                );
                encoder
                    .write_all(&tar_bytes(files))
                    .expect("the test layer compresses");
                encoder.finish().expect("the test layer compresses")
            }
            Self::Plain(files) => tar_bytes(files),
            Self::Opaque(text) => text.as_bytes().to_vec(),
        }
    }
}

/// A tar archive carrying `files`, each as a regular member at its path.
fn tar_bytes(files: Files<'_>) -> Vec<u8> {
    let mut builder = ::tar::Builder::new(Vec::new());
    for (path, content) in files {
        let mut header = ::tar::Header::new_gnu();
        header.set_size(u64::try_from(content.len()).expect("a test file fits in a u64"));
        header.set_mode(0o644);
        builder
            .append_data(&mut header, path, content.as_bytes())
            .expect("the test archive takes the entry");
    }
    builder.into_inner().expect("the test archive closes")
}

/// One row of the filesystem table.
struct Case<'a> {
    /// What the row is about.
    case: &'a str,
    /// The repository it pushes its image into — its own, so no row depends on another.
    repository: &'a str,
    /// The layers the image's manifest declares, in order.
    layers: Vec<Layer<'a>>,
    /// Whether a basic scan of that filesystem reaches the status whose findings are
    /// readable.
    complete: bool,
}

/// Push a Docker image manifest V2 schema 2 image built on `layers` into `repository`,
/// uploading every blob it references first — a manifest cannot be pushed before its
/// blobs are in the repository — and answer the digest the registry addresses it by.
async fn given_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    layers: &[Layer<'_>],
) -> String {
    let mut descriptors: Vec<Value> = Vec::new();
    for layer in layers {
        let bytes = layer.bytes();
        let digest = given_layer(router, case, repository, &bytes).await;
        descriptors.push(json!({
            "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
            "digest": digest,
            "size": bytes.len(),
        }));
    }
    let config = format!("{{\"architecture\":\"amd64\",\"os\":\"linux\",\"rootfs\":\"{repository}\"}}");
    let config_digest = given_layer(router, case, repository, config.as_bytes()).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "config": {
            "mediaType": "application/vnd.docker.container.image.v1+json",
            "digest": config_digest,
            "size": config.len(),
        },
        "layers": descriptors,
    })
    .to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({"repositoryName": repository, "imageManifest": manifest}),
    )
    .await;
    state::manifest_digest(&manifest)
}

/// A scan settles at `COMPLETE` for an image whose filesystem it could examine and at the
/// unsupported-image outcome for one it could not — so the terminal status, and the
/// findings block that goes with the first, follow from the image's content.
#[tokio::test]
async fn the_terminal_scan_status_follows_the_scanned_filesystem() {
    let cases = vec![
        Case {
            case: "an alpine filesystem: an os-release file and an apk database",
            repository: "fs-alpine",
            layers: vec![Layer::Gzipped(&[
                ("etc/os-release", ALPINE_OS_RELEASE),
                ("lib/apk/db/installed", ""),
                ("marker.txt", "pushed by the suite"),
            ])],
            complete: true,
        },
        Case {
            case: "a debian filesystem: dpkg's installed-package status file",
            repository: "fs-debian",
            layers: vec![Layer::Gzipped(&[
                (
                    "etc/os-release",
                    "NAME=\"Debian GNU/Linux\"\nID=debian\nVERSION_ID=\"12\"\n",
                ),
                (
                    "var/lib/dpkg/status",
                    "Package: bash\nStatus: install ok installed\nVersion: 5.2.15-2\n\n",
                ),
            ])],
            complete: true,
        },
        Case {
            case: "an rpm database at the location newer distributions moved it to",
            repository: "fs-rpm-sysimage",
            layers: vec![Layer::Gzipped(&[
                ("etc/os-release", "NAME=\"Amazon Linux\"\nID=amzn\nVERSION_ID=\"2023\"\n"),
                ("usr/lib/sysimage/rpm/rpmdb.sqlite", "rpm database"),
            ])],
            complete: true,
        },
        Case {
            case: "a release file of the older per-distribution kind, beside an rpm database",
            repository: "fs-redhat-release",
            layers: vec![Layer::Gzipped(&[
                (
                    "etc/redhat-release",
                    "Red Hat Enterprise Linux release 9.3 (Plow)\n",
                ),
                ("var/lib/rpm/Packages", "rpm database"),
            ])],
            complete: true,
        },
        Case {
            case: "an os-release file that carries a NAME but no ID",
            repository: "fs-name-only",
            layers: vec![Layer::Gzipped(&[
                ("usr/lib/os-release", "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\n"),
                ("var/lib/dpkg/status", "Package: dash\nVersion: 0.5.12-2\n\n"),
            ])],
            complete: true,
        },
        Case {
            case: "entry names written with the leading ./ one tool emits",
            repository: "fs-dot-slash",
            layers: vec![Layer::Gzipped(&[
                ("./etc/os-release", ALPINE_OS_RELEASE),
                ("./lib/apk/db/installed", ""),
            ])],
            complete: true,
        },
        Case {
            case: "an uncompressed tar layer",
            repository: "fs-plain-tar",
            layers: vec![Layer::Plain(&[
                ("etc/os-release", ALPINE_OS_RELEASE),
                ("lib/apk/db/installed", "the apk database"),
            ])],
            complete: true,
        },
        Case {
            case: "the release file and the package database in different layers",
            repository: "fs-split-layers",
            layers: vec![
                Layer::Gzipped(&[("etc/os-release", "ID=debian\nVERSION_ID=\"12\"\n")]),
                Layer::Gzipped(&[(
                    "var/lib/dpkg/status",
                    "Package: coreutils\nVersion: 9.1-1\n\n",
                )]),
            ],
            complete: true,
        },
        Case {
            case: "a filesystem with a release file but no package database",
            repository: "fs-no-package-database",
            layers: vec![Layer::Gzipped(&[
                ("etc/os-release", ALPINE_OS_RELEASE),
                ("usr/bin/app", "the application this image ships"),
            ])],
            complete: false,
        },
        Case {
            case: "a filesystem with a package database but no release file",
            repository: "fs-no-release-file",
            layers: vec![Layer::Gzipped(&[
                ("lib/apk/db/installed", ""),
                ("usr/bin/app", "the application this image ships"),
            ])],
            complete: false,
        },
        Case {
            case: "a file named like the rpm database directory, which is not one",
            repository: "fs-rpm-not-a-directory",
            layers: vec![Layer::Gzipped(&[
                ("etc/os-release", "ID=rhel\nVERSION_ID=\"9.3\"\n"),
                ("var/lib/rpm", "not the database directory"),
            ])],
            complete: false,
        },
        Case {
            case: "a filesystem carrying neither a release file nor a package database",
            repository: "fs-scratch",
            layers: vec![Layer::Gzipped(&[
                ("usr/bin/app", "the application this image ships"),
                ("etc/hostname", "builder\n"),
            ])],
            complete: false,
        },
        Case {
            case: "layer bytes that are not an archive at all",
            repository: "fs-not-an-archive",
            layers: vec![Layer::Opaque("these bytes are no tar archive")],
            complete: false,
        },
    ];

    let router = emulator();
    // Every row's scan is started before any is read, so the runs the clock owes proceed
    // together rather than one row at a time.
    let mut started_rows: Vec<(&str, &str, String, bool)> = Vec::new();
    for Case {
        case,
        repository,
        layers,
        complete,
    } in &cases
    {
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let digest = given_image(&router, case, repository, layers).await;
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
        started_rows.push((case, repository, digest, *complete));
    }

    for (case, repository, digest, complete) in started_rows {
        let expected_status = if complete {
            json!({
                "status": state::SCAN_STATUS_COMPLETE,
                "description": COMPLETE_DESCRIPTION,
            })
        } else {
            json!({
                "status": state::SCAN_STATUS_FAILED,
                "description": UNSUPPORTED_IMAGE_DESCRIPTION,
            })
        };
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
            expected_status,
            "case {case:?}: expected the terminal status {expected_status:?}, got {:?}",
            at(&ended, "/imageScanStatus")
        );
        if complete {
            // What `COMPLETE` guarantees: the findings block, with the instant the run
            // completed and the counts over its findings.
            assert!(
                at(&ended, "/imageScanFindings/imageScanCompletedAt").is_number(),
                "case {case:?}: a completed run reports when it completed, got {:?}",
                at(&ended, "/imageScanFindings/imageScanCompletedAt")
            );
            assert_eq!(
                at(&ended, "/imageScanFindings/findingSeverityCounts"),
                json!({}),
                "case {case:?}: the counts are over the findings the run reports, got {:?}",
                at(&ended, "/imageScanFindings/findingSeverityCounts")
            );
            assert_eq!(
                at(&ended, "/imageScanFindings/findings"),
                json!([]),
                "case {case:?}: no vulnerability is reported for a package nothing looked \
                 up, got {:?}",
                at(&ended, "/imageScanFindings/findings")
            );
        } else {
            assert_eq!(
                at(&ended, "/imageScanFindings"),
                Value::Null,
                "case {case:?}: a run that examined no filesystem reports no findings, \
                 got {:?}",
                at(&ended, "/imageScanFindings")
            );
        }
        // The outcome is the stored run's, so reading it again answers the same thing:
        // the read wrote nothing a later caller sees.
        let again = ok(
            &router,
            case,
            "DescribeImageScanFindings",
            &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
        )
        .await;
        assert_eq!(
            again, ended,
            "case {case:?}: a second read reports the same run, got {again:?}"
        );
    }
}
