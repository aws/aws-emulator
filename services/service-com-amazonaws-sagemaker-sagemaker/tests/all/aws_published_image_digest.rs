// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the digest a read reports for a container image AWS ITSELF
//! publishes, wherever a resource names one.
//!
//! The RULE under test, over a TABLE of inputs rather than one recorded call: the digest
//! reported for an image reference is the digest of the IMAGE VERSION the reference names, so a
//! reference naming a version AWS published — a repository and tag of a registry AWS publishes
//! SageMaker's containers from — is reported with the digest of the image AWS published there,
//! one value for that version whichever Region's published registry the path pulls it from and
//! whichever operation resolves it, while a reference naming anything else (another tag of the
//! same repository, another repository, another registry) is reported with what resolution gives
//! it there and never with that version's digest.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`)
//! through the same `MultiServiceRouter` the production router uses, on the harness
//! `tests/all/inference_deployment_lifecycle.rs` owns, so what these tests exercise is what a
//! real SDK client gets.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{one_variant_config, primary_container, Emulator, ACCOUNT};

/// The digest the version AWS publishes as tag `1` of its built-in gradient-boosted tree
/// algorithm's repository carries — the value a read of a resource built around THAT published
/// version reports, and no other.
///
/// Written out here rather than read from the service's own record, so this suite states the
/// expectation independently: a value edited in the implementation fails this test instead of
/// silently moving with it.
const PUBLISHED_DIGEST: &str =
    "sha256:0c8f830ac408e6dee08445fb60392e9c3f05f790a4b3c07ec22327c08f75bcbf";

/// The tag that published version is named by.
const PUBLISHED_TAG: &str = "1";

/// A digest in the form `ImageDigest` carries, for the row that PINS the image it names.
const PINNED_DIGEST: &str =
    "sha256:2222222222222222222222222222222222222222222222222222222222222222";

/// An image path into the Amazon ECR registry `registry` of `region`, which is one of the two
/// formats a container image member documents (`registry/repository[:tag]`).
fn image_path(registry: &str, region: &str, repository: &str) -> String {
    format!("{registry}.dkr.ecr.{region}.amazonaws.com/{repository}")
}

/// The repository part of an image reference: everything before the `:tag` or `@digest` of its
/// last path component.
///
/// Written here independently of the service's own reading of a reference, so what the
/// assertions compare against is not the code under test.
fn repository_of(image: &str) -> String {
    let (prefix, last) = match image.rsplit_once('/') {
        Some((prefix, last)) => (format!("{prefix}/"), last),
        None => (String::new(), image),
    };
    let repository = match last.split_once(['@', ':']) {
        Some((repository, _)) => repository,
        None => last,
    };
    format!("{prefix}{repository}")
}

/// Whether `digest` is in the form every reported image digest carries — the `ImageDigest`
/// shape's modelled pattern (`smithy_query(query="shape",shape="ImageDigest")`:
/// `^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`).
fn is_image_digest(digest: &str) -> bool {
    match digest.split_once(':') {
        Some((algorithm, hex)) => {
            algorithm.eq_ignore_ascii_case("sha256")
                && hex.len() == 64
                && hex.chars().all(|digit| digit.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// One row of the table: an image reference, and the digest a read of a resource built around
/// it has to report.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The Region the request is signed for, which is the Region its registry host names.
    region: &'static str,
    /// The reference a resource is built around.
    image: String,
    /// The digest the read has to report, and `None` for a reference whose digest is none of
    /// the published ones — resolved to this emulator's own identifier for the reference, which
    /// the row asserts only the form and the distinctness of.
    digest: Option<&'static str>,
}

/// The references the rule is asserted over.
///
/// Rows differ along what the rule generalises over: which published registry (and so which
/// Region) the path names the version in, WHICH TAG of the repository it names — the published
/// one, another, the registry's default, none at all, and a digest pinned instead of a tag —
/// whether the repository is the published one, and whose registry the host addresses: the
/// caller's own, a published account inside another Region's host, and a registry that is no
/// Amazon ECR registry at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the published version in one Region's registry",
            region: "us-west-2",
            image: format!(
                "{}:{PUBLISHED_TAG}",
                image_path("433757028032", "us-west-2", "xgboost")
            ),
            digest: Some(PUBLISHED_DIGEST),
        },
        Row {
            case: "the same published version in another Region's registry",
            region: "us-east-1",
            image: format!(
                "{}:{PUBLISHED_TAG}",
                image_path("811284229777", "us-east-1", "xgboost")
            ),
            digest: Some(PUBLISHED_DIGEST),
        },
        Row {
            case: "another tag of the same published repository",
            region: "us-west-2",
            image: format!(
                "{}:1.7-1",
                image_path("433757028032", "us-west-2", "xgboost")
            ),
            digest: None,
        },
        Row {
            case: "the default tag of the same published repository",
            region: "us-west-2",
            image: format!(
                "{}:latest",
                image_path("433757028032", "us-west-2", "xgboost")
            ),
            digest: None,
        },
        Row {
            case: "the same published repository with no tag at all",
            region: "us-west-2",
            image: image_path("433757028032", "us-west-2", "xgboost"),
            digest: None,
        },
        Row {
            case: "the published repository pinned by a digest of its own",
            region: "us-west-2",
            image: format!(
                "{}@{PINNED_DIGEST}",
                image_path("433757028032", "us-west-2", "xgboost")
            ),
            digest: Some(PINNED_DIGEST),
        },
        Row {
            case: "a published framework repository whose name carries the same segment",
            region: "us-west-2",
            image: format!(
                "{}:{PUBLISHED_TAG}",
                image_path("246618743249", "us-west-2", "sagemaker-xgboost")
            ),
            digest: None,
        },
        Row {
            case: "a published algorithm repository no digest is held for",
            region: "us-west-2",
            image: format!(
                "{}:{PUBLISHED_TAG}",
                image_path("433757028032", "us-west-2", "linear-learner")
            ),
            digest: None,
        },
        Row {
            case: "the same repository and tag in the caller's own registry",
            region: "us-west-2",
            image: format!(
                "{}:{PUBLISHED_TAG}",
                image_path(ACCOUNT, "us-west-2", "xgboost")
            ),
            digest: None,
        },
        Row {
            case: "a published registry account inside another Region's host",
            region: "us-east-1",
            image: format!(
                "{}:{PUBLISHED_TAG}",
                image_path("433757028032", "us-east-1", "xgboost")
            ),
            digest: None,
        },
        Row {
            case: "a registry that is no Amazon ECR registry",
            region: "us-west-2",
            image: format!("registry.example.com/xgboost:{PUBLISHED_TAG}"),
            digest: None,
        },
    ]
}

/// One SageMaker call signed for `region` that must be served, with whatever it answered.
///
/// The Region is the call's own, because a row's registry host names the Region the request is
/// made in: a reference into another Region's registry is a different question (`image_pull`).
async fn served(
    emulator: &Emulator,
    case: &str,
    region: &str,
    target: &str,
    body: &Value,
) -> Value {
    let (status, answered) = emulator.call_in(region, target, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {target} in {region}, got {status} with \
         {answered}"
    );
    answered
}

/// RULE: the `DeployedImages` entry a hosted variant reports names the image at the digest of
/// the image VERSION the reference names — the published version's own digest for a reference
/// naming it in a registry AWS publishes from, and never that digest for a reference naming
/// another tag, another repository or another registry.
#[tokio::test]
async fn a_variant_reports_the_digest_of_the_image_aws_published() {
    let mut derived: Vec<(&str, String)> = Vec::new();
    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            region,
            image,
            digest,
        } = row;
        let emulator = Emulator::new();
        let model = format!("published-digest-model-{index}");
        let config = format!("published-digest-config-{index}");
        let endpoint = format!("published-digest-endpoint-{index}");

        served(
            &emulator,
            case,
            region,
            "CreateModel",
            &json!({ "ModelName": model, "PrimaryContainer": primary_container(&image) }),
        )
        .await;
        served(
            &emulator,
            case,
            region,
            "CreateEndpointConfig",
            &one_variant_config(&config, &model),
        )
        .await;
        served(
            &emulator,
            case,
            region,
            "CreateEndpoint",
            &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
        )
        .await;
        let described = served(
            &emulator,
            case,
            region,
            "DescribeEndpoint",
            &json!({ "EndpointName": endpoint }),
        )
        .await;

        let deployed = described
            .pointer("/ProductionVariants/0/DeployedImages/0")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a DeployedImages entry in {described}"));
        assert_eq!(
            deployed.get("SpecifiedImage").and_then(Value::as_str),
            Some(image.as_str()),
            "case {case:?}: expected SpecifiedImage {image:?}, got {deployed}"
        );
        let resolved = deployed
            .get("ResolvedImage")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResolvedImage, got {deployed}"));
        let repository = repository_of(&image);
        let reported = resolved
            .strip_prefix(&format!("{repository}@"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected ResolvedImage {repository}@<digest>, got {resolved:?}")
            });
        assert!(
            is_image_digest(reported),
            "case {case:?}: expected a sha256 digest in {resolved:?}, got {reported:?}"
        );
        match digest {
            Some(expected) => assert_eq!(
                reported, expected,
                "case {case:?}: expected the digest {expected:?} of the image named, got \
                 {reported:?}"
            ),
            None => {
                assert_ne!(
                    reported, PUBLISHED_DIGEST,
                    "case {case:?}: a reference naming another image version must not report \
                     the published version's digest, got {resolved:?}"
                );
                derived.push((case, reported.to_string()));
            }
        }
    }
    // Every reference nothing published a digest for keeps its own: two such references never
    // collapse onto one digest.
    let distinct: std::collections::BTreeSet<&String> =
        derived.iter().map(|(_, digest)| digest).collect();
    assert_eq!(
        distinct.len(),
        derived.len(),
        "each unpublished reference keeps its own digest, got {derived:?}"
    );
}

/// RULE: one image version reports ONE digest, to every operation that resolves it — so the
/// digest a registered algorithm and a registered model package report for a published version
/// is the same one a hosted variant reports for it, and a reference naming a different tag of
/// that repository reports a different digest to those same readers.
#[tokio::test]
async fn every_reader_of_a_published_image_reports_the_same_digest() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let image = format!(
        "{}:{PUBLISHED_TAG}",
        image_path("433757028032", region, "xgboost")
    );

    // An algorithm registration, whose training specification resolves the training image.
    let case = "an algorithm registered around the published image";
    served(
        &emulator,
        case,
        region,
        "CreateAlgorithm",
        &json!({
            "AlgorithmName": "published-digest-algorithm",
            "TrainingSpecification": {
                "TrainingImage": image,
                "SupportedTrainingInstanceTypes": ["ml.m5.large"],
                "TrainingChannels": [{
                    "Name": "train",
                    "SupportedContentTypes": ["text/csv"],
                    "SupportedInputModes": ["File"],
                }],
            },
        }),
    )
    .await;
    let algorithm = served(
        &emulator,
        case,
        region,
        "DescribeAlgorithm",
        &json!({ "AlgorithmName": "published-digest-algorithm" }),
    )
    .await;
    assert_eq!(
        algorithm
            .pointer("/TrainingSpecification/TrainingImageDigest")
            .and_then(Value::as_str),
        Some(PUBLISHED_DIGEST),
        "case {case:?}: expected the published image's digest, got {algorithm}"
    );

    // A model package registration, whose inference containers resolve the same reference.
    let case = "a model package registered around the published image";
    served(
        &emulator,
        case,
        region,
        "CreateModelPackage",
        &json!({
            "ModelPackageName": "published-digest-package",
            "InferenceSpecification": {
                "Containers": [{ "Image": image }],
                "SupportedContentTypes": ["text/csv"],
                "SupportedResponseMIMETypes": ["text/csv"],
            },
        }),
    )
    .await;
    let package = served(
        &emulator,
        case,
        region,
        "DescribeModelPackage",
        &json!({ "ModelPackageName": "published-digest-package" }),
    )
    .await;
    assert_eq!(
        package
            .pointer("/InferenceSpecification/Containers/0/ImageDigest")
            .and_then(Value::as_str),
        Some(PUBLISHED_DIGEST),
        "case {case:?}: expected the published image's digest, got {package}"
    );

    // And a hosted variant, which reports the same digest inside the resolved path.
    let case = "a variant hosting a model built around the published image";
    served(
        &emulator,
        case,
        region,
        "CreateModel",
        &json!({
            "ModelName": "published-digest-shared-model",
            "PrimaryContainer": primary_container(&image),
        }),
    )
    .await;
    served(
        &emulator,
        case,
        region,
        "CreateEndpointConfig",
        &one_variant_config(
            "published-digest-shared-config",
            "published-digest-shared-model",
        ),
    )
    .await;
    served(
        &emulator,
        case,
        region,
        "CreateEndpoint",
        &json!({
            "EndpointName": "published-digest-shared-endpoint",
            "EndpointConfigName": "published-digest-shared-config",
        }),
    )
    .await;
    let described = served(
        &emulator,
        case,
        region,
        "DescribeEndpoint",
        &json!({ "EndpointName": "published-digest-shared-endpoint" }),
    )
    .await;
    assert_eq!(
        described
            .pointer("/ProductionVariants/0/DeployedImages/0/ResolvedImage")
            .and_then(Value::as_str),
        Some(format!("{}@{PUBLISHED_DIGEST}", repository_of(&image)).as_str()),
        "case {case:?}: expected the published image's digest-pinned path, got {described}"
    );

    // And the same readers tell that published version APART from another tag of the same
    // repository: a digest is known for one version, so another tag of it is a different image.
    let case = "another tag of the same published repository";
    let other = format!("{}:1.7-1", image_path("433757028032", region, "xgboost"));
    served(
        &emulator,
        case,
        region,
        "CreateAlgorithm",
        &json!({
            "AlgorithmName": "other-tag-algorithm",
            "TrainingSpecification": {
                "TrainingImage": other,
                "SupportedTrainingInstanceTypes": ["ml.m5.large"],
                "TrainingChannels": [{
                    "Name": "train",
                    "SupportedContentTypes": ["text/csv"],
                    "SupportedInputModes": ["File"],
                }],
            },
        }),
    )
    .await;
    let other_algorithm = served(
        &emulator,
        case,
        region,
        "DescribeAlgorithm",
        &json!({ "AlgorithmName": "other-tag-algorithm" }),
    )
    .await;
    let reported = other_algorithm
        .pointer("/TrainingSpecification/TrainingImageDigest")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a digest, got {other_algorithm}"));
    assert!(
        is_image_digest(reported),
        "case {case:?}: expected a sha256 digest, got {reported:?}"
    );
    assert_ne!(
        reported, PUBLISHED_DIGEST,
        "case {case:?}: another tag of the repository is another image, got {reported:?}"
    );
}
