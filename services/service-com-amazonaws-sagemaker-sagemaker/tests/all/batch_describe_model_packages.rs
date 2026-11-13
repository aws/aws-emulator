// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `BatchDescribeModelPackage`: the per-item batch read over
//! the versioned model packages `CreateModelPackage` registered.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert the RULE over a TABLE of inputs rather than one recorded call:
//! each requested ARN is resolved on its own and lands in exactly one of the two output
//! maps — `ModelPackageSummaries` for a version the account holds, projected from the
//! same state `DescribeModelPackage` replays, and `BatchDescribeModelPackageErrorMap`
//! for an ARN that has no summary — so the request itself always answers 200. The rows
//! vary the members a version carries (description, including non-ASCII and the modelled
//! maximum length; approval status; registration type; how many inference containers),
//! the batch's composition (found, unknown, unversioned, duplicated), its size at each
//! modelled bound, and whether the `Pending` -> `Completed` transition the clock owes is
//! due yet.
//!
//! A versioned package needs its group to exist first, so every case creates it through
//! `CreateModelPackageGroup` over the same wire. The harness and the
//! create/describe helpers are reused from `model_package_registry`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state;

use crate::model_package_registry::{
    container, error_code, expected_arn, str_field, Emulator, PAST_COMPLETED_DUE,
};

/// The members a `BatchDescribeModelPackageSummary` carries
/// (`smithy_query(query="shape",shape="BatchDescribeModelPackageSummary")`). Every one of
/// them is also a `DescribeModelPackageOutput` member, which is what lets a case assert
/// the summary is a projection of the described package rather than a second copy of it.
const SUMMARY_MEMBERS: &[&str] = &[
    "ModelPackageGroupName",
    "ModelPackageVersion",
    "ModelPackageArn",
    "ModelPackageDescription",
    "CreationTime",
    "InferenceSpecification",
    "ModelPackageStatus",
    "ModelApprovalStatus",
    "ModelPackageRegistrationType",
];

/// An `InferenceSpecification` carrying `containers` containers — the member a summary
/// reports, so every registered version in these tests carries one.
fn inference_specification(containers: usize) -> Value {
    let containers: Vec<Value> = (0..containers)
        .map(|index| container(&format!("123.dkr.ecr.us-east-1.amazonaws.com/image{index}:1")))
        .collect();
    json!({"Containers": containers})
}

/// Batch describe `arns` through the shared harness, requiring the request itself to
/// answer 200. A free function rather than a method, so `model_package_registry`'s
/// `Emulator` stays the one harness every model-package suite shares.
async fn batch_describe(emulator: &Emulator, case: &str, arns: &[String]) -> Value {
    emulator
        .ok(
            case,
            "BatchDescribeModelPackage",
            &json!({"ModelPackageArnList": arns}),
        )
        .await
}

/// The ARN form every model package carries, in the region these cases sign for.
fn package_arn(resource: &str) -> String {
    expected_arn(state::DEFAULT_REGION, resource)
}

/// `ModelPackageSummaries`, treating an omitted map as empty.
fn summaries(output: &Value) -> serde_json::Map<String, Value> {
    map_member(output, "ModelPackageSummaries")
}

/// `BatchDescribeModelPackageErrorMap`, treating an omitted map as empty.
fn item_errors(output: &Value) -> serde_json::Map<String, Value> {
    map_member(output, "BatchDescribeModelPackageErrorMap")
}

fn map_member(output: &Value, member: &str) -> serde_json::Map<String, Value> {
    output
        .get(member)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// One row of the projection table: how a version is registered, and what the summary
/// must then report for it.
struct Row {
    case: &'static str,
    /// The members the registration carries beyond `ModelPackageGroupName`.
    extras: Value,
    /// The description the summary must report, or `None` when the row registers none.
    description: Option<String>,
    /// The approval status the summary must report. `PendingManualApproval` is what a
    /// versioned registration that names none defaults to.
    approval: &'static str,
    /// The registration type the summary must report. `Registered` is what a package
    /// `CreateModelPackage` put in the registry carries when the request names none.
    registration: Option<&'static str>,
    /// How many containers the row's `InferenceSpecification` carries.
    containers: usize,
}

/// RULE: `BatchDescribeModelPackage` answers 200 with one `ModelPackageSummaries` entry
/// per requested ARN a versioned model package answers to, each projecting that stored
/// package's group name, version, ARN, description, creation time, inference
/// specification, status, approval status and registration type — the same values the
/// single-resource read reports — and the read leaves the package unchanged, so a second
/// identical batch answers identically.
#[tokio::test]
async fn summaries_project_each_requested_version_as_described() {
    let emulator = Emulator::new();
    let group = "projection";
    emulator.create_group(state::DEFAULT_REGION, group).await;

    // The modelled maximum of `EntityDescription` (`len 0..1024`), from
    // `smithy_query(query="shape",shape="EntityDescription")`.
    let longest_description = "d".repeat(1024);
    let rows = vec![
        Row {
            case: "no optional member beyond the inference specification",
            extras: json!({"InferenceSpecification": inference_specification(1)}),
            description: None,
            approval: "PendingManualApproval",
            registration: Some("Registered"),
            containers: 1,
        },
        Row {
            case: "ASCII description, approved, registered",
            extras: json!({
                "InferenceSpecification": inference_specification(2),
                "ModelPackageDescription": "fraud detector, production candidate",
                "ModelApprovalStatus": "Approved",
                "ModelPackageRegistrationType": "Registered",
            }),
            description: Some("fraud detector, production candidate".to_string()),
            approval: "Approved",
            registration: Some("Registered"),
            containers: 2,
        },
        Row {
            case: "non-ASCII description, rejected, logged",
            extras: json!({
                "InferenceSpecification": inference_specification(3),
                "ModelPackageDescription": "Détection ✓ (naïve) — 日本語 ∑ 1024",
                "ModelApprovalStatus": "Rejected",
                "ModelPackageRegistrationType": "Logged",
            }),
            description: Some("Détection ✓ (naïve) — 日本語 ∑ 1024".to_string()),
            approval: "Rejected",
            registration: Some("Logged"),
            containers: 3,
        },
        Row {
            case: "empty description, the lower bound of EntityDescription",
            extras: json!({
                "InferenceSpecification": inference_specification(1),
                "ModelPackageDescription": "",
            }),
            description: Some(String::new()),
            approval: "PendingManualApproval",
            registration: Some("Registered"),
            containers: 1,
        },
        Row {
            case: "description at the modelled 1024-character maximum",
            extras: json!({
                "InferenceSpecification": inference_specification(1),
                "ModelPackageDescription": longest_description,
            }),
            description: Some(longest_description.clone()),
            approval: "PendingManualApproval",
            registration: Some("Registered"),
            containers: 1,
        },
    ];

    let mut arns = Vec::new();
    for row in &rows {
        let mut body = json!({"ModelPackageGroupName": group});
        for (key, value) in row.extras.as_object().expect("the row's extras are members") {
            body[key] = value.clone();
        }
        arns.push(emulator.create(row.case, &body).await);
    }

    tokio::time::sleep(PAST_COMPLETED_DUE).await;

    let output = batch_describe(&emulator, "every registered version", &arns).await;
    let found = summaries(&output);
    assert!(
        item_errors(&output).is_empty(),
        "case \"every registered version\": no requested ARN is in error, got {output}"
    );
    assert_eq!(
        found.len(),
        rows.len(),
        "case \"every registered version\": expected one summary per requested ARN, got {output}"
    );

    for (index, (row, arn)) in rows.iter().zip(&arns).enumerate() {
        let case = row.case;
        let version = i64::try_from(index + 1).expect("the row index fits an i64");
        let summary = found
            .get(arn)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a summary keyed by {arn}"));

        assert_eq!(
            str_field(&summary, "ModelPackageGroupName"),
            group,
            "case {case:?}: expected the group the version belongs to, got {summary}"
        );
        assert_eq!(
            summary.get("ModelPackageVersion").and_then(Value::as_i64),
            Some(version),
            "case {case:?}: expected version {version}, got {summary}"
        );
        assert_eq!(
            str_field(&summary, "ModelPackageArn"),
            package_arn(&format!("{group}/{version}")),
            "case {case:?}: expected the package's own ARN, got {summary}"
        );
        assert_eq!(
            summary
                .get("ModelPackageDescription")
                .and_then(Value::as_str)
                .map(str::to_string),
            row.description,
            "case {case:?}: expected the description the request carried, got {summary}"
        );
        assert_eq!(
            str_field(&summary, "ModelApprovalStatus"),
            row.approval,
            "case {case:?}: expected the approval status the request carried, got {summary}"
        );
        assert_eq!(
            summary
                .get("ModelPackageRegistrationType")
                .and_then(Value::as_str),
            row.registration,
            "case {case:?}: expected the registration type the request carried, got {summary}"
        );
        assert_eq!(
            str_field(&summary, "ModelPackageStatus"),
            "Completed",
            "case {case:?}: expected Completed once the transition is due, got {summary}"
        );
        assert_eq!(
            summary
                .get("InferenceSpecification")
                .and_then(|spec| spec.get("Containers"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(row.containers),
            "case {case:?}: expected {} containers, got {summary}",
            row.containers
        );
        assert!(
            summary.get("CreationTime").and_then(Value::as_f64).is_some(),
            "case {case:?}: expected a numeric CreationTime, got {summary}"
        );

        // The summary is a projection of the package the single-resource read replays:
        // every member it carries holds the value `DescribeModelPackage` reports.
        let described = emulator.describe(case, arn).await;
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(member),
                described.get(member),
                "case {case:?}: expected the summary's {member} to equal the described \
                 package's, got {:?} and {:?}",
                summary.get(member),
                described.get(member)
            );
        }
        // A summary carries nothing beyond the modelled members.
        for member in summary.as_object().expect("a summary is an object").keys() {
            assert!(
                SUMMARY_MEMBERS.contains(&member.as_str()),
                "case {case:?}: unexpected summary member {member:?} in {summary}"
            );
        }
    }

    // Reading changes nothing a later caller sees: the same batch answers the same way.
    let again = batch_describe(&emulator, "the same batch again", &arns).await;
    assert_eq!(
        again, output,
        "case \"the same batch again\": a batch read must not change what it reports, \
         got {again} after {output}"
    );
}

/// RULE: the batch reports the status the package is actually in, so a version read back
/// before the clock owes it `Completed` is reported in the non-terminal `Pending` status
/// `CreateModelPackage` registered it in, and in `Completed` once that instant has
/// passed — the batch read neither skips nor anticipates the transition.
#[tokio::test]
async fn a_version_is_reported_pending_until_the_clock_owes_completed() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "settling").await;
    let arn = emulator
        .create(
            "a freshly registered version",
            &json!({
                "ModelPackageGroupName": "settling",
                "InferenceSpecification": inference_specification(1),
            }),
        )
        .await;

    // One registration and one batch call: the transition is not due yet, so the caller
    // still observes the non-terminal status.
    let pending = batch_describe(
        &emulator,
        "before the transition is due",
        std::slice::from_ref(&arn),
    )
    .await;
    let summary = summaries(&pending)
        .get(&arn)
        .cloned()
        .unwrap_or_else(|| panic!("expected a summary for {arn} in {pending}"));
    assert_eq!(
        str_field(&summary, "ModelPackageStatus"),
        "Pending",
        "case \"before the transition is due\": expected Pending, got {summary}"
    );

    tokio::time::sleep(PAST_COMPLETED_DUE).await;

    let settled = batch_describe(
        &emulator,
        "once the transition is due",
        std::slice::from_ref(&arn),
    )
    .await;
    let summary = summaries(&settled)
        .get(&arn)
        .cloned()
        .unwrap_or_else(|| panic!("expected a summary for {arn} in {settled}"));
    assert_eq!(
        str_field(&summary, "ModelPackageStatus"),
        "Completed",
        "case \"once the transition is due\": expected Completed, got {summary}"
    );
}

/// RULE: an ARN the account holds no versioned model package for is reported as its own
/// `BatchDescribeModelPackageErrorMap` entry, in the BATCH's error vocabulary — the fixed
/// `ErrorCode` `EntityNotFoundError` and the fixed `ErrorResponse` `The ARN is not found.`,
/// with nothing from the request interpolated, since the entry is already keyed by the ARN
/// it is about — so the request still answers 200 and the ARNs beside it are still
/// summarised.
#[tokio::test]
async fn an_arn_without_a_versioned_package_is_reported_per_item() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "known").await;
    let known = emulator
        .create(
            "the version every batch below also names",
            &json!({
                "ModelPackageGroupName": "known",
                "InferenceSpecification": inference_specification(1),
            }),
        )
        .await;
    // An unversioned package exists but belongs to no model package group, so this
    // operation — which describes versioned model packages — has no summary for it.
    let unversioned = emulator
        .create(
            "an unversioned package",
            &json!({
                "ModelPackageName": "unversioned",
                "InferenceSpecification": inference_specification(1),
            }),
        )
        .await;

    let rows = [
        ("a name nothing was registered under", package_arn("missing")),
        ("a version the group does not hold", package_arn("known/99")),
        ("a group that was never created", package_arn("absent/1")),
        ("a group name instead of a version", package_arn("known")),
        ("an unversioned model package", unversioned),
        (
            "an ARN naming another account",
            "arn:aws:sagemaker:us-east-1:999999999999:model-package/known/1".to_string(),
        ),
    ];

    for (case, arn) in rows {
        // Alone in the batch, and beside an ARN that does resolve: one bad item neither
        // fails the request nor hides the others.
        for (shape, requested) in [
            ("alone", vec![arn.clone()]),
            ("beside a known ARN", vec![known.clone(), arn.clone()]),
        ] {
            let case = format!("{case}, {shape}");
            let output = batch_describe(&emulator, &case, &requested).await;
            let errors = item_errors(&output);
            let entry = errors
                .get(&arn)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected an error entry for {arn} in {output}"));
            assert_eq!(
                str_field(&entry, "ErrorCode"),
                "EntityNotFoundError",
                "case {case:?}: expected the batch vocabulary's not-found code for an ARN \
                 with no summary, got {entry}"
            );
            assert_eq!(
                str_field(&entry, "ErrorResponse"),
                "The ARN is not found.",
                "case {case:?}: expected the fixed not-found response, got {entry}"
            );
            assert!(
                !str_field(&entry, "ErrorResponse").contains(&arn),
                "case {case:?}: the entry is keyed by the ARN, so its text interpolates \
                 nothing from the request, got {entry}"
            );
            assert!(
                !summaries(&output).contains_key(&arn),
                "case {case:?}: an ARN in error carries no summary, got {output}"
            );
            assert_eq!(
                summaries(&output).contains_key(&known),
                shape == "beside a known ARN",
                "case {case:?}: the ARN that resolves must still be summarised, got {output}"
            );
        }
    }

    // The failed items left the store alone: the unversioned package and the version are
    // both still readable through the single-resource read.
    for identifier in ["unversioned", "known/1"] {
        let described = emulator.describe(identifier, identifier).await;
        assert_eq!(
            str_field(&described, "ModelPackageArn"),
            package_arn(identifier),
            "case {identifier:?}: the batch read must leave the package in place, got {described}"
        );
    }
}

/// RULE: every requested ARN is accounted for exactly once — it is a key of
/// `ModelPackageSummaries` or of `BatchDescribeModelPackageErrorMap`, never of both and
/// never of neither — whatever the batch mixes and however often an ARN repeats.
#[tokio::test]
async fn every_requested_arn_is_accounted_for_exactly_once() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "mixed").await;
    let mut versions = Vec::new();
    for index in 0..3 {
        versions.push(
            emulator
                .create(
                    &format!("version {}", index + 1),
                    &json!({
                        "ModelPackageGroupName": "mixed",
                        "InferenceSpecification": inference_specification(1),
                    }),
                )
                .await,
        );
    }
    let unversioned = emulator
        .create(
            "an unversioned package",
            &json!({
                "ModelPackageName": "loner",
                "InferenceSpecification": inference_specification(1),
            }),
        )
        .await;
    let unknown = package_arn("nothing");

    let batches = [
        ("one found ARN", vec![versions[0].clone()]),
        ("one unknown ARN", vec![unknown.clone()]),
        (
            "found, unknown and unversioned, interleaved",
            vec![
                versions[2].clone(),
                unknown.clone(),
                versions[0].clone(),
                unversioned.clone(),
                versions[1].clone(),
            ],
        ),
        (
            "the same ARNs in the opposite order",
            vec![
                versions[1].clone(),
                unversioned.clone(),
                versions[0].clone(),
                unknown.clone(),
                versions[2].clone(),
            ],
        ),
        (
            "every ARN repeated",
            vec![
                versions[0].clone(),
                versions[0].clone(),
                unknown.clone(),
                unknown.clone(),
                unversioned.clone(),
                unversioned.clone(),
            ],
        ),
    ];

    for (case, requested) in batches {
        let output = batch_describe(&emulator, case, &requested).await;
        let found: BTreeSet<String> = summaries(&output).keys().cloned().collect();
        let failed: BTreeSet<String> = item_errors(&output).keys().cloned().collect();
        let asked: BTreeSet<String> = requested.iter().cloned().collect();

        assert!(
            found.is_disjoint(&failed),
            "case {case:?}: no ARN is both summarised and in error, got {output}"
        );
        let answered: BTreeSet<String> = found.union(&failed).cloned().collect();
        assert_eq!(
            answered, asked,
            "case {case:?}: every requested ARN must be answered exactly once, got {output}"
        );
        // The composition of the answer follows the ARNs, not their order or multiplicity.
        let expected_found: BTreeSet<String> = asked
            .iter()
            .filter(|arn| versions.contains(arn))
            .cloned()
            .collect();
        assert_eq!(
            found, expected_found,
            "case {case:?}: expected exactly the registered versions to be summarised, \
             got {output}"
        );
    }
}

/// RULE: `ModelPackageArnList` is modelled `len 1..100`
/// (`smithy_query(query="shape",shape="ModelPackageArnList")`), so a batch of 1 and a
/// batch of 100 are both answered item by item, while an empty list and a list of 101 —
/// one past each bound — are refused 400 `ValidationException` and nothing is answered.
#[tokio::test]
async fn the_batch_size_bounds_the_model_declares() {
    let emulator = Emulator::new();
    emulator.create_group(state::DEFAULT_REGION, "bounded").await;
    let mut arns = Vec::new();
    for index in 0..100 {
        arns.push(
            emulator
                .create(
                    &format!("version {}", index + 1),
                    &json!({
                        "ModelPackageGroupName": "bounded",
                        "InferenceSpecification": inference_specification(1),
                    }),
                )
                .await,
        );
    }

    for size in [1_usize, 100] {
        let case = format!("a batch of {size}");
        let requested = arns[..size].to_vec();
        let output = batch_describe(&emulator, &case, &requested).await;
        assert_eq!(
            summaries(&output).len(),
            size,
            "case {case:?}: expected one summary per requested ARN, got {output}"
        );
        assert!(
            item_errors(&output).is_empty(),
            "case {case:?}: every requested ARN resolves, got {output}"
        );
    }

    let mut past_maximum: Vec<String> = arns.clone();
    past_maximum.push(package_arn("bounded/1"));
    for (case, requested) in [
        ("an empty list, one below the minimum", Vec::new()),
        ("a list of 101, one past the maximum", past_maximum),
    ] {
        let (status, refused) = emulator
            .call(
                "BatchDescribeModelPackage",
                &json!({"ModelPackageArnList": requested}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
    }

    // The refusals changed nothing: a batch inside the bounds still answers from the
    // same store.
    let after = batch_describe(&emulator, "after the refusals", &arns[..1]).await;
    assert_eq!(
        summaries(&after).len(),
        1,
        "case \"after the refusals\": a refused request must leave the store intact, \
         got {after}"
    );
}
