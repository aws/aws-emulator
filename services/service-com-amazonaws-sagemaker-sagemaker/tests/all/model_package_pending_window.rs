// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the window `CreateModelPackage` leaves between a
//! registration and the `Completed` transition the clock owes it.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The RULE, asserted over a TABLE rather than one call: a registration with validation
//! work still to run settles a fixed span after the registering call, and that span has to
//! OUTLAST the caller's own next requests — a client that registers such a package and then
//! reads it back, through any of the operations that report its status, still observes the
//! non-terminal `Pending`, and observes `Completed` only once the due instant has passed.
//! The rows vary the kind of
//! registration (a version in a group, an unversioned package), the optional members it
//! carries, how many inference containers it declares, and WHICH reporting operation the
//! caller reads it through — one read per reader, so each row spends several round trips
//! inside the window.
//!
//! The harness and the create/describe helpers are reused from `model_package_registry`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::model_package_registry::{container, str_field, validation_specification, Emulator};

/// The narrowest registration window a caller can still observe.
///
/// One request's round trip through the router costs anything up to hundreds of
/// milliseconds on a loaded host — which is what the elapsed time the table below reports
/// measures — so a window below this is one a client's very next call can outrun, making
/// the non-terminal status unreachable for every caller rather than only for a slow one.
/// This is the floor the span is held above; the span itself is
/// `state::MODEL_PACKAGE_COMPLETED_AFTER`.
const OBSERVABLE_WINDOW_FLOOR: Duration = Duration::from_secs(1);

/// How long a poll below gives the clock to settle a registration: far longer than the
/// window itself, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often such a poll looks again. Only the loop's condition decides when it stops.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The operations that report a model package's `ModelPackageStatus`, which is what makes
/// the window observable. A versioned package is reported by all three; an unversioned one
/// has no `BatchDescribeModelPackage` summary (that operation "batch describes a list of
/// versioned model packages"), so its rows read it through the other two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reader {
    Describe,
    Batch,
    List,
}

/// An `InferenceSpecification` carrying `containers` containers.
fn inference_specification(containers: usize) -> Value {
    let containers: Vec<Value> = (0..containers)
        .map(|index| container(&format!("123.dkr.ecr.us-east-1.amazonaws.com/image{index}:1")))
        .collect();
    json!({"Containers": containers})
}

/// The `ModelPackageStatus` `reader` reports for the package `arn` names.
async fn status_through(emulator: &Emulator, case: &str, reader: Reader, arn: &str) -> String {
    match reader {
        Reader::Describe => str_field(&emulator.describe(case, arn).await, "ModelPackageStatus"),
        Reader::Batch => {
            let output = emulator
                .ok(
                    case,
                    "BatchDescribeModelPackage",
                    &json!({"ModelPackageArnList": [arn]}),
                )
                .await;
            let summary = output
                .get("ModelPackageSummaries")
                .and_then(|summaries| summaries.get(arn))
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn}, got {output}"));
            str_field(&summary, "ModelPackageStatus")
        }
        Reader::List => {
            let output = emulator
                .ok(
                    case,
                    "ListModelPackages",
                    &json!({"ModelPackageType": "Both", "SortBy": "Name", "SortOrder": "Ascending"}),
                )
                .await;
            let summary = output
                .get("ModelPackageSummaryList")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {output}"))
                .iter()
                .find(|summary| summary.get("ModelPackageArn") == Some(&json!(arn)))
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected {arn} to be listed, got {output}"));
            str_field(&summary, "ModelPackageStatus")
        }
    }
}

/// One row: how the package is registered, and which operations report its status.
struct Row {
    case: &'static str,
    /// The model package group the row's registration joins, if it registers a version.
    group: Option<&'static str>,
    /// The members the registration carries beyond its kind-selecting member.
    extras: Value,
    readers: &'static [Reader],
}

/// RULE: the `Pending` -> `Completed` transition `CreateModelPackage` owes a registration
/// with validation work still to run is due a fixed span after the registering call, and
/// the span outlasts the caller's own next requests:
/// every operation that reports `ModelPackageStatus` reports the stored `Pending` for such
/// a package read back straight after its registration, and `Completed` only once the due
/// instant has passed.
#[tokio::test]
async fn a_registration_is_reported_pending_through_every_reader_until_the_clock_owes_completed() {
    let rows = [
        Row {
            case: "a version with one inference container",
            group: Some("window-plain"),
            extras: json!({
                "InferenceSpecification": inference_specification(1),
                "ValidationSpecification": validation_specification(1),
            }),
            readers: &[Reader::Describe, Reader::Batch, Reader::List],
        },
        Row {
            case: "a version with a description, an approval decision and three containers",
            group: Some("window-described"),
            extras: json!({
                "InferenceSpecification": inference_specification(3),
                "ValidationSpecification": validation_specification(1),
                "ModelPackageDescription": "fraud detector — Détection ✓",
                "ModelApprovalStatus": "Approved",
                "ModelPackageRegistrationType": "Registered",
            }),
            readers: &[Reader::Describe, Reader::Batch, Reader::List],
        },
        Row {
            case: "an unversioned package",
            group: None,
            extras: json!({
                "ModelPackageName": "window-solo",
                "InferenceSpecification": inference_specification(1),
                "ValidationSpecification": validation_specification(1),
            }),
            readers: &[Reader::Describe, Reader::List],
        },
    ];

    let window = state::MODEL_PACKAGE_COMPLETED_AFTER;
    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let mut body = row.extras;
        if let Some(group) = row.group {
            emulator.create_group(state::DEFAULT_REGION, group).await;
            body["ModelPackageGroupName"] = json!(group);
        }

        // The clock starts at the registering call, so everything the caller does next is
        // measured against it.
        let registered_at = Instant::now();
        let arn = emulator.create(case, &body).await;

        // One read per reader, all of them inside the window: the span is wide enough that
        // a caller continuing straight into its next requests still sees the registration
        // settling.
        for reader in row.readers {
            let status = status_through(&emulator, case, *reader, &arn).await;
            let elapsed = registered_at.elapsed();
            assert_eq!(
                status, "Pending",
                "case {case:?} read through {reader:?}: expected Pending {elapsed:?} into the \
                 {window:?} registration window, got {status:?}"
            );
        }

        // Look again until the registration has settled, however long the host takes:
        // what is under test is that the transition happens, not when.
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let status = status_through(&emulator, case, row.readers[0], &arn).await;
            if status == "Completed" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the registration to settle within \
                 {SETTLE_DEADLINE:?}, got {status:?}"
            );
            tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        }

        for reader in row.readers {
            let status = status_through(&emulator, case, *reader, &arn).await;
            assert_eq!(
                status, "Completed",
                "case {case:?} read through {reader:?}: expected Completed once the \
                 {window:?} registration window has passed, got {status:?}"
            );
        }
    }
}

/// RULE: the registration window is wide enough to be observable — a span a caller's very
/// next request can spend is a span no caller ever sees the non-terminal status in.
#[test]
fn the_registration_window_is_wider_than_one_round_trip() {
    let window = state::MODEL_PACKAGE_COMPLETED_AFTER;
    assert!(
        window >= OBSERVABLE_WINDOW_FLOOR,
        "case \"the registration window\": expected at least {OBSERVABLE_WINDOW_FLOOR:?}, \
         got {window:?}"
    );
}
