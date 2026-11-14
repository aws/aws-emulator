// SPDX-License-Identifier: CC0-1.0

//! The image-scan status surface: WHICH `ScanStatus` a basic scan reports, and the
//! description beside it — end to end over the wire.
//!
//! The rules under test:
//!
//! 1. A scan is a run with a lifecycle, and the status reported for it is how far the
//!    clock has carried THAT run: a start call ADMITS a scan, so the run it records is
//!    queued (`PENDING`) and the start reports it so; the clock then owes it its move into
//!    the run (`IN_PROGRESS`) and then its terminal status. No caller moves it, and no
//!    read ever reports an earlier state than a read before it.
//! 2. The `description` beside a status is Amazon ECR's own wording for that state,
//!    fixed text with no part taken from the request or from the store.
//!
//! The first rule's states are exercised twice over: from a store arranged so each state
//! is read deterministically, whatever the host's speed, and over the real start path,
//! where the sequence of statuses a client polls is checked for going only forwards.
//!
//! The wire harness is `crate::wire`'s: every read goes through the router, so it
//! exercises routing, deserialization, the handlers, the persisted state and
//! serialization — the same path a client takes.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, emulator, emulator_with_engine, given_layer, ok, POLL_DEADLINE, POLL_INTERVAL,
};

use ecr::ops::types::{ImageScanFinding, ImageScanStatus};
use ecr::state;

/// The `ScanStatus` values a basic scan's run moves through, spelled out here rather than
/// imported so a wrong constant in the handler fails this suite. The spellings are the
/// model's own (`ecr#ScanStatus`), which carries the queued state beside the running one
/// precisely because a scan that has been accepted but has not begun is observable.
const PENDING: &str = "PENDING";
/// See [`PENDING`].
const IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`PENDING`].
const COMPLETE: &str = "COMPLETE";
/// See [`PENDING`].
const FAILED: &str = "FAILED";

/// The `description` Amazon ECR reports beside a run it has begun but not finished,
/// spelled out here for the same reason as [`PENDING`].
const IN_PROGRESS_DESCRIPTION: &str = "The scan is in progress.";

/// The `description` Amazon ECR reports beside a run that examined the image and
/// finished. See [`IN_PROGRESS_DESCRIPTION`].
const COMPLETE_DESCRIPTION: &str = "The scan was completed successfully.";

/// The `description` Amazon ECR reports for an image basic scanning cannot assess: one
/// fixed sentence, whose `UnsupportedImageError: ` prefix is the part a client keys on.
/// See [`IN_PROGRESS_DESCRIPTION`].
const UNSUPPORTED_IMAGE_DESCRIPTION: &str = "UnsupportedImageError: The image is not supported.";

/// Far enough ahead that a due instant set to it has NOT fallen due while the case runs,
/// and far enough behind that one set back by it HAS — so every row of the arranged table
/// reads one state and only one, whatever the host's speed.
const WELL_CLEAR: Duration = Duration::from_secs(3_600);

/// Where a reported status sits in the lifecycle a run moves through: queued, running,
/// ended. A client may miss a state between two reads, but it can never be shown an
/// earlier one than a read before it.
fn lifecycle_rank(status: &str) -> usize {
    match status {
        PENDING => 0,
        IN_PROGRESS => 1,
        COMPLETE | FAILED => 2,
        other => panic!("{other:?} is no state of a basic scan's lifecycle"),
    }
}

/// The `imageScanStatus.status` an answer reports.
fn status_of(answer: &Value) -> String {
    at(answer, "/imageScanStatus/status")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create `repository` and push a one-layer OCI image into it, answering the digest the
/// registry addresses that image by. A manifest cannot be pushed before its blobs are in
/// the repository, so the layer goes through the documented push sequence first.
async fn given_image(router: &MultiServiceRouter, case: &str, repository: &str) -> String {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let layer = given_layer(
        router,
        case,
        repository,
        format!("{repository}-layer").as_bytes(),
    )
    .await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer,
            "size": 16,
        }],
    })
    .to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({"repositoryName": repository, "imageManifest": manifest, "imageTag": "v1"}),
    )
    .await;
    state::manifest_digest(&manifest)
}

/// Read the scan of one image over the wire.
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

/// One row of the arranged-lifecycle table.
struct StateCase<'a> {
    /// What the row is about.
    case: &'a str,
    /// The repository it pushes its image into — its own, so no row depends on another.
    repository: &'a str,
    /// Whether the clock has already taken the arranged run out of the queue.
    running: bool,
    /// Whether the clock has already made the arranged run's completion due.
    ended: bool,
    /// The terminal status the arranged run came to.
    terminal: (&'a str, &'a str),
    /// What the arranged run found, as `(name, severity)` pairs.
    findings: &'a [(&'a str, &'a str)],
    /// The `imageScanStatus` the read must report for this row.
    expected: Value,
}

/// The status reported for a scan is the state of THAT run: queued, running, or the
/// terminal status it came to — each with Amazon ECR's own description beside it, and its
/// findings readable only once a run that examined the image has ended.
///
/// Every row arranges one scan record in the store the handlers read, with the instants
/// the clock owes set well clear of now, and then reads it over the wire: the state a row
/// is about is therefore the state it reads, however fast or slow the host is. The rows
/// differ along what the rule generalises over — how far through its lifecycle the run is,
/// which terminal status it came to, and whether it found anything.
#[tokio::test]
async fn the_reported_scan_status_is_the_state_of_that_run() {
    let cases = vec![
        StateCase {
            case: "a run the start call has only just admitted",
            repository: "scan-state-queued",
            running: false,
            ended: false,
            terminal: (COMPLETE, COMPLETE_DESCRIPTION),
            findings: &[],
            // A queued run has nothing to describe yet: no wording is authored for a
            // state in which nothing has happened.
            expected: json!({"status": PENDING}),
        },
        StateCase {
            case: "a run the clock has taken out of the queue",
            repository: "scan-state-running",
            running: true,
            ended: false,
            terminal: (COMPLETE, COMPLETE_DESCRIPTION),
            findings: &[],
            expected: json!({"status": IN_PROGRESS, "description": IN_PROGRESS_DESCRIPTION}),
        },
        StateCase {
            case: "a run that examined the image and finished",
            repository: "scan-state-complete",
            running: true,
            ended: true,
            terminal: (COMPLETE, COMPLETE_DESCRIPTION),
            findings: &[("CVE-2026-0001", "HIGH")],
            expected: json!({"status": COMPLETE, "description": COMPLETE_DESCRIPTION}),
        },
        StateCase {
            case: "a run whose image basic scanning cannot assess",
            repository: "scan-state-unsupported",
            running: true,
            ended: true,
            terminal: (FAILED, UNSUPPORTED_IMAGE_DESCRIPTION),
            findings: &[],
            expected: json!({"status": FAILED, "description": UNSUPPORTED_IMAGE_DESCRIPTION}),
        },
    ];

    let (router, engine) = emulator_with_engine();
    for row in cases {
        let StateCase {
            case,
            repository,
            running,
            ended,
            terminal,
            findings: found,
            expected,
        } = row;
        let digest = given_image(&router, case, repository).await;

        {
            let now = SystemTime::now();
            let store = state::service_state(&*engine).expect("the ecr store is reachable");
            let mut store = store.lock();
            let image = store
                .repositories
                .get_mut(repository)
                .expect("the repository was created over the wire")
                .images
                .get_mut(&digest)
                .expect("the image was pushed over the wire");
            image.scan = Some(state::ImageScan {
                started_at: now - WELL_CLEAR,
                running_due_at: if running {
                    now - WELL_CLEAR
                } else {
                    now + WELL_CLEAR
                },
                complete_due_at: if ended { now } else { now + WELL_CLEAR },
                terminal: ImageScanStatus {
                    status: Some(terminal.0.to_string()),
                    description: Some(terminal.1.to_string()),
                },
                findings: found
                    .iter()
                    .map(|(name, severity)| ImageScanFinding {
                        name: Some((*name).to_string()),
                        description: None,
                        uri: None,
                        severity: Some((*severity).to_string()),
                        attributes: None,
                    })
                    .collect(),
            });
        }

        let reported = findings(&router, case, repository, &digest).await;
        assert_eq!(
            at(&reported, "/imageScanStatus"),
            expected,
            "case {case:?}: expected the scan status {expected:?}, got {:?}",
            at(&reported, "/imageScanStatus")
        );

        // The findings of a run are a statement about the image it examined, so they are
        // readable exactly where such a run has ended.
        let readable = ended && terminal.0 == COMPLETE;
        let expected_findings: Value = if readable {
            found
                .iter()
                .map(|(name, _)| json!(name))
                .collect::<Vec<Value>>()
                .into()
        } else {
            Value::Null
        };
        let answered = if readable {
            at(&reported, "/imageScanFindings/findings")
                .as_array()
                .map(|findings| {
                    findings
                        .iter()
                        .map(|finding| at(finding, "/name"))
                        .collect::<Vec<Value>>()
                        .into()
                })
                .unwrap_or(Value::Null)
        } else {
            at(&reported, "/imageScanFindings")
        };
        assert_eq!(
            answered, expected_findings,
            "case {case:?}: expected the findings {expected_findings:?}, got {answered:?}"
        );

        // Reporting a run changes nothing a later caller sees: the second read is the
        // first.
        let again = findings(&router, case, repository, &digest).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read reports the same run, got {again:?}"
        );
    }
}

/// Over the real start path: a scan is QUEUED when the start admits it, and the statuses
/// a client polls afterwards only ever move forwards — queued, running, ended — with the
/// run's own terminal description at the end.
///
/// The rows differ along what the rule generalises over: how many filesystem layers the
/// scanned image declares, and therefore how much content the run was started on. The
/// wait is for the CONDITION, not for a span of wall clock: the poll looks again until
/// the run has ended.
#[tokio::test]
async fn a_started_scan_is_queued_and_its_reported_status_only_moves_forwards() {
    let cases: Vec<(&str, &str)> = vec![
        ("a one-layer image", "scan-progress-one"),
        ("another image of the same registry", "scan-progress-two"),
    ];

    let router = emulator();
    for (case, repository) in cases {
        let digest = given_image(&router, case, repository).await;
        let started = ok(
            &router,
            case,
            "StartImageScan",
            &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
        )
        .await;
        assert_eq!(
            at(&started, "/imageScanStatus"),
            json!({"status": PENDING}),
            "case {case:?}: the start admits a queued run, got {:?}",
            at(&started, "/imageScanStatus")
        );

        let mut observed: Vec<String> = vec![status_of(&started)];
        let since = Instant::now();
        loop {
            let answer = findings(&router, case, repository, &digest).await;
            let status = status_of(&answer);
            let previous = observed.last().cloned().unwrap_or_default();
            assert!(
                lifecycle_rank(&status) >= lifecycle_rank(&previous),
                "case {case:?}: the run went backwards from {previous:?} to {status:?}"
            );
            if lifecycle_rank(&status) < lifecycle_rank(COMPLETE) {
                assert_eq!(
                    at(&answer, "/imageScanFindings"),
                    Value::Null,
                    "case {case:?}: a run that has not ended reports no findings, got {:?}",
                    at(&answer, "/imageScanFindings")
                );
            }
            observed.push(status.clone());
            if lifecycle_rank(&status) == lifecycle_rank(FAILED) {
                // These layers are no filesystem archive, so the run could identify no
                // operating system to report on: the one terminal status it can reach is
                // the unsupported-image one, with ECR's own sentence for it.
                assert_eq!(
                    at(&answer, "/imageScanStatus"),
                    json!({
                        "status": FAILED,
                        "description": UNSUPPORTED_IMAGE_DESCRIPTION,
                    }),
                    "case {case:?}: expected the unsupported-image outcome, got {:?}",
                    at(&answer, "/imageScanStatus")
                );
                break;
            }
            assert!(
                since.elapsed() < POLL_DEADLINE,
                "case {case:?}: the run never ended; the statuses polled were {observed:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        assert_eq!(
            observed.first().map(String::as_str),
            Some(PENDING),
            "case {case:?}: the first state a client sees is the queued one, got \
             {observed:?}"
        );
    }
}
