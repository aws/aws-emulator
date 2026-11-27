// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's Inference Recommender job lifecycle:
//! `CreateInferenceRecommendationsJob` -> `DescribeInferenceRecommendationsJob`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! optional member and both job types; the single clock-owed move out of `PENDING`; the
//! refusal of a name already in use and of an unknown name; and the request-shape rules
//! the Smithy model states in prose (`smithy_query(query="shape",
//! shape="RecommendationJobInputConfig")`), which the schema cannot express and the
//! handler therefore enforces.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator, one request builder
// and one set of body accessors.
use crate::inference_deployment_lifecycle::{
    error_code, message, one_variant_config, primary_container, str_field, Emulator, ACCOUNT,
};

/// Long enough for a created job's single move to be due, whenever it falls
/// (`state::recommendation_job_settles_at`), so a read past this point reports a terminal
/// status.
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerRecommenderRole";

/// A `ModelPackageArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:model-package/[\S]{1,2048}$`.
const MODEL_PACKAGE_VERSION_ARN: &str =
    "arn:aws:sagemaker:us-east-1:000000000000:model-package/ranker-group/3";

/// The model every job in these tests benchmarks, created before the job is.
const MODEL: &str = "recommender-model";

/// An endpoint an `InputConfig.Endpoints` row can name, created before the job is.
const ENDPOINT: &str = "recommender-endpoint";

/// Register `MODEL` in `region`, so a job may name it.
async fn seed_model(emulator: &Emulator, region: &str) {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateModel",
            &json!({
                "ModelName": MODEL,
                "PrimaryContainer": primary_container(
                    "000000000000.dkr.ecr.us-east-1.amazonaws.com/ranker:1",
                ),
                "ExecutionRoleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(status, 200, "seeding {MODEL}: got {status} with {created}");
}

/// Register `ENDPOINT` (and the configuration it needs) in `us-east-1`.
async fn seed_endpoint(emulator: &Emulator) {
    emulator
        .ok("CreateEndpointConfig", &one_variant_config("recommender-config", MODEL))
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": ENDPOINT, "EndpointConfigName": "recommender-config" }),
        )
        .await;
}

/// A minimal create request: every required member, naming the seeded model.
fn minimal_create(name: &str) -> Value {
    json!({
        "JobName": name,
        // `RecommendationJobType` is `Default | Advanced` (smithy_query
        // operation=CreateInferenceRecommendationsJob).
        "JobType": "Default",
        "RoleArn": ROLE_ARN,
        "InputConfig": { "ModelName": MODEL },
    })
}

/// `minimal_create`, with `extra` merged over the request and `extra_input` merged into
/// its `InputConfig`.
fn create_with(name: &str, extra: &Value, extra_input: &Value) -> Value {
    let mut body = minimal_create(name);
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        body[key] = value.clone();
    }
    for (key, value) in extra_input.as_object().expect("a JSON object of InputConfig members") {
        body["InputConfig"][key] = value.clone();
    }
    body
}

/// `DescribeInferenceRecommendationsJob` for `name`, as the raw (status, body) pair, so a
/// test can assert on a refusal as well as on a stored job.
async fn describe(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call("DescribeInferenceRecommendationsJob", &json!({ "JobName": name }))
        .await
}

/// RULE: a created recommendation job's ARN is
/// `arn:aws:sagemaker:<signed region>:<account>:inference-recommendations-job/<JobName>`,
/// and the job the matching `Describe` replays carries that same ARN, whatever the name's
/// shape and whichever region the caller signed for.
#[tokio::test]
async fn minted_arn_names_the_signed_region_the_account_and_the_job_name() {
    // Rows differ along the axes the rule generalises over: the name's shape (one
    // character, digits, hyphens, and the 64-character modelled maximum) and the region.
    let names = [
        "r",
        "job7",
        "nightly-inference-recommendation",
        // `RecommendationJobName` is `len 1..64` (smithy_query
        // operation=CreateInferenceRecommendationsJob), so this row sits on the upper
        // bound.
        "a2345678901234567890123456789012345678901234567890123456789012b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            seed_model(&emulator, region).await;
            let case = format!("region={region} name={name}");
            let expected = format!(
                "arn:aws:sagemaker:{region}:{ACCOUNT}:inference-recommendations-job/{name}"
            );

            let (status, created) = emulator
                .call_in(region, "CreateInferenceRecommendationsJob", &minimal_create(name))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: CreateInferenceRecommendationsJob, got {status} with {created}"
            );
            assert_eq!(
                str_field(&created, "JobArn"),
                expected,
                "case {case}: minted JobArn, got {created}"
            );

            let (status, described) = emulator
                .call_in(
                    region,
                    "DescribeInferenceRecommendationsJob",
                    &json!({ "JobName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeInferenceRecommendationsJob, got {status} with {described}"
            );
            assert_eq!(
                str_field(&described, "JobArn"),
                expected,
                "case {case}: replayed JobArn, got {described}"
            );
            assert_eq!(
                str_field(&described, "JobName"),
                name,
                "case {case}: replayed JobName, got {described}"
            );
        }
    }
}

/// RULE: a described recommendation job replays exactly what its create carried — the job
/// type, the role, the description, the stopping conditions and every member of the input
/// configuration — and nothing the create did not carry.
#[tokio::test]
async fn describe_replays_every_member_the_create_carried() {
    // Rows differ along the axes the rule generalises over: which optional members the
    // request carries, which model source it names, which traffic pattern it asks for,
    // and whether the job is an instance recommendation or a load test.
    let rows: [(&str, Value, Value); 11] = [
        ("minimal", json!({}), json!({})),
        (
            "job-description",
            json!({ "JobDescription": "nightly instance recommendation" }),
            json!({}),
        ),
        (
            "advanced-load-test",
            json!({
                "JobType": "Advanced",
                "StoppingConditions": {
                    "MaxInvocations": 500,
                    "ModelLatencyThresholds": [
                        { "Percentile": "P95", "ValueInMilliseconds": 100 },
                    ],
                    // `FlatInvocations` is `Continue | Stop` (smithy_query
                    // shape=FlatInvocations).
                    "FlatInvocations": "Stop",
                },
            }),
            json!({}),
        ),
        (
            "output-config",
            json!({
                "OutputConfig": {
                    "KmsKeyId": "alias/ExampleAlias",
                    "CompiledOutputConfig": { "S3OutputUri": "s3://bucket/compiled" },
                },
            }),
            json!({}),
        ),
        ("tags", json!({ "Tags": [{ "Key": "team", "Value": "ml" }] }), json!({})),
        (
            "model-package-version",
            json!({}),
            // The other model source: this row drops `ModelName` below.
            json!({ "ModelPackageVersionArn": MODEL_PACKAGE_VERSION_ARN }),
        ),
        (
            "traffic-phases",
            // A traffic pattern and a test budget configure the LOAD TEST, which only the
            // `Advanced` job type runs, so this row asks for that type.
            json!({ "JobType": "Advanced" }),
            json!({
                "JobDurationInSeconds": 7200,
                // `TrafficType` is `PHASES | STAIRS` (smithy_query shape=TrafficType).
                "TrafficPattern": {
                    "TrafficType": "PHASES",
                    "Phases": [
                        { "InitialNumberOfUsers": 1, "SpawnRate": 1, "DurationInSeconds": 120 },
                    ],
                },
                "ResourceLimit": { "MaxNumberOfTests": 5, "MaxParallelOfTests": 2 },
            }),
        ),
        (
            "traffic-stairs",
            json!({ "JobType": "Advanced" }),
            json!({
                "TrafficPattern": {
                    "TrafficType": "STAIRS",
                    "Stairs": {
                        "DurationInSeconds": 120,
                        "NumberOfSteps": 3,
                        // `UsersPerStep` is `range 1..3` (smithy_query shape=Stairs).
                        "UsersPerStep": 3,
                    },
                },
            }),
        ),
        (
            "endpoint-configurations",
            json!({}),
            json!({
                "EndpointConfigurations": [{
                    "InstanceType": "ml.m5.large",
                    "EnvironmentParameterRanges": {
                        "CategoricalParameterRanges": [
                            { "Name": "OMP_NUM_THREADS", "Value": ["2", "4"] },
                        ],
                    },
                }],
                "VolumeKmsKeyId": "alias/ExampleAlias",
            }),
        ),
        (
            "container-config",
            json!({}),
            json!({
                "ContainerConfig": {
                    "Domain": "NATURAL_LANGUAGE_PROCESSING",
                    "Task": "FILL_MASK",
                    "Framework": "PYTORCH",
                    "FrameworkVersion": "1.9",
                    "PayloadConfig": {
                        "SamplePayloadUrl": "s3://bucket/payload.tar.gz",
                        "SupportedContentTypes": ["application/json"],
                    },
                    "NearestModelName": "bert-base-cased",
                    "SupportedInstanceTypes": ["ml.m5.large"],
                    // `RecommendationJobSupportedEndpointType` is `RealTime | Serverless`
                    // (smithy_query shape=RecommendationJobSupportedEndpointType).
                    "SupportedEndpointType": "RealTime",
                    "SupportedResponseMIMETypes": ["application/json"],
                },
            }),
        ),
        (
            "existing-endpoint-and-vpc",
            json!({}),
            json!({
                "Endpoints": [{ "EndpointName": ENDPOINT }],
                "VpcConfig": {
                    "SecurityGroupIds": ["sg-0123456789abcdef"],
                    "Subnets": ["subnet-0123456789abc"],
                },
            }),
        ),
    ];

    for (case, extra, extra_input) in rows {
        let emulator = Emulator::new();
        seed_model(&emulator, "us-east-1").await;
        seed_endpoint(&emulator).await;

        let name = format!("job-{case}");
        let mut request = create_with(&name, &extra, &extra_input);
        if extra_input.get("ModelPackageVersionArn").is_some() {
            // Exactly one model source per request, so this row's source replaces the
            // default `ModelName`.
            request["InputConfig"]
                .as_object_mut()
                .expect("InputConfig is an object")
                .remove("ModelName");
        }

        let (status, created) = emulator
            .call("CreateInferenceRecommendationsJob", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateInferenceRecommendationsJob, got {status} with {created}"
        );

        let (status, described) = describe(&emulator, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeInferenceRecommendationsJob, got {status} with {described}"
        );
        assert_eq!(
            described.get("InputConfig"),
            request.get("InputConfig"),
            "case {case}: replayed InputConfig, got {described}"
        );
        for member in ["JobType", "RoleArn", "JobName"] {
            let expected = match member {
                "JobName" => Value::String(name.clone()),
                other => request[other].clone(),
            };
            assert_eq!(
                described.get(member),
                Some(&expected),
                "case {case}: replayed {member}, got {described}"
            );
        }
        for member in ["JobDescription", "StoppingConditions"] {
            assert_eq!(
                described.get(member),
                request.get(member),
                "case {case}: replayed {member}, got {described}"
            );
        }
        // The job's benchmark ran inside the create, so the terminal status the clock owes
        // from the instant that create answered is already decided
        // (`RecommendationJobStatus`, smithy_query shape=RecommendationJobStatus). Which one
        // it is follows from what the row put under test: the seeded model's container names
        // no model data, so the model it serves is the container image's own — a valid model
        // AWS benchmarks like any other, which nothing here invokes — while the row naming a
        // model package this account holds none of has no container under test at all.
        let nothing_under_test = extra_input.get("ModelPackageVersionArn").is_some();
        if nothing_under_test {
            assert_eq!(
                str_field(&described, "Status"),
                "FAILED",
                "case {case}: a job with nothing under test reports its failure, got {described}"
            );
            assert!(
                !str_field(&described, "FailureReason").is_empty(),
                "case {case}: a FAILED job must say what stopped it, got {described}"
            );
            for absent in ["InferenceRecommendations", "EndpointPerformances"] {
                assert!(
                    described.get(absent).is_none(),
                    "case {case}: {absent} must be absent when nothing was put under test, got \
                     {:?}",
                    described.get(absent)
                );
            }
        } else {
            assert_eq!(
                str_field(&described, "Status"),
                "COMPLETED",
                "case {case}: the first read reports what the create's own benchmark produced, \
                 got {described}"
            );
            assert!(
                described.get("FailureReason").is_none(),
                "case {case}: a COMPLETED job states no failure, got {described}"
            );
            // Nothing ran the container's own model, so the test reports no metric — and an
            // endpoint performance carries metrics by definition, so none is reported either.
            let reported = described
                .get("InferenceRecommendations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                reported.len(),
                1,
                "case {case}: one recommendation per endpoint put under test, got {described}"
            );
            assert!(
                reported[0].get("Metrics").is_none(),
                "case {case}: nothing invoked the container's own model, so no metric is \
                 reported, got {described}"
            );
            assert!(
                described.get("EndpointPerformances").is_none(),
                "case {case}: EndpointPerformances must be absent when nothing was measured, \
                 got {:?}",
                described.get("EndpointPerformances")
            );
        }
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime in {described}"));
        assert!(
            creation_time > 0.0,
            "case {case}: CreationTime must be the instant of the create, got {creation_time}"
        );
        let completion_time = described
            .get("CompletionTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CompletionTime in {described}"));
        assert!(
            completion_time >= creation_time,
            "case {case}: the job completed at or after it was created, got {described}"
        );
        assert_eq!(
            described.get("LastModifiedTime").and_then(Value::as_f64),
            Some(completion_time),
            "case {case}: LastModifiedTime is the instant the move was due, got {described}"
        );
    }
}

/// RULE: a job is accepted in the non-terminal `PENDING` status and owes its single move to
/// a terminal status to the clock — and that move is owed from the instant the create that
/// performed the job's own benchmark answers, so the FIRST read after it reports the terminal
/// status with the reason and the completion time stamped at the instant the move was due, and
/// further reads change nothing. A polling client therefore spends none of its interval on the
/// job (`state::recommendation_job_settles_at`).
#[tokio::test]
async fn the_first_read_after_the_create_reports_the_benchmark_the_create_ran() {
    let emulator = Emulator::new();
    seed_model(&emulator, "us-east-1").await;
    emulator
        .ok("CreateInferenceRecommendationsJob", &minimal_create("clock-owed"))
        .await;

    let (status, settled) = describe(&emulator, "clock-owed").await;
    assert_eq!(status, 200, "case settled: describe, got {status} with {settled}");
    let reached = str_field(&settled, "Status");
    // `RecommendationJobStatus`'s terminal members are COMPLETED | FAILED | STOPPED
    // (smithy_query shape=RecommendationJobStatus). This job's model carries no model data of
    // its own, so the model its container serves is the image's own: a valid model, put under
    // test like any other, which nothing here invokes.
    assert_eq!(
        reached, "COMPLETED",
        "case settled: the model under test is a valid one, so the run completes, got {settled}"
    );
    assert!(
        settled.get("FailureReason").is_none(),
        "case settled: a COMPLETED job states no failure, got {settled}"
    );
    // Nothing was measured, so no metric is reported and the job's own test was on an
    // endpoint it deployed rather than on an existing one.
    assert!(
        settled
            .pointer("/InferenceRecommendations/0/Metrics")
            .is_none(),
        "case settled: nothing invoked the model, so no metric is reported, got {settled}"
    );
    assert!(
        settled.get("EndpointPerformances").is_none(),
        "case settled: EndpointPerformances must be absent when nothing was measured, got {:?}",
        settled.get("EndpointPerformances")
    );
    let completion_time = settled
        .get("CompletionTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case settled: expected a CompletionTime in {settled}"));
    assert_eq!(
        settled.get("LastModifiedTime").and_then(Value::as_f64),
        Some(completion_time),
        "case settled: LastModifiedTime is the instant the move was due, got {settled}"
    );

    // Reading it again reports exactly the same job: the move happened once, at the instant
    // it was due, not on every read.
    for read in ["second", "third"] {
        let (status, again) = describe(&emulator, "clock-owed").await;
        assert_eq!(status, 200, "case read={read}: describe, got {status} with {again}");
        assert_eq!(
            again, settled,
            "case read={read}: a settled job is stable across reads, got {again}"
        );
    }

    // The transient status is the one the create STORES, and nothing a reader does moves it:
    // a job whose due instant has not arrived still reports `PENDING` with no completion time.
    {
        let state = emulator.engine.state::<
            service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState,
        >();
        let mut store = state.lock();
        let job = store
            .inference_recommendations_jobs
            .get_mut("clock-owed")
            .expect("the created job is in the store");
        job.status = "PENDING".to_string();
        job.failure_reason = None;
        job.completion_time = None;
        job.ends_at = ::std::time::SystemTime::now() + Duration::from_secs(3_600);
    }
    for read in ["first", "second"] {
        let (status, pending) = describe(&emulator, "clock-owed").await;
        assert_eq!(status, 200, "case pending read={read}: got {status} with {pending}");
        assert_eq!(
            str_field(&pending, "Status"),
            "PENDING",
            "case pending read={read}: a move that is not due must not happen because \
             something read the job, got {pending}"
        );
        assert!(
            pending.get("CompletionTime").is_none(),
            "case pending read={read}: CompletionTime must be absent while PENDING, got {pending}"
        );
    }
}

/// RULE: a job name is unique to the account in a Region — a create naming one already in
/// use is refused with `ResourceInUse` and leaves the stored job untouched — and a
/// describe of a name no job has is refused with `ResourceNotFound`.
#[tokio::test]
async fn a_name_in_use_is_refused_and_an_unknown_name_is_not_found() {
    // Rows differ along the name's shape, the axis the per-name rule generalises over.
    for name in ["r", "taken-job", "job9"] {
        let emulator = Emulator::new();
        seed_model(&emulator, "us-east-1").await;

        let first = emulator
            .ok("CreateInferenceRecommendationsJob", &minimal_create(name))
            .await;
        let first_arn = str_field(&first, "JobArn");

        let (status, refused) = emulator
            .call(
                "CreateInferenceRecommendationsJob",
                &create_with(name, &json!({ "JobDescription": "a second job" }), &json!({})),
            )
            .await;
        assert_eq!(status, 400, "case name={name}: re-create, got {status} with {refused}");
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case name={name}: re-create error code, got {refused}"
        );
        assert!(
            message(&refused).contains(name),
            "case name={name}: the refusal must name the job, got {refused}"
        );

        let (status, described) = describe(&emulator, name).await;
        assert_eq!(status, 200, "case name={name}: describe, got {status} with {described}");
        assert_eq!(
            str_field(&described, "JobArn"),
            first_arn,
            "case name={name}: the first job survives the refused re-create, got {described}"
        );
        assert!(
            described.get("JobDescription").is_none(),
            "case name={name}: the refused create must not have overwritten the job, got \
             {described}"
        );

        let (status, missing) = describe(&emulator, &format!("{name}-absent")).await;
        assert_eq!(
            status, 400,
            "case name={name}: describe of an unknown job, got {status} with {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFound",
            "case name={name}: unknown-job error code, got {missing}"
        );
    }
}

/// RULE: `InputConfig.JobDurationInSeconds` is accepted anywhere inside its documented
/// range — at least 1 second (the modelled minimum) and at most 18,000 seconds (the
/// member's doc: "The maximum value is 18,000 seconds.") — and refused outside it.
#[tokio::test]
async fn job_duration_is_accepted_inside_its_documented_range_and_refused_outside_it() {
    // Both published bounds, a value between them, and one past each bound.
    let rows: [(i64, bool); 6] = [
        (1, true),
        (900, true),
        (17_999, true),
        (18_000, true),
        (18_001, false),
        (1_000_000, false),
    ];

    for (duration, accepted) in rows {
        let emulator = Emulator::new();
        seed_model(&emulator, "us-east-1").await;
        let name = format!("duration-{duration}");
        let (status, body) = emulator
            .call(
                "CreateInferenceRecommendationsJob",
                &create_with(&name, &json!({}), &json!({ "JobDurationInSeconds": duration })),
            )
            .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case duration={duration}: expected the create to be accepted, got {status} \
                 with {body}"
            );
            let (status, described) = describe(&emulator, &name).await;
            assert_eq!(
                status, 200,
                "case duration={duration}: describe, got {status} with {described}"
            );
            assert_eq!(
                described["InputConfig"].get("JobDurationInSeconds"),
                Some(&Value::from(duration)),
                "case duration={duration}: replayed JobDurationInSeconds, got {described}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case duration={duration}: expected the create to be refused, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case duration={duration}: refusal error code, got {body}"
            );
            let (status, missing) = describe(&emulator, &name).await;
            assert_eq!(
                status, 400,
                "case duration={duration}: a refused create persists nothing, got {status} \
                 with {missing}"
            );
        }
    }

    // Below the modelled minimum (`range 1..`), which the boundary validator refuses
    // before the handler runs — with the same 400 `ValidationException`.
    let emulator = Emulator::new();
    seed_model(&emulator, "us-east-1").await;
    let (status, body) = emulator
        .call(
            "CreateInferenceRecommendationsJob",
            &create_with("duration-zero", &json!({}), &json!({ "JobDurationInSeconds": 0 })),
        )
        .await;
    assert_eq!(
        status, 400,
        "case duration=0: the modelled minimum is 1 second, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case duration=0: refusal error code, got {body}"
    );
}

/// RULE: an input configuration names the model under test exactly once — through
/// `ModelPackageVersionArn` or through `ModelName` — so a request carrying both, or
/// neither, is refused.
#[tokio::test]
async fn the_input_config_must_name_exactly_one_model_source() {
    let rows: [(&str, Value, bool); 4] = [
        ("model-name-only", json!({ "ModelName": MODEL }), true),
        (
            "model-package-only",
            json!({ "ModelPackageVersionArn": MODEL_PACKAGE_VERSION_ARN }),
            true,
        ),
        (
            "both-sources",
            json!({
                "ModelName": MODEL,
                "ModelPackageVersionArn": MODEL_PACKAGE_VERSION_ARN,
            }),
            false,
        ),
        ("no-source", json!({}), false),
    ];

    for (case, input_config, accepted) in rows {
        let emulator = Emulator::new();
        seed_model(&emulator, "us-east-1").await;
        let name = format!("source-{case}");
        let request = json!({
            "JobName": name,
            "JobType": "Default",
            "RoleArn": ROLE_ARN,
            "InputConfig": input_config,
        });

        let (status, body) = emulator
            .call("CreateInferenceRecommendationsJob", &request)
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected the create to be accepted, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: expected the create to be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: refusal error code, got {body}"
            );
            assert!(
                message(&body).contains("ModelName")
                    && message(&body).contains("ModelPackageVersionArn"),
                "case {case}: the refusal must name both members, got {body}"
            );
            let (status, missing) = describe(&emulator, &name).await;
            assert_eq!(
                status, 400,
                "case {case}: a refused create persists nothing, got {status} with {missing}"
            );
        }
    }
}

/// RULE: a job benchmarks resources the account already holds, so a create naming a model
/// or an endpoint this service does not hold is refused and persists nothing.
#[tokio::test]
async fn a_job_naming_a_resource_the_account_does_not_hold_is_refused() {
    // Rows differ along which referenced resource is missing and how it is named.
    let rows: [(&str, Value, Value, bool); 5] = [
        ("known-model", json!({ "ModelName": MODEL }), json!({}), true),
        ("unknown-model", json!({ "ModelName": "never-created-model" }), json!({}), false),
        (
            "known-endpoint",
            json!({ "ModelName": MODEL }),
            json!({ "Endpoints": [{ "EndpointName": ENDPOINT }] }),
            true,
        ),
        (
            "unknown-endpoint",
            json!({ "ModelName": MODEL }),
            json!({ "Endpoints": [{ "EndpointName": "never-created-endpoint" }] }),
            false,
        ),
        (
            "unnamed-endpoint",
            json!({ "ModelName": MODEL }),
            json!({ "Endpoints": [{}] }),
            false,
        ),
    ];

    for (case, source, extra_input, accepted) in rows {
        let emulator = Emulator::new();
        seed_model(&emulator, "us-east-1").await;
        seed_endpoint(&emulator).await;
        let name = format!("reference-{case}");
        let mut request = json!({
            "JobName": name,
            "JobType": "Advanced",
            "RoleArn": ROLE_ARN,
            "InputConfig": source,
        });
        for (key, value) in extra_input.as_object().expect("a JSON object") {
            request["InputConfig"][key] = value.clone();
        }

        let (status, body) = emulator
            .call("CreateInferenceRecommendationsJob", &request)
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected the create to be accepted, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected the create to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: refusal error code, got {body}"
        );
        let (status, missing) = describe(&emulator, &name).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create persists nothing, got {status} with {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFound",
            "case {case}: the unstored job must be not found, got {missing}"
        );
    }
}

/// The `JobName`s `ListInferenceRecommendationsJobs` reports for `filter`, in the order
/// the listing answered them.
async fn listed(emulator: &Emulator, filter: &Value) -> Vec<String> {
    let listed = emulator
        .ok("ListInferenceRecommendationsJobs", filter)
        .await;
    listed
        .get("InferenceRecommendationsJobs")
        .and_then(Value::as_array)
        .map(|jobs| {
            jobs.iter()
                .filter_map(|job| job.get("JobName").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: `ListInferenceRecommendationsJobs` reports the jobs this account holds — the
/// members each one's create stored, and the status the clock owes it — selected by the
/// request's filters, ordered by `SortBy`/`SortOrder` and paged by `MaxResults`/`NextToken`.
#[tokio::test]
async fn the_listing_reports_the_jobs_the_account_holds() {
    let emulator = Emulator::new();
    seed_model(&emulator, "us-east-1").await;

    // Three jobs, created in a known order, so a creation-time sort is unambiguous: the
    // names sort the OPPOSITE way, which is what makes the two sort keys distinguishable.
    let created = ["zulu-job", "mike-job", "alpha-job"];
    for name in created {
        emulator
            .ok("CreateInferenceRecommendationsJob", &minimal_create(name))
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Every job reports what its create stored, and the terminal status the clock owes it
    // once the span is past — the same status its own reader reports.
    tokio::time::sleep(PAST_JOB_END_DUE).await;
    let page = emulator
        .ok("ListInferenceRecommendationsJobs", &json!({}))
        .await;
    let summary = page
        .get("InferenceRecommendationsJobs")
        .and_then(Value::as_array)
        .and_then(|jobs| {
            jobs.iter()
                .find(|job| job.get("JobName").and_then(Value::as_str) == Some("mike-job"))
                .cloned()
        })
        .unwrap_or(Value::Null);
    assert_eq!(
        summary.get("JobArn").and_then(Value::as_str),
        Some(
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:inference-recommendations-job/mike-job")
                .as_str()
        ),
        "JobArn, got {summary:?}"
    );
    assert_eq!(
        summary.get("JobType").and_then(Value::as_str),
        Some("Default"),
        "JobType, got {summary:?}"
    );
    assert_eq!(
        summary.get("RoleArn").and_then(Value::as_str),
        Some(ROLE_ARN),
        "RoleArn, got {summary:?}"
    );
    assert_eq!(
        summary.get("ModelName").and_then(Value::as_str),
        Some(MODEL),
        "ModelName must be the one the job's InputConfig named, got {summary:?}"
    );
    let (_, described) = describe(&emulator, "mike-job").await;
    assert_eq!(
        summary.get("Status").and_then(Value::as_str),
        described.get("Status").and_then(Value::as_str),
        "the listing and the reader must report one status, got {summary:?} and {described:?}"
    );

    // Rows differ along what the rule generalises over: the sort key, the sort order, each
    // filter the request may carry, and the page size.
    let status = described
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "by name ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-job", "mike-job", "zulu-job"],
        ),
        (
            "by name descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["zulu-job", "mike-job", "alpha-job"],
        ),
        (
            "by creation time ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["zulu-job", "mike-job", "alpha-job"],
        ),
        (
            "name contains",
            json!({ "SortBy": "Name", "SortOrder": "Ascending", "NameContains": "ike" }),
            vec!["mike-job"],
        ),
        (
            "the status every job reached",
            json!({ "SortBy": "Name", "SortOrder": "Ascending", "StatusEquals": status }),
            vec!["alpha-job", "mike-job", "zulu-job"],
        ),
        (
            "a status none of them is in",
            json!({ "StatusEquals": "STOPPED" }),
            vec![],
        ),
        (
            "the model every job named",
            json!({ "SortBy": "Name", "SortOrder": "Ascending", "ModelNameEquals": MODEL }),
            vec!["alpha-job", "mike-job", "zulu-job"],
        ),
        (
            "a model none of them named",
            json!({ "ModelNameEquals": "another-model" }),
            vec![],
        ),
        (
            "a model package none of them named",
            json!({ "ModelPackageVersionArnEquals": MODEL_PACKAGE_VERSION_ARN }),
            vec![],
        ),
        (
            "created before the first of them",
            json!({ "CreationTimeBefore": 1.0 }),
            vec![],
        ),
    ];
    for (case, filter, expected) in cases {
        assert_eq!(
            listed(&emulator, &filter).await,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: the listing's selection and order"
        );
    }

    // `MaxResults` caps the page and `NextToken` resumes immediately after it, so a
    // paginating client walks the whole ordered result exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending", "MaxResults": 2 });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let page = emulator
            .ok("ListInferenceRecommendationsJobs", &body)
            .await;
        let names: Vec<String> = page
            .get("InferenceRecommendationsJobs")
            .and_then(Value::as_array)
            .map(|jobs| {
                jobs.iter()
                    .filter_map(|job| {
                        job.get("JobName").and_then(Value::as_str).map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            names.len() <= 2,
            "a page must never exceed MaxResults, got {names:?}"
        );
        seen.extend(names);
        assert!(
            seen.len() <= created.len(),
            "paging did not terminate, saw {seen:?}"
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![
            "alpha-job".to_string(),
            "mike-job".to_string(),
            "zulu-job".to_string()
        ],
        "paging must walk the whole result exactly once"
    );

    // A request outside the enums or naming no page boundary is refused, never served
    // with a silently re-sorted or restarted listing.
    for (case, body) in [
        ("sort-by outside the enum", json!({ "SortBy": "Nmae" })),
        ("sort-order outside the enum", json!({ "SortOrder": "ascending" })),
        ("status outside the enum", json!({ "StatusEquals": "Completed" })),
        ("a token naming no boundary", json!({ "NextToken": "not-a-token" })),
    ] {
        let (status, refused) = emulator
            .call("ListInferenceRecommendationsJobs", &body)
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} with {refused}"
        );
    }

    // An account that holds no job lists none rather than refusing.
    let empty = Emulator::new();
    assert_eq!(
        listed(&empty, &json!({})).await,
        Vec::<String>::new(),
        "an account with no recommendation job must list none"
    );
}
