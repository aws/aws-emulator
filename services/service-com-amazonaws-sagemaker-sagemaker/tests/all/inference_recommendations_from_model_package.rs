// SPDX-License-Identifier: CC0-1.0

//! The Inference Recommender flow AWS documents for the MODEL REGISTRY: register a model
//! package, start an `Advanced` load test against it, wait for the job to complete, then ask
//! `GetScalingConfigurationRecommendation` for the scaling configuration of one of the
//! recommendation ids the job reported.
//!
//! RULE — a recommendation job's model under test is the one its `InputConfig` names, however it
//! names it: `$ModelName` is "the name of the created model" and `$ModelPackageVersionArn` is
//! "the Amazon Resource Name (ARN) of a versioned model package", and a model package carries
//! "the Amazon ECR paths of containers that contain the inference code and model artifacts" in
//! its inference specification — the one
//! `EndpointInputConfiguration$InferenceSpecificationName` ("the inference specification name
//! in the model package version") selects. So a job that names a model package this account
//! holds is tested on the containers that specification carries, reaches `COMPLETED` with one
//! recommendation per endpoint configuration it tested, and the recommendation ids it reports
//! are the ones a scaling configuration is computed from; a container that names no model data
//! at all brings its own model — `ModelDataUrl` and `ModelDataSource` are both optional — so it
//! is tested like any other and its recommendation simply carries no metrics, while a job
//! naming a model package this account holds NOTHING of has no container to put under test and
//! ends in the modelled `FAILED` status instead.
//!
//! The rows span what the rule generalises over: how many endpoint configurations are under
//! test, which instance type each declares, which of the package's inference specifications the
//! test deploys, and whether the named package resolves to a container and to an artifact at
//! all.
//!
//! Driven over the emulator's own Amazon S3 and IAM mounted beside SageMaker on ONE shared
//! engine, through the real wire, on the training-run suite's harness
//! ([`crate::training_job_execution`]): the artifact the package is registered around is one a
//! training job of this emulator really wrote.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message};
// The poll-until helper and the terminal-status list this crate already has for a job driven
// to its end on this harness, shared rather than copied.
use crate::managed_job_orchestration::{ended, TERMINAL_RECOMMENDATION_STATUSES};
use crate::training_job_execution::{
    channel, csv_records, read_and_write, str_field, training_body, Emulator, BUCKET,
};

/// The `TrainingJobStatus` members a training job's work has ENDED in.
const TERMINAL_TRAINING_STATUSES: [&str; 3] = ["Completed", "Failed", "Stopped"];

/// The account every ARN below is scoped to, and whose Amazon ECR registry the inference
/// container images address — the one registry a model package may be registered around.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The name of the additional inference specification one of the packages carries, as
/// `AdditionalInferenceSpecificationDefinition$Name` spells it.
const ADDITIONAL_SPECIFICATION: &str = "tuned";

/// A well-formed `ModelPackageArn` (pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:model-package/[\S]{1,2048}$`) that
/// this account holds no package for.
fn unheld_package_arn() -> String {
    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package/absent-group/7")
}

/// Train a model of this emulator's own and answer `(role ARN, the artifact it wrote)` — the
/// artifact a model package is then registered around.
async fn given_trained_artifact(emulator: &Emulator, name: &str) -> (String, String) {
    let role_arn = emulator
        .given_role("sagemaker-recommender-role", Some(&read_and_write()))
        .await;
    let key = format!("{name}/train/data.csv");
    emulator
        .given_object(BUCKET, &key, &csv_records(24, 3))
        .await;
    let body = training_body(
        name,
        &role_arn,
        json!([channel("train", &format!("s3://{BUCKET}/{key}"), "S3Prefix")]),
        &format!("s3://{BUCKET}/{name}/output/"),
    );
    emulator.sagemaker_ok("CreateTrainingJob", &body).await;
    let trained = ended(
        emulator,
        "DescribeTrainingJob",
        &json!({ "TrainingJobName": name }),
        "TrainingJobStatus",
        &TERMINAL_TRAINING_STATUSES,
        name,
    )
    .await;
    assert_eq!(
        str_field(&trained, "TrainingJobStatus"),
        "Completed",
        "case {name:?}: the model under test has to be trained first, got {trained}"
    );
    let artifact = trained
        .pointer("/ModelArtifacts/S3ModelArtifacts")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: no artifact in {trained}"))
        .to_string();
    (role_arn, artifact)
}

/// One inference container of a model package: an image in this account's own Amazon ECR
/// registry in the request's Region — the registry a registration is admitted around — and the
/// `ModelDataUrl` the container serves from, when the case gives it one.
fn container(artifact: Option<&str>) -> Value {
    let mut container = json!({
        "Image": format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/inference:1"),
    });
    if let Some(artifact) = artifact {
        container["ModelDataUrl"] = json!(artifact);
    }
    container
}

/// Register a versioned model package in its own group and answer its `ModelPackageArn`.
///
/// `own_specification` is the artifact the package's own `InferenceSpecification` container
/// names, and `additional` the one carried by its additional inference specification — so a
/// case can put the artifact behind either and name the specification it wants benchmarked.
async fn given_package(
    emulator: &Emulator,
    group: &str,
    own_specification: Option<&str>,
    additional: Option<&str>,
) -> String {
    emulator
        .sagemaker_ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": group }),
        )
        .await;
    let mut body = json!({
        "ModelPackageGroupName": group,
        "ModelApprovalStatus": "Approved",
        "InferenceSpecification": {
            "Containers": [container(own_specification)],
            "SupportedContentTypes": ["text/csv"],
            "SupportedResponseMIMETypes": ["text/csv"],
            "SupportedRealtimeInferenceInstanceTypes": ["ml.m5.xlarge"],
        },
    });
    if let Some(artifact) = additional {
        body["AdditionalInferenceSpecifications"] = json!([{
            "Name": ADDITIONAL_SPECIFICATION,
            "Containers": [container(Some(artifact))],
            "SupportedContentTypes": ["text/csv"],
            "SupportedResponseMIMETypes": ["text/csv"],
            "SupportedRealtimeInferenceInstanceTypes": ["ml.m5.xlarge"],
        }]);
    }
    let registered = emulator.sagemaker_ok("CreateModelPackage", &body).await;
    let arn = str_field(&registered, "ModelPackageArn");
    assert!(
        !arn.is_empty(),
        "case {group:?}: the registration must answer a ModelPackageArn, got {registered}"
    );
    arn
}

/// One case: a job naming a model package, the endpoint configurations it puts under test and
/// how many recommendations that has to produce (none for a job with nothing to benchmark),
/// plus whether those recommendations carry MEASURED metrics.
struct Row {
    case: &'static str,
    /// The `InstanceType` of each `EndpointConfigurations` entry; empty declares none at all.
    instance_types: &'static [&'static str],
    /// `EndpointInputConfiguration$InferenceSpecificationName`, when the test names one.
    specification: Option<&'static str>,
    recommendations: usize,
    /// Whether the invocations of each test measured metrics: a container that declares no
    /// model data of its own brings its own model, which this emulator does not run, so its
    /// test is reported with no metrics on it.
    measured: bool,
}

/// The `CreateInferenceRecommendationsJob` body of one row: an `Advanced` load test — the only
/// job type that takes `TrafficPattern` and `ResourceLimit` — against `package`.
fn create_body(name: &str, role_arn: &str, package: &str, row: &Row) -> Value {
    let mut input = json!({
        "ModelPackageVersionArn": package,
        // `TrafficPattern` "defines the traffic pattern of the load test" (smithy_query
        // shape=TrafficPattern); `PHASES` is one of its modelled `TrafficType` members.
        "TrafficPattern": {
            "TrafficType": "PHASES",
            "Phases": [{ "InitialNumberOfUsers": 1, "SpawnRate": 1, "DurationInSeconds": 120 }],
        },
        "JobDurationInSeconds": 1800,
    });
    if !row.instance_types.is_empty() {
        input["EndpointConfigurations"] = row
            .instance_types
            .iter()
            .map(|instance_type| {
                let mut entry = json!({ "InstanceType": instance_type });
                if let Some(specification) = row.specification {
                    entry["InferenceSpecificationName"] = json!(specification);
                }
                entry
            })
            .collect();
        input["ResourceLimit"] = json!({
            "MaxNumberOfTests": row.instance_types.len(),
            "MaxParallelOfTests": 1,
        });
    }
    json!({
        "JobName": name,
        "JobType": "Advanced",
        "RoleArn": role_arn,
        "InputConfig": input,
        "StoppingConditions": {
            "MaxInvocations": 30,
            "ModelLatencyThresholds": [{ "Percentile": "P99", "ValueInMilliseconds": 50_000 }],
        },
    })
}

/// THE RULE: an `Advanced` job that names its model under test by a model package ARN is
/// tested on the containers that package's named inference specification carries, completes
/// with one recommendation per endpoint configuration it tested — carrying metrics where its
/// invocations measured any — and those recommendation ids are what
/// `GetScalingConfigurationRecommendation` computes from.
#[tokio::test]
async fn an_advanced_job_benchmarks_the_model_package_it_names() {
    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    let (role_arn, artifact) = given_trained_artifact(&emulator, "recpkg").await;
    // One package whose OWN inference specification carries the artifact, and one whose
    // artifact is carried only by an additional inference specification.
    let own = given_package(&emulator, "RecPkgOwn", Some(&artifact), None).await;
    let extra = given_package(&emulator, "RecPkgExtra", None, Some(&artifact)).await;
    let absent = unheld_package_arn();

    let rows: [(Row, &String); 5] = [
        (
            Row {
                case: "one endpoint configuration under test",
                instance_types: &["ml.m5.xlarge"],
                specification: None,
                recommendations: 1,
                measured: true,
            },
            &own,
        ),
        (
            Row {
                case: "two endpoint configurations, one test each",
                instance_types: &["ml.m5.xlarge", "ml.c5.large"],
                specification: None,
                recommendations: 2,
                measured: true,
            },
            &own,
        ),
        (
            Row {
                case: "no endpoint configuration declared at all",
                instance_types: &[],
                specification: None,
                recommendations: 1,
                measured: true,
            },
            &own,
        ),
        (
            Row {
                case: "the additional inference specification the test names",
                instance_types: &["ml.m5.xlarge"],
                specification: Some(ADDITIONAL_SPECIFICATION),
                recommendations: 1,
                measured: true,
            },
            &extra,
        ),
        (
            Row {
                // A container that names no model data is a container that brings its own
                // model, which `ModelDataUrl` and `ModelDataSource` being optional makes a
                // valid registration: the job tests it rather than failing for want of
                // artifacts, and reports its test with no metrics measured off it.
                case: "a package whose own specification names no artifact",
                instance_types: &["ml.m5.xlarge"],
                specification: None,
                recommendations: 1,
                measured: false,
            },
            &extra,
        ),
    ];

    for (index, (row, package)) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("recjob{index}");
        emulator
            .sagemaker_ok(
                "CreateInferenceRecommendationsJob",
                &create_body(&name, &role_arn, package, row),
            )
            .await;

        let settled = ended(
            &emulator,
            "DescribeInferenceRecommendationsJob",
            &json!({ "JobName": name }),
            "Status",
            &TERMINAL_RECOMMENDATION_STATUSES,
            case,
        )
        .await;

        if row.recommendations == 0 {
            assert_eq!(
                str_field(&settled, "Status"),
                "FAILED",
                "case {case:?}: a job with no container to put under test must fail, got \
                 {settled}"
            );
            assert!(
                !str_field(&settled, "FailureReason").is_empty(),
                "case {case:?}: a FAILED job must say why, got {settled}"
            );
            assert!(
                settled.get("InferenceRecommendations").is_none(),
                "case {case:?}: a job that put nothing under test reports no recommendation, got \
                 {settled}"
            );
            // And nothing can be computed from a job that recommended nothing.
            let (status, refused) = emulator
                .sagemaker(
                    "GetScalingConfigurationRecommendation",
                    &json!({
                        "InferenceRecommendationsJobName": name,
                        "EndpointName": format!("sm-ep-{name}-001"),
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&refused).as_str(), message(&refused).as_str()),
                (
                    400,
                    "ResourceNotFound",
                    format!(
                        "Inference recommendations job {name} has no recommendation for the \
                         requested endpoint or recommendation id."
                    )
                    .as_str()
                ),
                "case {case:?}: expected the modelled miss, got {status} with {refused}"
            );
            continue;
        }

        assert_eq!(
            str_field(&settled, "Status"),
            "COMPLETED",
            "case {case:?}: the job tested the package's containers, so it completes, got \
             {settled}"
        );
        let measured = settled
            .get("InferenceRecommendations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            measured.len(),
            row.recommendations,
            "case {case:?}: one recommendation per endpoint tested, got {settled}"
        );

        for (test, recommendation) in measured.iter().enumerate() {
            // The endpoint SageMaker deployed for this test, and the ML compute it ran on.
            assert_eq!(
                recommendation.pointer("/EndpointConfiguration/EndpointName"),
                Some(&json!(format!("sm-ep-{name}-{:03}", test + 1))),
                "case {case:?} test {test}: the job reports the endpoint it deployed, got \
                 {recommendation}"
            );
            assert_eq!(
                recommendation
                    .pointer("/EndpointConfiguration/InstanceType")
                    .and_then(Value::as_str),
                row.instance_types.get(test).copied(),
                "case {case:?} test {test}: the test ran on the instance type under test, got \
                 {recommendation}"
            );
            let latency = recommendation
                .pointer("/Metrics/ModelLatency")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let invocations = recommendation
                .pointer("/Metrics/MaxInvocations")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            if row.measured {
                assert!(
                    latency >= 1 && invocations >= 1,
                    "case {case:?} test {test}: the metrics are the ones the invocations \
                     measured, got {recommendation}"
                );
            } else {
                assert!(
                    recommendation.get("Metrics").is_none(),
                    "case {case:?} test {test}: a test that measured nothing reports no \
                     metric, got {recommendation}"
                );
            }

            // And the scaling configuration is computed from THAT recommendation, resolved by
            // the id the job reported for it.
            let recommendation_id = recommendation
                .get("RecommendationId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                !recommendation_id.is_empty(),
                "case {case:?} test {test}: a reported recommendation carries an id, got \
                 {recommendation}"
            );
            let scaling = emulator
                .sagemaker_ok(
                    "GetScalingConfigurationRecommendation",
                    &json!({
                        "InferenceRecommendationsJobName": name,
                        "RecommendationId": recommendation_id,
                    }),
                )
                .await;
            assert_eq!(
                (
                    str_field(&scaling, "InferenceRecommendationsJobName"),
                    str_field(&scaling, "RecommendationId"),
                ),
                (name.clone(), recommendation_id.clone()),
                "case {case:?} test {test}: the answer names what it was computed from, got \
                 {scaling}"
            );
            let min_capacity = scaling
                .pointer("/DynamicScalingConfiguration/MinCapacity")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let max_capacity = scaling
                .pointer("/DynamicScalingConfiguration/MaxCapacity")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            assert!(
                min_capacity >= 1 && max_capacity >= min_capacity,
                "case {case:?} test {test}: expected 1 <= MinCapacity <= MaxCapacity, got \
                 {scaling}"
            );
            if row.measured {
                assert_eq!(
                    scaling.pointer("/Metric/ModelLatency"),
                    Some(&json!(latency)),
                    "case {case:?} test {test}: the metric it reports is the measured \
                     latency, got {scaling}"
                );
            } else {
                assert!(
                    scaling.get("Metric").is_none(),
                    "case {case:?} test {test}: nothing was measured, so the scaling \
                     configuration states no metric, got {scaling}"
                );
            }

            // The same recommendation is reachable by the endpoint it was measured on, which is
            // the other way the request may pick one.
            let by_endpoint = emulator
                .sagemaker_ok(
                    "GetScalingConfigurationRecommendation",
                    &json!({
                        "InferenceRecommendationsJobName": name,
                        "EndpointName": format!("sm-ep-{name}-{:03}", test + 1),
                    }),
                )
                .await;
            assert_eq!(
                str_field(&by_endpoint, "RecommendationId"),
                recommendation_id,
                "case {case:?} test {test}: EndpointName picks the recommendation reported for \
                 that endpoint, got {by_endpoint}"
            );
        }
    }

    // A job naming a model package this account holds none of has nothing to deploy for its
    // test at all — no container, so not even a container that brings its own model — so it
    // puts nothing under test and ends FAILED.
    let case = "a model package this account does not hold";
    let name = "recjob-absent";
    let row = Row {
        case,
        instance_types: &["ml.m5.xlarge"],
        specification: None,
        recommendations: 0,
        measured: false,
    };
    emulator
        .sagemaker_ok(
            "CreateInferenceRecommendationsJob",
            &create_body(name, &role_arn, &absent, &row),
        )
        .await;
    let settled = ended(
        &emulator,
        "DescribeInferenceRecommendationsJob",
        &json!({ "JobName": name }),
        "Status",
        &TERMINAL_RECOMMENDATION_STATUSES,
        case,
    )
    .await;
    assert_eq!(
        str_field(&settled, "Status"),
        "FAILED",
        "case {case:?}: a job with nothing to benchmark must fail, got {settled}"
    );
    assert!(
        settled.get("InferenceRecommendations").is_none(),
        "case {case:?}: no recommendation is reported for a job that measured nothing, got \
         {settled}"
    );
}
