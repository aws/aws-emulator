// SPDX-License-Identifier: CC0-1.0

//! How a submit whose union-typed `operationConfig` cannot be read is refused.
//!
//! RULE: a request refused at the boundary is told what to fix in terms of the REQUEST:
//! the arity of a union-typed input member is stated as that member plus the member names
//! the model declares for it, and a constraint broken inside the variant that was set is
//! reported at its full input path, `operationConfig.<variant>.<member>` — with the value
//! half of the clause stated for a member the model discloses and dropped for one it marks
//! `@sensitive`, absent or not.
//!
//! The rows span what the rule generalises over rather than one recorded call: both submit
//! operations (each carrying its own union), each union member a required member is missing
//! from, a missing member at depth inside a variant, a member that is present but outside
//! its declared range, and both of the arity failures (no member set and two).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The Region every call here signs for.
const REGION: &str = "us-west-2";

/// The session every row submits against: refusals are about the BODY, so the session the
/// control plane created is resolved first and resolution never decides a row.
const SESSION: &str = "refusal-rows";

/// One emulator with one created session.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The one call that makes a training session exist, as the SageMaker control
        // plane's `CreateTrainingSession` performs it.
        engine
            .state::<service_common::training_sessions::TrainingSessionDirectory>()
            .lock()
            .register(service_common::training_sessions::TrainingSessionRecord {
                name: SESSION.to_string(),
                arn: engine::emulator_arn(
                    "sagemaker",
                    REGION,
                    &format!("training-session/{SESSION}"),
                ),
                // A session whose compute the control plane has brought up: "The session
                // must be in Running or Blocked state to accept requests."
                lifecycle: service_common::training_sessions::TrainingSessionLifecycle::settled(
                    "Running",
                ),
                // The session's base model decides the context window its sampling
                // operations are served over; this one is of a family this emulator
                // publishes no window for, so no ceiling is in play in the cases below.
                base_model_arn: format!(
                    "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
                     meta-textgeneration-llama-3-8b/1.0.0"
                ),
            });
        Self { router }
    }
}

/// What the emulator answered.
struct Answer {
    status: u16,
    error_type: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_else(|| {
            self.json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        raw.rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Submit `body` to one of the two submit routes, against `session`.
async fn submit_to(emulator: &Emulator, session: &str, route: &str, body: &Value) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=00"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://training-session-runtime.sagemaker.{REGION}.amazonaws.com\
             /v1/training-sessions/{session}/{route}"
        ))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the body serializes"),
        ))
        .expect("the test request builds");
    let response = emulator.router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        error_type: response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body: response.body().to_vec(),
    }
}

/// Submit `body` to one of the two submit routes, against the session that exists.
async fn submit(emulator: &Emulator, route: &str, body: &Value) -> Answer {
    submit_to(emulator, SESSION, route, body).await
}

/// The envelope every clause below is carried in
/// (`aws_smithy_schema_server::constraints::envelope`).
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// A prompt the model accepts, so a row is about the member it leaves out.
fn prompt() -> Value {
    json!({"chunks": [{"tokens": [1, 2], "type": "encoded_text"}]})
}

/// RULE: a union-typed input member carrying NO member at all is refused by naming that
/// member and the member names the model declares for the union, and so is one carrying
/// more than one — that one after the session it names has been resolved, which is the
/// order this surface answers in.
#[tokio::test]
async fn the_arity_of_a_union_member_is_stated_as_the_request_names_it() {
    let emulator = Emulator::new();

    // (case, the route, the body, the member names the model declares for that union)
    // The names and their order are the model's own
    // (smithy_query(query="shape",shape="SampleOperationConfig,OperationConfig")).
    let sampling_members = "sample, computeLogprobs, loadWeightsForSampling";
    let training_members =
        "forward, forwardBackward, optimizerStep, saveWeights, saveState, loadState";
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a sampling config with no member set",
            "sampling-operations",
            json!({"operationConfig": {}}),
            sampling_members,
        ),
        (
            "a sampling config with two members set",
            "sampling-operations",
            json!({"operationConfig": {
                "loadWeightsForSampling": {"modelPackageArn": "arn:aws:sagemaker:us-west-2:000000000000:model-package/p/1"},
                "computeLogprobs": {"prompt": prompt()},
            }}),
            sampling_members,
        ),
        (
            "a sampling config whose only member the union does not declare",
            "sampling-operations",
            json!({"operationConfig": {"notAMember": {}}}),
            sampling_members,
        ),
        (
            "a training config with no member set",
            "operations",
            json!({"sequenceNumber": 1, "operationConfig": {}}),
            training_members,
        ),
        (
            "a training config with two members set",
            "operations",
            json!({"sequenceNumber": 1, "operationConfig": {
                "saveState": {},
                "saveWeights": {},
            }}),
            training_members,
        ),
    ];

    for (case, route, body, members) in rows {
        let answer = submit(&emulator, route, &body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            format!("operationConfig must contain exactly one of: {members}"),
            "case {case:?}: expected the arity rule stated over the request's member, \
             got {:?}",
            answer.message()
        );
    }
}

/// RULE: the two arity failures are refused at different points of the request walk,
/// because one of them cannot be read at all: a body carrying NO member is refused before
/// anything is resolved, while one carrying several is refused only after the session it
/// names has been resolved — so a submit naming a session that does not exist reports the
/// session, not the body.
#[tokio::test]
async fn an_ambiguous_config_is_refused_after_the_session_is_resolved() {
    let emulator = Emulator::new();
    let absent = "never-created";

    // (case, the body, the status, the message)
    let rows: Vec<(&str, Value, u16, String)> = vec![
        (
            "no member set, against a session that does not exist",
            json!({"operationConfig": {}}),
            400,
            "operationConfig must contain exactly one of: sample, computeLogprobs, \
             loadWeightsForSampling"
                .to_string(),
        ),
        (
            "two members set, against a session that does not exist",
            json!({"operationConfig": {
                "sample": {
                    "prompt": prompt(),
                    "numSamples": 1,
                    "samplingParams": {"maxTokens": 512},
                },
                "computeLogprobs": {"prompt": prompt()},
            }}),
            404,
            format!(
                "Session not found: {absent}-{}-{REGION}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            // Every member the body set is a caller's input, so each is judged against
            // its own constraints: a broken one is reported even where another member is
            // well formed, and before anything is resolved.
            "two members set, one of them broken",
            json!({"operationConfig": {
                "sample": {
                    "prompt": prompt(),
                    "numSamples": 0,
                    "samplingParams": {"maxTokens": 512},
                },
                "computeLogprobs": {"prompt": prompt()},
            }}),
            400,
            one_error(
                "Value '0' at 'operationConfig.sample.numSamples' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
        ),
    ];

    for (case, body, status, message) in rows {
        let answer = submit_to(&emulator, absent, "sampling-operations", &body).await;
        assert_eq!(
            (answer.status, answer.message()),
            (status, message.clone()),
            "case {case:?}: expected HTTP {status} {message:?}, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // Against the session that DOES exist, the same ambiguous body is refused on its
    // arity: resolution succeeded, so the body is what is left to report.
    let ambiguous = submit(
        &emulator,
        "sampling-operations",
        &json!({"operationConfig": {
            "sample": {
                "prompt": prompt(),
                "numSamples": 1,
                "samplingParams": {"maxTokens": 512},
            },
            "computeLogprobs": {"prompt": prompt()},
        }}),
    )
    .await;
    assert_eq!(
        (ambiguous.status, ambiguous.message()),
        (
            400,
            "operationConfig must contain exactly one of: sample, computeLogprobs, \
             loadWeightsForSampling"
                .to_string()
        ),
        "case an ambiguous body against a session that exists: expected the arity rule, \
         got {} {}",
        ambiguous.status,
        ambiguous.text()
    );
}

/// RULE: a constraint broken inside the variant that was set is reported at
/// `operationConfig.<variant>.<member>`, with the value half of the clause stated for a
/// member the model discloses and dropped for one it marks `@sensitive`.
#[tokio::test]
async fn a_violation_inside_a_variant_is_reported_under_that_variant() {
    let emulator = Emulator::new();
    let arn = "arn:aws:sagemaker:us-west-2:000000000000:model-package/p/1";

    // (case, the route, the body, the whole message)
    //
    // `SampleConfig$prompt` and `ComputeLogprobsConfig$prompt` are `@required` and
    // `@sensitive`, `SampleConfig$numSamples` and `$samplingParams` are `@required`,
    // `LoadWeightsForSamplingConfig$modelPackageArn` is `@required`, and `ModelInput`
    // is itself `@sensitive`
    // (smithy_query(query="shape",shape="SampleConfig,ModelInput")) — so the sensitive
    // rows state no value and the others state the `null` of a member that was absent.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a sample config with no prompt",
            "sampling-operations",
            json!({"operationConfig": {"sample": {
                "numSamples": 1,
                "samplingParams": {"maxTokens": 512},
            }}}),
            one_error(
                "Value at 'operationConfig.sample.prompt' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "a computeLogprobs config with no prompt",
            "sampling-operations",
            json!({"operationConfig": {"computeLogprobs": {}}}),
            one_error(
                "Value at 'operationConfig.computeLogprobs.prompt' failed to satisfy \
                 constraint: Member must not be null",
            ),
        ),
        (
            "a prompt carrying no chunks member at all",
            "sampling-operations",
            json!({"operationConfig": {"sample": {
                "prompt": {},
                "numSamples": 1,
                "samplingParams": {"maxTokens": 512},
            }}}),
            one_error(
                "Value at 'operationConfig.sample.prompt.chunks' failed to satisfy \
                 constraint: Member must not be null",
            ),
        ),
        (
            "a sample config with no numSamples",
            "sampling-operations",
            json!({"operationConfig": {"sample": {
                "prompt": prompt(),
                "samplingParams": {"maxTokens": 512},
            }}}),
            one_error(
                "Value null at 'operationConfig.sample.numSamples' failed to satisfy \
                 constraint: Member must not be null",
            ),
        ),
        (
            "a sample config with no samplingParams",
            "sampling-operations",
            json!({"operationConfig": {"sample": {
                "prompt": prompt(),
                "numSamples": 1,
            }}}),
            one_error(
                "Value null at 'operationConfig.sample.samplingParams' failed to satisfy \
                 constraint: Member must not be null",
            ),
        ),
        (
            "a loadWeightsForSampling config with no modelPackageArn",
            "sampling-operations",
            json!({"operationConfig": {"loadWeightsForSampling": {}}}),
            one_error(
                "Value null at 'operationConfig.loadWeightsForSampling.modelPackageArn' \
                 failed to satisfy constraint: Member must not be null",
            ),
        ),
        (
            "a numSamples below its declared minimum",
            "sampling-operations",
            json!({"operationConfig": {"sample": {
                "prompt": prompt(),
                "numSamples": 0,
                "samplingParams": {"maxTokens": 512},
            }}}),
            one_error(
                "Value '0' at 'operationConfig.sample.numSamples' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
        ),
        (
            "a maxTokens below its declared minimum, two variants deep",
            "sampling-operations",
            json!({"operationConfig": {"sample": {
                "prompt": prompt(),
                "numSamples": 1,
                "samplingParams": {"maxTokens": 511},
            }}}),
            one_error(
                "Value '511' at 'operationConfig.sample.samplingParams.maxTokens' failed \
                 to satisfy constraint: Member must have value greater than or equal to 512",
            ),
        ),
        (
            "a loadState config with no modelPackageArn",
            "operations",
            json!({"sequenceNumber": 1, "operationConfig": {"loadState": {}}}),
            one_error(
                "Value null at 'operationConfig.loadState.modelPackageArn' failed to \
                 satisfy constraint: Member must not be null",
            ),
        ),
        (
            "a forwardBackward datum with no modelInput",
            "operations",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "ppo",
                "datumList": [{}],
            }}}),
            one_error(
                "Value at 'operationConfig.forwardBackward.datumList.1.member.modelInput' \
                 failed to satisfy constraint: Member must not be null",
            ),
        ),
        (
            "a loadState ARN outside the modelled syntax",
            "operations",
            json!({"sequenceNumber": 1, "operationConfig": {"loadState": {
                "modelPackageArn": arn,
                "loadOptimizer": false,
            }}}),
            String::new(),
        ),
    ];

    for (case, route, body, expected) in rows {
        let answer = submit(&emulator, route, &body).await;
        if expected.is_empty() {
            // The last row is the control: a config whose members are all there and all
            // inside their constraints is NOT refused at the boundary, so the rows above
            // are the constraint and not a blanket refusal of the variant.
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected a well-formed config to be accepted, got {} {}",
                answer.status,
                answer.text()
            );
            continue;
        }
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}
