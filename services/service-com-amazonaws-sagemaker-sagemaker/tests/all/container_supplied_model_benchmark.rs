// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for an Inference Recommender job over a model package whose
//! container brings its OWN model.
//!
//! RULE: model artifacts are OPTIONAL on an inference container — `ModelDataUrl`,
//! `ModelDataSource` and `ProductId` are all optional members of
//! `ModelPackageContainerDefinition` (smithy_query query="shape"
//! shape="ModelPackageContainerDefinition") — so a container that loads its own model (one
//! driven by its `Environment`, one identified by a `ProductId`, one with the model baked into
//! the image) registers with no artifacts at all and is a valid package. A load test over such
//! a package is therefore NOT refused for want of artifacts: the job reaches its terminal
//! `COMPLETED` status with one recommendation per endpoint configuration it put under test,
//! each carrying a `RecommendationId` that `GetScalingConfigurationRecommendation` resolves to
//! a `DynamicScalingConfiguration` whose `MaxCapacity` is at least its `MinCapacity`, which is
//! at least the one instance an endpoint runs on.
//!
//! What this emulator cannot do is RUN the container image, so nothing invokes such a model
//! here and no metric is reported for it: `RecommendationMetrics` is an optional member of a
//! recommendation, and a number no invocation produced would be no measurement. A package
//! whose container DOES name an artifact is measured, which
//! [`crate::inference_recommendations_from_model_package`] asserts beside this suite.
//!
//! The rows span what the rule generalises over: HOW the container supplies its own model
//! (environment variables, a Marketplace `ProductId`, the image alone), how many endpoint
//! configurations the job puts under test (none declared, one, two), and whether the job's own
//! resource limit admits them.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`)
//! on the harness the `inference_deployment_lifecycle` suite stands up.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The region every call below signs for.
const REGION: &str = "us-east-1";

/// How long a job is polled for a terminal status, and how often. The CONDITION is re-read in
/// a loop — nothing here asserts how long the job took — so a loaded host cannot decide a case.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const SETTLE_POLL: Duration = Duration::from_millis(25);

/// The `RecommendationJobStatus` members a job's work has ENDED in.
const TERMINAL_STATUSES: [&str; 3] = ["COMPLETED", "FAILED", "STOPPED"];

/// A framework inference image of the AWS Deep Learning Containers registry, the kind of
/// container that loads its own weights.
fn framework_image() -> String {
    format!(
        "{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/\
         huggingface-pytorch-inference:1.13.1-transformers4.26.0-cpu-py39-ubuntu20.04"
    )
}

/// The role the job runs under.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/inference-recommender")
}

/// Register `group`/1 around `container` and answer the version's ARN.
async fn given_package(emulator: &Emulator, group: &str, container: Value) -> String {
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": group }),
        )
        .await;
    let registered = emulator
        .ok(
            "CreateModelPackage",
            &json!({
                "ModelPackageGroupName": group,
                "ModelApprovalStatus": "Approved",
                "InferenceSpecification": {
                    "Containers": [container],
                    "SupportedContentTypes": ["application/json"],
                    "SupportedResponseMIMETypes": ["application/json"],
                    "SupportedRealtimeInferenceInstanceTypes": ["ml.m5.xlarge"],
                },
            }),
        )
        .await;
    let arn = str_field(&registered, "ModelPackageArn");
    assert!(
        !arn.is_empty(),
        "setup {group}: the registration must answer a ModelPackageArn, got {registered}"
    );
    arn
}

/// The `CreateInferenceRecommendationsJob` body of one case: an `Advanced` load test — the job
/// type that takes a traffic pattern and a resource limit — over `package`.
fn create_body(name: &str, package: &str, instance_types: &[&str]) -> Value {
    let mut input = json!({
        "ModelPackageVersionArn": package,
        "ContainerConfig": {
            "Domain": "NATURAL_LANGUAGE_PROCESSING",
            "Task": "CLASSIFICATION",
            "Framework": "PYTORCH",
            "FrameworkVersion": "1.13",
            "NearestModelName": "bert-base-cased",
            "SupportedInstanceTypes": ["ml.m5.xlarge"],
        },
        "TrafficPattern": {
            "TrafficType": "PHASES",
            "Phases": [{ "InitialNumberOfUsers": 1, "SpawnRate": 1, "DurationInSeconds": 120 }],
        },
        "JobDurationInSeconds": 1800,
    });
    if !instance_types.is_empty() {
        input["EndpointConfigurations"] = instance_types
            .iter()
            .map(|instance_type| json!({ "InstanceType": instance_type }))
            .collect();
        input["ResourceLimit"] = json!({
            "MaxNumberOfTests": instance_types.len(),
            "MaxParallelOfTests": 1,
        });
    }
    json!({
        "JobName": name,
        "JobType": "Advanced",
        "RoleArn": role_arn(),
        "InputConfig": input,
        "StoppingConditions": {
            "MaxInvocations": 30,
            "ModelLatencyThresholds": [{ "Percentile": "P99", "ValueInMilliseconds": 50_000 }],
        },
    })
}

/// Poll the job `name` until it reports a terminal status, and answer that read.
async fn ended(emulator: &Emulator, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = emulator
            .ok(
                "DescribeInferenceRecommendationsJob",
                &json!({ "JobName": name }),
            )
            .await;
        if TERMINAL_STATUSES.contains(&str_field(&read, "Status").as_str()) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal Status; last read {read}"
        );
        tokio::time::sleep(SETTLE_POLL).await;
    }
}

/// THE RULE: a package whose container declares no model data is tested, not refused.
#[tokio::test]
async fn a_container_that_brings_its_own_model_completes_the_load_test() {
    // (case, the container the package is registered around, the instance types under test)
    let rows: Vec<(&str, Value, &[&str])> = vec![
        (
            "a container told which model to load by its environment",
            json!({
                "Image": framework_image(),
                "Environment": {
                    "HF_MODEL_ID": "hf-internal-testing/tiny-random-distilbert",
                    "HF_TASK": "text-classification",
                },
            }),
            &["ml.m5.xlarge"],
        ),
        (
            "a Marketplace container identified by its ProductId",
            json!({
                "Image": framework_image(),
                "ProductId": "prod-abcdefghijklm",
            }),
            &["ml.m5.xlarge", "ml.c5.large"],
        ),
        (
            "a container carrying nothing but its image, with no configuration under test",
            json!({ "Image": framework_image() }),
            &[],
        ),
    ];

    for (index, (case, container, instance_types)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let group = format!("own-model-{index}");
        let package = given_package(&emulator, &group, container).await;
        let name = format!("recjob-own-{index}");
        emulator
            .ok(
                "CreateInferenceRecommendationsJob",
                &create_body(&name, &package, instance_types),
            )
            .await;

        let settled = ended(&emulator, case, &name).await;
        assert_eq!(
            str_field(&settled, "Status"),
            "COMPLETED",
            "case {case:?}: a container that brings its own model is a valid package, so the \
             job completes, got {settled}"
        );
        assert!(
            settled.get("FailureReason").is_none(),
            "case {case:?}: a completed job states no failure, got {settled}"
        );
        let reported = settled
            .get("InferenceRecommendations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // A job declaring no endpoint configuration at all is still one test.
        let expected = instance_types.len().max(1);
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: one recommendation per endpoint configuration under test, got \
             {settled}"
        );

        for (test, recommendation) in reported.iter().enumerate() {
            assert!(
                recommendation.get("Metrics").is_none(),
                "case {case:?} test {test}: nothing invoked the container's own model, so no \
                 metric is reported, got {recommendation}"
            );
            assert_eq!(
                recommendation
                    .pointer("/EndpointConfiguration/InstanceType")
                    .and_then(Value::as_str),
                instance_types.get(test).copied(),
                "case {case:?} test {test}: the test names the ML compute it was planned on, \
                 got {recommendation}"
            );
            let recommendation_id = str_field(recommendation, "RecommendationId");
            assert!(
                !recommendation_id.is_empty(),
                "case {case:?} test {test}: every recommendation carries an id, got \
                 {recommendation}"
            );

            // And that id resolves to a scaling configuration a client can act on.
            let scaling = emulator
                .ok(
                    "GetScalingConfigurationRecommendation",
                    &json!({
                        "InferenceRecommendationsJobName": name,
                        "RecommendationId": recommendation_id,
                    }),
                )
                .await;
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
            assert!(
                scaling.get("Metric").is_none(),
                "case {case:?} test {test}: with nothing measured the configuration states no \
                 metric, got {scaling}"
            );

            // The same recommendation is reachable by the endpoint the test was planned on,
            // which is the other way a request picks one.
            let endpoint_name = recommendation
                .pointer("/EndpointConfiguration/EndpointName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let by_endpoint = emulator
                .ok(
                    "GetScalingConfigurationRecommendation",
                    &json!({
                        "InferenceRecommendationsJobName": name,
                        "EndpointName": endpoint_name,
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
}

/// RULE: the capacity band a scaling configuration recommends is at least the one instance an
/// endpoint runs on, whatever traffic objective the caller anticipates — a recommendation with
/// no measured load states no per-instance figure to divide that traffic by, and states none
/// rather than inventing one.
#[tokio::test]
async fn a_scaling_configuration_over_an_unmeasured_recommendation_still_recommends_capacity() {
    let emulator = Emulator::new();
    let package = given_package(
        &emulator,
        "own-model-scaling",
        json!({
            "Image": framework_image(),
            "Environment": { "HF_TASK": "text-classification" },
        }),
    )
    .await;
    let name = "recjob-own-scaling";
    emulator
        .ok(
            "CreateInferenceRecommendationsJob",
            &create_body(name, &package, &["ml.m5.xlarge"]),
        )
        .await;
    let settled = ended(&emulator, "the scaling rows", name).await;
    assert_eq!(
        str_field(&settled, "Status"),
        "COMPLETED",
        "setup: the job must complete, got {settled}"
    );
    let recommendation_id = settled
        .get("InferenceRecommendations")
        .and_then(Value::as_array)
        .and_then(|reported| reported.first())
        .map(|first| str_field(first, "RecommendationId"))
        .unwrap_or_default();
    assert!(
        !recommendation_id.is_empty(),
        "setup: the job must report a recommendation, got {settled}"
    );

    // Rows differ along the traffic objective the request anticipates and the utilisation it
    // asks to run at.
    let rows: Vec<(&str, Value)> = vec![
        ("no objective at all", json!({})),
        (
            "a modest objective",
            json!({
                "ScalingPolicyObjective": { "MinInvocationsPerMinute": 10, "MaxInvocationsPerMinute": 100 },
            }),
        ),
        (
            "a large objective at a low utilisation",
            json!({
                "TargetCpuUtilizationPerCore": 20,
                "ScalingPolicyObjective": {
                    "MinInvocationsPerMinute": 1_000,
                    "MaxInvocationsPerMinute": 100_000,
                },
            }),
        ),
    ];
    for (case, extra) in rows {
        let mut body = json!({
            "InferenceRecommendationsJobName": name,
            "RecommendationId": recommendation_id,
        });
        for (member, value) in extra.as_object().expect("the row is an object") {
            body[member] = value.clone();
        }
        let scaling = emulator
            .ok("GetScalingConfigurationRecommendation", &body)
            .await;
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
            "case {case:?}: expected 1 <= MinCapacity <= MaxCapacity, got {scaling}"
        );
    }

    // A recommendation id the job does not hold is still the modelled miss, so the answers
    // above are about the recommendation the job reported.
    let (status, refused) = emulator
        .call(
            "GetScalingConfigurationRecommendation",
            &json!({
                "InferenceRecommendationsJobName": name,
                "RecommendationId": "no-such-recommendation",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str(), message(&refused).as_str()),
        (
            400,
            "ResourceNotFound",
            format!(
                "Inference recommendations job {name} has no recommendation for the requested \
                 endpoint or recommendation id."
            )
            .as_str()
        ),
        "case an id the job does not hold: expected the modelled miss, got {status} with \
         {refused}"
    );
}
