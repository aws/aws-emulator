// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the `ModelPackageStatus` a registration is reported in, as
//! soon as `CreateModelPackage` answers and afterwards.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness and the create/describe
//! helpers are reused from `model_package_registry`; the window a registration with
//! validation work outstanding leaves is asserted in `model_package_pending_window`.
//!
//! The RULE, asserted over a TABLE of inputs rather than one recorded call: a model
//! package's status reports the registration work still outstanding. A
//! `ValidationSpecification` "specifies batch transform jobs that SageMaker runs to
//! validate your model package", so a registration that names a validation profile is
//! `Pending` until that work is owed complete; a registration with no validation to run —
//! because it named no specification, named an empty profile list, or asked for
//! `SkipModelValidation=All` — has nothing left to finish and is `Completed` the moment it
//! is registered, through every operation that reports the status.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::model_package_registry::{
    container, status_at_registration, str_field, validation_specification, Emulator,
};

/// How long a poll below gives the clock to settle a registration: far longer than
/// `state::MODEL_PACKAGE_COMPLETED_AFTER`, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often such a poll looks again. Only the loop's condition decides when it stops.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The group every versioned row registers into.
const GROUP: &str = "status-group";

/// A `ValidationSpecification` that names no profile at all —
/// `ModelPackageValidationSpecification$ValidationProfiles` is modelled `@length 0..1`, so
/// an empty list is a request that asks for no validation work.
fn validation_specification_without_profiles() -> Value {
    json!({
        "ValidationRole": "arn:aws:iam::000000000000:role/SageMakerValidation",
        "ValidationProfiles": [],
    })
}

/// The `ModelPackageStatus` each reader reports for the package `arn` names: the single
/// describe, the batch summary, and the listing entry.
async fn statuses_through_every_reader(
    emulator: &Emulator,
    case: &str,
    arn: &str,
) -> Vec<(&'static str, String)> {
    let described = emulator.describe(case, arn).await;
    let batch = emulator
        .ok(
            case,
            "BatchDescribeModelPackage",
            &json!({"ModelPackageArnList": [arn]}),
        )
        .await;
    let summary = batch
        .pointer("/ModelPackageSummaries")
        .and_then(|summaries| summaries.get(arn))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn}, got {batch}"));
    let listed = emulator
        .ok(
            case,
            "ListModelPackages",
            &json!({"ModelPackageGroupName": GROUP}),
        )
        .await;
    let entry = listed
        .get("ModelPackageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {listed}"))
        .iter()
        .find(|entry| entry.get("ModelPackageArn") == Some(&json!(arn)))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected {arn} to be listed, got {listed}"));

    vec![
        (
            "DescribeModelPackage",
            str_field(&described, "ModelPackageStatus"),
        ),
        (
            "BatchDescribeModelPackage",
            str_field(&summary, "ModelPackageStatus"),
        ),
        ("ListModelPackages", str_field(&entry, "ModelPackageStatus")),
    ]
}

/// One row: the members the registration carries beyond the group it joins.
struct Row {
    case: &'static str,
    extras: Value,
}

/// The rows the rule is asserted over, spanning what it generalises over: whether the
/// request names a validation specification at all, whether that specification names a
/// profile, whether `SkipModelValidation` waives the work, and how much else the
/// registration carries beside it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no validation specification",
            extras: json!({
                "InferenceSpecification": {
                    "Containers": [container("123.dkr.ecr.us-east-1.amazonaws.com/image:1")],
                },
            }),
        },
        Row {
            case: "a validation specification with no profile",
            extras: json!({
                "ValidationSpecification": validation_specification_without_profiles(),
            }),
        },
        Row {
            case: "a validation profile waived by SkipModelValidation",
            extras: json!({
                "ValidationSpecification": validation_specification(1),
                "SkipModelValidation": "All",
            }),
        },
        Row {
            case: "a description, an approval decision and three containers",
            extras: json!({
                "InferenceSpecification": {
                    "Containers": [
                        container("123.dkr.ecr.us-east-1.amazonaws.com/image:1"),
                        container("123.dkr.ecr.us-east-1.amazonaws.com/image:2"),
                        container("123.dkr.ecr.us-east-1.amazonaws.com/image:3"),
                    ],
                },
                "ModelPackageDescription": "fraud detector — Détection ✓",
                "ModelApprovalStatus": "Approved",
            }),
        },
        Row {
            case: "a validation profile still to run",
            extras: json!({
                "ValidationSpecification": validation_specification(1),
            }),
        },
    ]
}

/// RULE: a registration with no validation work to run is reported `Completed` by every
/// reader as soon as `CreateModelPackage` answers, and stays `Completed`; a registration
/// that does have validation work is reported `Pending` and reaches `Completed` once the
/// work is owed complete. Every reader reports the one stored status, so the three never
/// disagree.
#[tokio::test]
async fn a_registration_reports_the_work_it_has_outstanding_through_every_reader() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.create_group(state::DEFAULT_REGION, GROUP).await;
        let mut body = row.extras.clone();
        body["ModelPackageGroupName"] = json!(GROUP);
        let arn = emulator.create(case, &body).await;

        let expected = status_at_registration(&body);
        for (reader, status) in statuses_through_every_reader(&emulator, case, &arn).await {
            assert_eq!(
                status, expected,
                "case {case:?} read through {reader}: expected {expected} as registered, \
                 got {status:?}"
            );
        }

        // Whatever it was registered in, the status the readers report once the clock has
        // nothing left to owe is the terminal one — and a package registered complete was
        // never anything else.
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let described = emulator.describe(case, &arn).await;
            let status = str_field(&described, "ModelPackageStatus");
            if status == "Completed" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the registration to reach Completed within \
                 {SETTLE_DEADLINE:?}, got {status:?}"
            );
            tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        }
        for (reader, status) in statuses_through_every_reader(&emulator, case, &arn).await {
            assert_eq!(
                status, "Completed",
                "case {case:?} read through {reader}: expected Completed once nothing is \
                 outstanding, got {status:?}"
            );
        }
    }
}

/// RULE: the status details go with the status — a registration with no validation work
/// reports no validation item and an empty `ImageScanStatuses` list, and a registered
/// package reports a `LastModifiedTime` only once something has CHANGED it, which a
/// registration that was complete when it answered has had nothing do.
#[tokio::test]
async fn a_completed_registration_reports_no_outstanding_validation_item() {
    let rows = rows();
    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.create_group(state::DEFAULT_REGION, GROUP).await;
        let mut body = row.extras.clone();
        body["ModelPackageGroupName"] = json!(GROUP);
        let arn = emulator.create(case, &body).await;
        let described = emulator.describe(case, &arn).await;

        let profiles = body
            .pointer("/ValidationSpecification/ValidationProfiles")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let items = described
            .pointer("/ModelPackageStatusDetails/ValidationStatuses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            profiles,
            "case {case:?}: expected one validation item per profile the request named, \
             got {described}"
        );
        assert_eq!(
            described.pointer("/ModelPackageStatusDetails/ImageScanStatuses"),
            Some(&json!([])),
            "case {case:?}: expected an empty ImageScanStatuses list, got {described}"
        );
        assert!(
            described.get("LastModifiedTime").is_none(),
            "case {case:?}: nothing has modified the package yet, got {described}"
        );
    }
}
