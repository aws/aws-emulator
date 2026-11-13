// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's batch-transform surface:
//! `CreateTransformJob` and the `DescribeTransformJob` that reads a job back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! optional member and the defaults the model's docs state; the clock-owed terminal
//! status; the refusals for a duplicate name, an unknown model and an unknown job; and
//! the documented payload-sizing bounds on each published bound and one past it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::TRANSFORM_JOB_SETTLES_AFTER`, so the terminal status the
/// clock owes is due by the time the next call runs.
const PAST_SETTLE_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::TRANSFORM_JOB_SETTLES_AFTER
        .saturating_mul(3);

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
///
/// Mirrors the harness `tests/all/inference_deployment_lifecycle.rs` builds: each suite
/// is its own module, so each owns the few lines that mount the service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create a model named `model` so a transform job can name it.
    async fn with_model(&self, model: &str) -> &Self {
        self.ok(
            "CreateModel",
            &json!({ "ModelName": model, "PrimaryContainer": { "Image": "demo:1" } }),
        )
        .await;
        self
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

fn num_field(body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a numeric member {member} in {body}"))
}

/// The members every `CreateTransformJob` request must carry, for `model`.
fn transform_body(name: &str, model: &str) -> Value {
    json!({
        "TransformJobName": name,
        "ModelName": model,
        "TransformInput": {
            "DataSource": {
                "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": "s3://demo-bucket/input/" },
            },
        },
        "TransformOutput": { "S3OutputPath": "s3://demo-bucket/output/" },
        "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
    })
}

/// `body` with every member of `extra` merged in.
fn merged(mut body: Value, extra: &Value) -> Value {
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        body[key] = value.clone();
    }
    body
}

/// RULE: every transform-job ARN this service mints is
/// `arn:aws:sagemaker:<signed region>:<account>:transform-job/<name>`, whatever the
/// name's shape and whatever region the caller signed for, and `DescribeTransformJob`
/// reports the very same ARN.
#[tokio::test]
async fn minted_transform_job_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: region and the name's shape
    // (short, digits, hyphens, and the 63-character modelled maximum of
    // `TransformJobName`, `len 1..63` per smithy_query operation=CreateTransformJob).
    let names = [
        "t",
        "job1",
        "demo-transform-job-with-hyphens",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            emulator.with_model("m").await;

            let (status, created) = emulator
                .call_in(region, "CreateTransformJob", &transform_body(name, "m"))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: CreateTransformJob, got {status} with {created}"
            );
            let expected = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:transform-job/{name}");
            assert_eq!(
                str_field(&created, "TransformJobArn"),
                expected,
                "case {case}: TransformJobArn, got {created:?}"
            );

            let (status, described) = emulator
                .call_in(
                    region,
                    "DescribeTransformJob",
                    &json!({ "TransformJobName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeTransformJob, got {status} with {described}"
            );
            assert_eq!(
                str_field(&described, "TransformJobArn"),
                expected,
                "case {case}: DescribeTransformJob TransformJobArn, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "TransformJobName"),
                name,
                "case {case}: TransformJobName, got {described:?}"
            );
        }
    }
}

/// RULE: `DescribeTransformJob` replays every member `CreateTransformJob` stored —
/// present members echoed, absent ones absent — except the members whose model docs
/// state a default, which are reported with that default resolved.
#[tokio::test]
async fn describe_transform_job_replays_what_create_stored_with_documented_defaults() {
    // Rows differ along the axes the rule generalises over: which optional members are
    // present, the data-source kind, the compression and split forms, the batch
    // strategy, the instance count and type, and the payload sizing.
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({})),
        (
            "csv-line-split",
            json!({
                "TransformInput": {
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://demo-bucket/input/dataset01/",
                        },
                    },
                    "ContentType": "text/csv",
                    "SplitType": "Line",
                    "CompressionType": "None",
                },
                "TransformOutput": {
                    "S3OutputPath": "s3://demo-bucket/output/",
                    "Accept": "text/csv",
                    "AssembleWith": "Line",
                },
                "BatchStrategy": "MultiRecord",
            }),
        ),
        (
            "manifest-gzip-recordio",
            json!({
                "TransformInput": {
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "ManifestFile",
                            "S3Uri": "s3://demo-bucket/example.manifest",
                        },
                    },
                    "CompressionType": "Gzip",
                    "SplitType": "RecordIO",
                },
                "BatchStrategy": "SingleRecord",
                "MaxPayloadInMB": 1,
                "MaxConcurrentTransforms": 4,
            }),
        ),
        (
            "environment-processing-and-experiment",
            json!({
                "Environment": { "LOG_LEVEL": "debug", "THREADS": "2" },
                "DataProcessing": {
                    "InputFilter": "$.features",
                    "OutputFilter": "$['id','prediction']",
                    "JoinSource": "Input",
                },
                "ExperimentConfig": {
                    "ExperimentName": "demo-experiment",
                    "TrialName": "demo-trial",
                    "TrialComponentDisplayName": "demo-component",
                },
                "ModelClientConfig": {
                    "InvocationsTimeoutInSeconds": 120,
                    "InvocationsMaxRetries": 2,
                },
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
        (
            "data-capture-kms-and-three-instances",
            json!({
                "TransformInput": {
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://demo-bucket/input/",
                        },
                    },
                    "SplitType": "TFRecord",
                },
                "TransformOutput": {
                    "S3OutputPath": "s3://demo-bucket/output/",
                    "KmsKeyId": "alias/ExampleAlias",
                    "AssembleWith": "None",
                },
                "TransformResources": {
                    "InstanceType": "ml.m4.xlarge",
                    "InstanceCount": 3,
                    "VolumeKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                },
                "DataCaptureConfig": {
                    "DestinationS3Uri": "s3://demo-bucket/capture/",
                    "GenerateInferenceId": true,
                },
                "MaxPayloadInMB": 0,
                "MaxConcurrentTransforms": 0,
            }),
        ),
    ];

    // Members the service stores and reports verbatim: present in the request means
    // present and equal in the answer, absent means absent.
    let echoed = [
        "TransformOutput",
        "TransformResources",
        "DataCaptureConfig",
        "DataProcessing",
        "ExperimentConfig",
        "ModelClientConfig",
        "Environment",
        "BatchStrategy",
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        emulator.with_model("demo-model").await;
        let body = merged(transform_body(case, "demo-model"), &extra);

        emulator.ok("CreateTransformJob", &body).await;
        let described = emulator
            .ok("DescribeTransformJob", &json!({ "TransformJobName": case }))
            .await;

        assert_eq!(
            str_field(&described, "ModelName"),
            "demo-model",
            "case {case}: ModelName, got {described:?}"
        );
        for member in echoed {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    described.get(member),
                    Some(sent),
                    "case {case}: {member} must be replayed as sent, got {:?}",
                    described.get(member)
                ),
                None => assert!(
                    described.get(member).is_none_or(Value::is_null),
                    "case {case}: {member} was not sent, so it must be absent, got {:?}",
                    described.get(member)
                ),
            }
        }

        // `TransformInput` is replayed with the two defaults its members' docs state:
        // "The default value is None" (`TransformInput$CompressionType`) and "The
        // default value for SplitType is None" (`TransformInput$SplitType`).
        let mut expected_input = body["TransformInput"].clone();
        for member in ["CompressionType", "SplitType"] {
            if expected_input.get(member).is_none() {
                expected_input[member] = json!("None");
            }
        }
        assert_eq!(
            described.get("TransformInput"),
            Some(&expected_input),
            "case {case}: TransformInput, got {:?}",
            described.get("TransformInput")
        );

        // "The default value is 6 MB" (`CreateTransformJob$MaxPayloadInMB`), and
        // "If MaxConcurrentTransforms is set to 0 or left unset ... the default value
        // is 1" (`CreateTransformJob$MaxConcurrentTransforms`).
        let expected_payload = body
            .get("MaxPayloadInMB")
            .and_then(Value::as_i64)
            .unwrap_or(6);
        let expected_concurrency = match body.get("MaxConcurrentTransforms").and_then(Value::as_i64)
        {
            Some(sent) if sent > 0 => sent,
            _ => 1,
        };
        assert_eq!(
            num_field(&described, "MaxPayloadInMB"),
            expected_payload,
            "case {case}: MaxPayloadInMB, got {described:?}"
        );
        assert_eq!(
            num_field(&described, "MaxConcurrentTransforms"),
            expected_concurrency,
            "case {case}: MaxConcurrentTransforms, got {described:?}"
        );

        // A freshly created job is still running, so neither the end time nor the
        // failure reason exists yet, and no ML instance was ever started.
        assert_eq!(
            str_field(&described, "TransformJobStatus"),
            "InProgress",
            "case {case}: TransformJobStatus, got {described:?}"
        );
        for absent in ["FailureReason", "TransformEndTime", "TransformStartTime"] {
            assert!(
                described.get(absent).is_none_or(Value::is_null),
                "case {case}: {absent} must be absent on a running job, got {:?}",
                described.get(absent)
            );
        }
        assert!(
            described.get("CreationTime").is_some(),
            "case {case}: CreationTime must be reported, got {described:?}"
        );
    }
}

/// RULE: a transform job is created `InProgress` and the clock owes it a terminal status
/// a span later; because this emulator runs no inference container, that terminal status
/// is the modelled `Failed` with a `FailureReason` saying so and a `TransformEndTime`
/// that is stable across later reads. Holds for every job, whatever its shape.
#[tokio::test]
async fn a_transform_job_reaches_the_terminal_status_the_clock_owes_it() {
    // Rows differ along the shape of the job: the data-source kind, the split form and
    // the instance count — none of which changes the status the clock owes.
    let cases: Vec<(&str, Value)> = vec![
        ("prefix-single-instance", json!({})),
        (
            "manifest-four-concurrent",
            json!({ "MaxConcurrentTransforms": 4, "MaxPayloadInMB": 10 }),
        ),
        (
            "line-split-three-instances",
            json!({
                "TransformResources": { "InstanceType": "ml.m4.xlarge", "InstanceCount": 3 },
                "TransformInput": {
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://demo-bucket/input/",
                        },
                    },
                    "SplitType": "Line",
                },
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        emulator.with_model("demo-model").await;
        let body = merged(transform_body(case, "demo-model"), &extra);
        emulator.ok("CreateTransformJob", &body).await;

        let running = emulator
            .ok("DescribeTransformJob", &json!({ "TransformJobName": case }))
            .await;
        assert_eq!(
            str_field(&running, "TransformJobStatus"),
            "InProgress",
            "case {case}: a job describes as running before its end is due, got {running:?}"
        );

        tokio::time::sleep(PAST_SETTLE_DUE).await;

        let settled = emulator
            .ok("DescribeTransformJob", &json!({ "TransformJobName": case }))
            .await;
        assert_eq!(
            str_field(&settled, "TransformJobStatus"),
            "Failed",
            "case {case}: the terminal status the clock owes, got {settled:?}"
        );
        assert_eq!(
            str_field(&settled, "FailureReason"),
            service_com_amazonaws_sagemaker_sagemaker::state::TRANSFORM_NOT_RUN_FAILURE,
            "case {case}: FailureReason must say why nothing was transformed, got {settled:?}"
        );
        let reason = str_field(&settled, "FailureReason");
        assert!(
            reason.contains("no records were transformed"),
            "case {case}: FailureReason must say no record was transformed, got {reason:?}"
        );
        let end_time = settled
            .get("TransformEndTime")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            !end_time.is_null(),
            "case {case}: a terminal job reports a TransformEndTime, got {settled:?}"
        );

        // The outcome was owed at a stored instant, so reading it again reports the
        // same body: nothing about the read moves the clock on.
        let reread = emulator
            .ok("DescribeTransformJob", &json!({ "TransformJobName": case }))
            .await;
        assert_eq!(
            reread, settled,
            "case {case}: a second describe must report the same job"
        );
    }
}

/// RULE: a `TransformJobName` already held in this account and region is refused with
/// `ResourceInUse` and the stored job is left as it was — whatever the second request's
/// members look like.
#[tokio::test]
async fn a_duplicate_transform_job_name_is_refused_and_the_stored_job_survives() {
    // Rows differ in how far the second request's body differs from the first's.
    let cases: Vec<(&str, Value)> = vec![
        ("identical-body", json!({})),
        (
            "different-output-path",
            json!({ "TransformOutput": { "S3OutputPath": "s3://other-bucket/elsewhere/" } }),
        ),
        (
            "different-model",
            json!({ "ModelName": "second-model", "MaxPayloadInMB": 20 }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        emulator.with_model("demo-model").await;
        emulator.with_model("second-model").await;
        let first = transform_body("dup-job", "demo-model");
        emulator.ok("CreateTransformJob", &first).await;

        let (status, refused) = emulator
            .call("CreateTransformJob", &merged(first.clone(), &extra))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case {case}: error code, got {refused:?}"
        );

        let described = emulator
            .ok(
                "DescribeTransformJob",
                &json!({ "TransformJobName": "dup-job" }),
            )
            .await;
        assert_eq!(
            str_field(&described, "ModelName"),
            "demo-model",
            "case {case}: the stored job must be untouched, got {described:?}"
        );
        assert_eq!(
            described.get("TransformOutput"),
            first.get("TransformOutput"),
            "case {case}: the stored job's TransformOutput must be untouched, got {described:?}"
        );
    }
}

/// RULE: a `ModelName` this account and region holds no model for is refused with a 400
/// `ValidationException` naming the model the request asked for — a dangling reference to
/// another resource is a property of the REQUEST — and nothing is persisted, so the job
/// cannot be described.
#[tokio::test]
async fn a_transform_job_naming_an_unknown_model_is_refused_and_persists_nothing() {
    // Rows differ along the name shape and along what else the account holds: a store
    // with a model of another name, and an empty store.
    let cases = [
        ("absent-with-other-model-present", Some("other-model"), "ghost"),
        ("absent-from-empty-store", None, "ghost"),
        ("case-differs-from-stored", Some("demo-model"), "Demo-Model"),
        ("prefix-of-stored", Some("demo-model-v2"), "demo-model"),
    ];

    for (case, stored, requested) in cases {
        let emulator = Emulator::new();
        if let Some(name) = stored {
            emulator.with_model(name).await;
        }

        let (status, refused) = emulator
            .call(
                "CreateTransformJob",
                &transform_body("orphan-job", requested),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: an unknown model is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: error code, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Could not find model \"{requested}\"."),
            "case {case}: the refusal must name the model the request asked for, got \
             {refused:?}"
        );

        let (status, missing) = emulator
            .call(
                "DescribeTransformJob",
                &json!({ "TransformJobName": "orphan-job" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the refused job must not be stored, got {status} with {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFound",
            "case {case}: describe error code, got {missing:?}"
        );
    }
}

/// RULE: `DescribeTransformJob` refuses a name no job in this account and region answers
/// to with `ResourceNotFound`, whatever else the store holds.
#[tokio::test]
async fn describing_an_unknown_transform_job_is_refused() {
    let emulator = Emulator::new();
    emulator.with_model("demo-model").await;
    emulator
        .ok("CreateTransformJob", &transform_body("stored-job", "demo-model"))
        .await;

    // Rows differ along how close the asked-for name is to the stored one.
    for name in ["other-job", "stored", "stored-job-2", "Stored-Job"] {
        let (status, refused) = emulator
            .call("DescribeTransformJob", &json!({ "TransformJobName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case name={name}: an unknown job is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case name={name}: error code, got {refused:?}"
        );
    }
}

/// RULE: the payload sizing SageMaker documents is enforced — `MaxPayloadInMB` may not
/// exceed 100 MB, and when `MaxConcurrentTransforms` is specified, their product may not
/// either; a request outside a bound is refused with `ValidationException` and persists
/// nothing, and one on a bound is accepted.
#[tokio::test]
async fn documented_payload_sizing_bounds_are_enforced_on_each_bound_and_one_past_it() {
    // The bounds come from `CreateTransformJob$MaxPayloadInMB`'s model doc: "The value of
    // MaxPayloadInMB cannot be greater than 100 MB. If you specify the
    // MaxConcurrentTransforms parameter, the value of (MaxConcurrentTransforms *
    // MaxPayloadInMB) also cannot exceed 100 MB." Rows sit on each published bound, one
    // past it, and inside it.
    let cases: Vec<(&str, Option<i64>, Option<i64>, bool)> = vec![
        ("payload-unset", None, None, true),
        ("payload-on-ceiling", Some(100), None, true),
        ("payload-past-ceiling", Some(101), None, false),
        ("payload-far-past-ceiling", Some(1024), None, false),
        ("payload-inside-ceiling", Some(6), None, true),
        ("product-on-ceiling", Some(10), Some(10), true),
        ("product-past-ceiling", Some(10), Some(11), false),
        ("product-far-past-ceiling", Some(50), Some(8), false),
        ("product-inside-ceiling", Some(6), Some(4), true),
        // Concurrency specified with no payload: the product is bounded against the
        // documented 6 MB default the service resolves, so 16 fits and 17 does not.
        ("default-payload-product-on-ceiling", None, Some(16), true),
        ("default-payload-product-past-ceiling", None, Some(17), false),
        ("default-payload-product-far-past-ceiling", None, Some(100), false),
        // `MaxConcurrentTransforms` of 0 means "unset" per its doc, so there is no
        // product to bound even next to the payload ceiling.
        ("concurrency-zero-on-ceiling", Some(100), Some(0), true),
        ("payload-zero", Some(0), Some(100), true),
    ];

    for (case, payload, concurrency, accepted) in cases {
        let emulator = Emulator::new();
        emulator.with_model("demo-model").await;
        let mut body = transform_body(case, "demo-model");
        if let Some(payload) = payload {
            body["MaxPayloadInMB"] = json!(payload);
        }
        if let Some(concurrency) = concurrency {
            body["MaxConcurrentTransforms"] = json!(concurrency);
        }

        let (status, answer) = emulator.call("CreateTransformJob", &body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: within both documented bounds, got {status} with {answer}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: outside a documented bound, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: error code, got {answer:?}"
        );
        let (status, missing) = emulator
            .call("DescribeTransformJob", &json!({ "TransformJobName": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused request persists nothing, got {status} with {missing}"
        );
    }
}
