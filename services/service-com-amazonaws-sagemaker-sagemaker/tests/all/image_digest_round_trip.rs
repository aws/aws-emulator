// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the digest a container image is reported with, on both
//! registries that carry one: an algorithm's training image
//! (`TrainingSpecification$TrainingImageDigest`) and a model package's inference
//! containers (`ModelPackageContainerDefinition$ImageDigest`).
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness and the create/describe
//! helpers are reused from `model_package_registry`; what is reported for a reference that
//! carries no digest at all is asserted in `image_digest_probe`.
//!
//! The RULE, asserted over a TABLE of inputs rather than one recorded call: the digest a
//! read reports for a container image is the one the registration CARRIES — the digest
//! member when the request supplied one, otherwise the digest the image reference itself
//! pins in the documented `registry/repository[@digest]` format — and it is never
//! recomputed over a value the caller gave.

use serde_json::{json, Value};

use crate::model_package_registry::Emulator;

/// 64 hex digits, as `ImageDigest`'s modelled pattern requires
/// (`smithy_query(query="shape",shape="ImageDigest")`: `@length 0..72`,
/// `@pattern ^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`).
pub(crate) fn hex64(seed: &str) -> String {
    let unit: String = seed.chars().filter(char::is_ascii_hexdigit).collect();
    let mut hex = unit.repeat(64);
    hex.truncate(64);
    hex
}

/// A digest in the `sha256:<64 hex>` form, with `algorithm` spelled as the row wants it —
/// the modelled pattern admits any case of the algorithm name and of the hex.
fn digest(algorithm: &str, seed: &str) -> String {
    format!("{algorithm}:{}", hex64(seed))
}

/// Whether `value` is a digest an `ImageDigest`-typed member can carry, i.e. one inside
/// the modelled pattern.
pub(crate) fn is_reportable_digest(value: &str) -> bool {
    let Some((algorithm, hex)) = value.split_once(':') else {
        return false;
    };
    algorithm.eq_ignore_ascii_case("sha256")
        && hex.len() == 64
        && hex.chars().all(|character| character.is_ascii_hexdigit())
}

/// The digest `DescribeAlgorithm` reports for an algorithm's training image.
fn training_digest(described: &Value) -> Option<String> {
    described
        .pointer("/TrainingSpecification/TrainingImageDigest")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The digest a described resource reports for the inference container at `index`.
pub(crate) fn container_digest(described: &Value, index: usize) -> Option<String> {
    described
        .pointer(&format!(
            "/InferenceSpecification/Containers/{index}/ImageDigest"
        ))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A `TrainingSpecification` naming `image`, with the three members the shape marks
/// required, and `supplied` as its `TrainingImageDigest` when the row supplies one.
pub(crate) fn training_specification(image: &str, supplied: Option<&str>) -> Value {
    let mut specification = json!({
        "TrainingImage": image,
        "SupportedTrainingInstanceTypes": ["ml.m5.large"],
        "TrainingChannels": [{
            "Name": "train",
            "SupportedContentTypes": ["text/csv"],
            "SupportedInputModes": ["File"],
        }],
    });
    if let Some(supplied) = supplied {
        specification["TrainingImageDigest"] = json!(supplied);
    }
    specification
}

/// An `InferenceSpecification` whose single container names `image`, with `supplied` as its
/// `ImageDigest` when the row supplies one.
pub(crate) fn inference_specification(image: &str, supplied: Option<&str>) -> Value {
    let mut container = json!({"Image": image, "ModelDataUrl": "s3://models/artifact.tar.gz"});
    if let Some(supplied) = supplied {
        container["ImageDigest"] = json!(supplied);
    }
    json!({"Containers": [container]})
}

/// One row of the table: the image reference a registration names, the digest member it
/// supplies (if any), and the digest every reader must then report.
struct Row {
    case: &'static str,
    image: String,
    supplied: Option<String>,
    expected: String,
}

/// The rows both registries are driven over, spanning what the rule generalises over:
/// whether the request supplies the digest member at all, the case of the algorithm name
/// and of the hex in a supplied digest, whether the image reference pins a digest of its
/// own, and a supplied digest that DISAGREES with the one the reference pins.
fn rows() -> Vec<Row> {
    let registry = "000000000000.dkr.ecr.us-east-1.amazonaws.com";
    let supplied = digest("sha256", "8ebddbc1");
    let pinned = digest("sha256", "a9a77d47");
    vec![
        Row {
            case: "a supplied digest, tagged image",
            image: format!("{registry}/inference:1"),
            supplied: Some(supplied.clone()),
            expected: supplied.clone(),
        },
        Row {
            case: "a supplied digest in uppercase hex",
            image: format!("{registry}/inference:2"),
            supplied: Some(digest("sha256", "8EBDDBC1")),
            expected: digest("sha256", "8EBDDBC1"),
        },
        Row {
            case: "a supplied digest with an uppercased algorithm name",
            image: format!("{registry}/inference:3"),
            supplied: Some(digest("SHA256", "8ebddbc1")),
            expected: digest("SHA256", "8ebddbc1"),
        },
        Row {
            case: "a supplied digest that disagrees with the one the reference pins",
            image: format!("{registry}/inference@{pinned}"),
            supplied: Some(supplied.clone()),
            expected: supplied,
        },
        Row {
            case: "no digest supplied, a reference that pins one",
            image: format!("{registry}/inference@{pinned}"),
            supplied: None,
            expected: pinned.clone(),
        },
        Row {
            case: "no digest supplied, a reference that pins one behind a tag",
            image: format!("{registry}/inference:4@{pinned}"),
            supplied: None,
            expected: pinned.clone(),
        },
        Row {
            case: "no digest supplied, a registry with a port that pins one",
            image: format!("registry.example.com:5000/inference@{pinned}"),
            supplied: None,
            expected: pinned,
        },
    ]
}

/// RULE: `DescribeAlgorithm` reports, for the training image and for every inference
/// container of the algorithm, the digest the registration carried — the supplied member
/// when there was one, otherwise the digest the image reference pins — and never a value
/// recomputed over it.
#[tokio::test]
async fn an_algorithm_reports_the_image_digest_its_registration_carried() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let body = json!({
            "AlgorithmName": "digests",
            "TrainingSpecification": training_specification(&row.image, row.supplied.as_deref()),
            "InferenceSpecification": inference_specification(&row.image, row.supplied.as_deref()),
        });
        emulator.ok(case, "CreateAlgorithm", &body).await;

        let described = emulator
            .ok(case, "DescribeAlgorithm", &json!({"AlgorithmName": "digests"}))
            .await;
        assert_eq!(
            training_digest(&described).as_deref(),
            Some(row.expected.as_str()),
            "case {case:?}: expected TrainingImageDigest {:?}, got {described}",
            row.expected
        );
        assert_eq!(
            container_digest(&described, 0).as_deref(),
            Some(row.expected.as_str()),
            "case {case:?}: expected the inference container's ImageDigest {:?}, got \
             {described}",
            row.expected
        );
        assert!(
            is_reportable_digest(&row.expected),
            "case {case:?}: the row's own expectation must be a digest the member can \
             carry, got {:?}",
            row.expected
        );

        // A second read reports the same digest: it is stored, not resolved per call.
        let again = emulator
            .ok(case, "DescribeAlgorithm", &json!({"AlgorithmName": "digests"}))
            .await;
        assert_eq!(
            training_digest(&again),
            training_digest(&described),
            "case {case:?}: the reported digest must be stable across reads, got {again}"
        );
    }
}

/// RULE: the same rule on the model-package registry — `DescribeModelPackage` and the
/// `BatchDescribeModelPackage` summary report, for every inference container and every
/// additional inference specification's container, the digest the registration carried.
#[tokio::test]
async fn a_model_package_reports_the_image_digest_its_registration_carried() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.create_group("us-east-1", "digest-group").await;
        let containers =
            inference_specification(&row.image, row.supplied.as_deref())["Containers"].clone();
        let arn = emulator
            .create(
                case,
                &json!({
                    "ModelPackageGroupName": "digest-group",
                    "InferenceSpecification":
                        inference_specification(&row.image, row.supplied.as_deref()),
                    "AdditionalInferenceSpecifications": [{
                        "Name": "extra",
                        "Containers": containers,
                    }],
                }),
            )
            .await;

        let described = emulator.describe(case, &arn).await;
        assert_eq!(
            container_digest(&described, 0).as_deref(),
            Some(row.expected.as_str()),
            "case {case:?}: expected the container's ImageDigest {:?}, got {described}",
            row.expected
        );
        assert_eq!(
            described
                .pointer("/AdditionalInferenceSpecifications/0/Containers/0/ImageDigest")
                .and_then(Value::as_str),
            Some(row.expected.as_str()),
            "case {case:?}: expected the additional specification's container to carry \
             {:?}, got {described}",
            row.expected
        );

        let batch = emulator
            .ok(
                case,
                "BatchDescribeModelPackage",
                &json!({"ModelPackageArnList": [arn.clone()]}),
            )
            .await;
        let summary = batch
            .pointer("/ModelPackageSummaries")
            .and_then(|summaries| summaries.get(&arn))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {arn}, got {batch}"));
        assert_eq!(
            container_digest(&summary, 0).as_deref(),
            Some(row.expected.as_str()),
            "case {case:?}: expected the batch summary to carry {:?}, got {summary}",
            row.expected
        );
    }
}
