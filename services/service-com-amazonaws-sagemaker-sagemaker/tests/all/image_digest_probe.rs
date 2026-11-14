// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire probes for the digest the service reports when a registration carries
//! NONE: the image reference names no content digest, and nothing here pulls an image, so
//! what a reader reports is the service's own identifier for that reference.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The digest helpers and the rows
//! where the registration DOES carry a digest belong to `image_digest_round_trip`.
//!
//! The RULE, asserted over a TABLE of references rather than one recorded call: the value
//! reported for a reference that pins no digest a member can carry is in the modelled
//! `ImageDigest` form, belongs to the REFERENCE — the same for every resource naming it
//! and on every read — and is never shared by two different references.

use serde_json::{json, Value};

use crate::image_digest_round_trip::{
    container_digest, hex64, inference_specification, is_reportable_digest,
    training_specification,
};
use crate::model_package_registry::Emulator;

/// The digest `DescribeAlgorithm` reports for an algorithm's training image.
fn training_digest(described: &Value) -> Option<String> {
    described
        .pointer("/TrainingSpecification/TrainingImageDigest")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// RULE: for an image reference that pins NO digest, the service has no registry to
/// resolve one from, so it reports its own identifier for that reference: a value in the
/// modelled `ImageDigest` form, one per reference — the same for every resource naming the
/// same reference and on every read, and a different one for a different reference.
#[tokio::test]
async fn a_reference_that_pins_no_digest_is_reported_with_one_identifier_per_reference() {
    // The rows differ along the part of the reference that changes: the tag, the
    // repository, the registry host, and a reference with no tag at all.
    let references = [
        "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:1",
        "000000000000.dkr.ecr.us-east-1.amazonaws.com/inference:2",
        "000000000000.dkr.ecr.us-east-1.amazonaws.com/other:1",
        "registry.example.com/inference:1",
        "registry.example.com/inference",
    ];

    let emulator = Emulator::new();
    emulator.create_group("us-east-1", "identifier-group").await;
    let mut reported: Vec<(String, String)> = Vec::new();
    for (index, image) in references.iter().enumerate() {
        let case = format!("reference {image}");
        let algorithm = format!("identifier{index}");
        emulator
            .ok(
                &case,
                "CreateAlgorithm",
                &json!({
                    "AlgorithmName": algorithm,
                    "TrainingSpecification": training_specification(image, None),
                }),
            )
            .await;
        let described = emulator
            .ok(
                &case,
                "DescribeAlgorithm",
                &json!({"AlgorithmName": algorithm}),
            )
            .await;
        let digest = training_digest(&described)
            .unwrap_or_else(|| panic!("case {case:?}: expected a digest, got {described}"));
        assert!(
            is_reportable_digest(&digest),
            "case {case:?}: expected a digest in the modelled ImageDigest form, got \
             {digest:?}"
        );

        // A second read reports the same value: it is stored, not resolved per call.
        let again = emulator
            .ok(
                &case,
                "DescribeAlgorithm",
                &json!({"AlgorithmName": algorithm}),
            )
            .await;
        assert_eq!(
            training_digest(&again).as_deref(),
            Some(digest.as_str()),
            "case {case:?}: the reported identifier must be stable across reads, got \
             {again}"
        );

        // A model package naming the SAME reference is reported with the same identifier:
        // the value belongs to the reference, not to the resource that names it.
        let arn = emulator
            .create(
                &case,
                &json!({
                    "ModelPackageGroupName": "identifier-group",
                    "InferenceSpecification": inference_specification(image, None),
                }),
            )
            .await;
        let package = emulator.describe(&case, &arn).await;
        assert_eq!(
            container_digest(&package, 0).as_deref(),
            Some(digest.as_str()),
            "case {case:?}: expected the same identifier for the same reference, got \
             {package}"
        );
        reported.push(((*image).to_string(), digest));
    }

    for (image, digest) in &reported {
        for (other_image, other_digest) in &reported {
            if image == other_image {
                continue;
            }
            assert_ne!(
                digest, other_digest,
                "case {image:?} against {other_image:?}: two references must not share \
                 one identifier, got {digest:?} twice"
            );
        }
    }
}

/// RULE: a digest outside the modelled `ImageDigest` pattern is no digest the member can
/// carry, so a reference pinning one is reported with the service's own identifier
/// instead — and the registration is still admitted, since what a registry HOLDS is not
/// part of admission.
#[tokio::test]
async fn a_reference_pinning_an_unreportable_digest_falls_back_to_the_identifier() {
    // Rows: the digest forms the published reference grammar admits but the modelled
    // `ImageDigest` pattern does not — another algorithm, and a hex run of the wrong
    // length.
    let references = [
        (
            "another algorithm",
            format!("registry.example.com/inference@sha512:{}", hex64("ab")),
        ),
        (
            "a hex run shorter than the hash",
            "registry.example.com/inference@sha256:0123abcd".to_string(),
        ),
    ];

    for (case, image) in references {
        let emulator = Emulator::new();
        emulator
            .ok(
                case,
                "CreateAlgorithm",
                &json!({
                    "AlgorithmName": "unreportable",
                    "TrainingSpecification": training_specification(&image, None),
                }),
            )
            .await;
        let described = emulator
            .ok(
                case,
                "DescribeAlgorithm",
                &json!({"AlgorithmName": "unreportable"}),
            )
            .await;
        let digest = training_digest(&described)
            .unwrap_or_else(|| panic!("case {case:?}: expected a digest, got {described}"));
        assert!(
            is_reportable_digest(&digest),
            "case {case:?}: expected a digest in the modelled ImageDigest form, got \
             {digest:?}"
        );
        assert!(
            !image.ends_with(&digest),
            "case {case:?}: a digest the member cannot carry must not be reported, got \
             {digest:?} for {image:?}"
        );
    }
}
