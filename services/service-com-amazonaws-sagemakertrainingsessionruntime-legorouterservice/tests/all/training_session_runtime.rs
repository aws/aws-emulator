// SPDX-License-Identifier: CC0-1.0

//! The training-session runtime's submit-then-poll loop, end to end over the wire.
//!
//! Every case here goes through `MultiServiceRouter::dispatch`, so each call is a
//! separate restJson1 request against one shared engine: a value that survives from one
//! call to the next survived because it was PERSISTED, not because a handler echoed its
//! own input back.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs that
//! spans what the rule generalises over (the union members, the published numeric
//! bounds, the sequence-number orderings, the payload sizes):
//!
//! 1. A submit mints and persists an operation id; a poll answers that operation's
//!    status and result from stored state, and the populated `OperationResult` member is
//!    the one whose `OperationConfig` member was submitted.
//! 2. An operation whose whole result is a number only a model's forward pass has — no
//!    language model runs in this emulator — ends in the modelled `Failed` status with
//!    `errorDetails` that say so, and never in `Completed`; an operation this emulator
//!    does serve completes and reports its own result member (see
//!    `sampling_completion.rs` for what a served `sample` carries).
//! 3. A checkpoint operation (`saveWeights`, `saveState`, `loadState`,
//!    `loadWeightsForSampling`) is executed for real: a save publishes a model package
//!    ARN built from the request's region and an incrementing version, and a load
//!    succeeds only for a checkpoint the session holds.
//! 4. A status transition is owed to the CLOCK: an operation is non-terminal straight
//!    after the submit and terminal once its stored due instant has passed.
//! 5. `clientToken` is idempotent per session: a retry answers the id the first call
//!    minted and creates no second operation.
//! 6. Operations execute strictly in sequence-number order: a taken or already-executed
//!    slot is `ConflictException`, and a gap blocks the session rather than failing.
//! 7. A session or operation this emulator has never seen is `ResourceNotFoundException`,
//!    including a training id presented on the sampling route and the other way round.
//! 8. A request that breaks a modelled constraint or a rule the model states in prose is
//!    `ValidationException`.
//! 9. `SampleTrainingSession` resolves the session's checkpoint from stored state and
//!    answers a body that is not the request's own payload.
//! 10. `SampleTrainingSessionWithResponseStream` resolves and refuses exactly as the
//!     synchronous call does, and frames what it serves as Server-Sent Events terminated
//!     by `data: [DONE]`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the tests' SigV4 credential scope names, and therefore the region the
/// checkpoint ARNs they read back must carry.
const REGION: &str = "us-west-2";

/// How many polls a terminal status is waited for before the case fails. Far above the
/// service's own execution span, so a loaded host still passes.
const POLL_BUDGET: u32 = 40;

/// One emulator: a shared engine with this service mounted on the production router, so
/// the tests drive the same dispatch path the binary does.
///
/// The engine is held beside the router because a training session EXISTS only because
/// the SageMaker control plane created it: this data plane models no create and resolves
/// the name a request carries through the directory both services read, so each case
/// registers the sessions it is about ([`Emulator::create_session`]) and a name it does
/// not register is a session that was never created.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// Register `name` the way the SageMaker control plane's `CreateTrainingSession`
    /// does: the one call that makes a training session exist, which this crate's own
    /// model declares nothing for.
    fn create_session(&self, name: &str) {
        self.engine
            .state::<service_common::training_sessions::TrainingSessionDirectory>()
            .lock()
            .register(service_common::training_sessions::TrainingSessionRecord {
                name: name.to_string(),
                arn: engine::emulator_arn(
                    "sagemaker",
                    REGION,
                    &format!("training-session/{name}"),
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
    }
}

impl std::ops::Deref for Emulator {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &Self::Target {
        &self.router
    }
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests assert behaviour, not signatures; the emulator verifies signatures
    // itself and a service never checks them.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered.
struct Answer {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code a client branches on: the `x-amzn-errortype` header the
    /// restJson1 envelope carries, falling back to the body's `__type`.
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

    fn text(&self, member: &str) -> String {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Send one request whose SigV4 credential scope names `region`, so the router's
/// `can_handle` claims it for this service's `sagemaker` signing name.
async fn call_in_region(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: bytes::Bytes,
    region: &str,
    headers: &[(&str, String)],
) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://training-session-runtime.sagemaker.{region}.amazonaws.com{path}"
        ))
        .header("authorization", authorization)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, value.as_str());
    }
    let response = router
        .dispatch(request.body(body).expect("test request builds"))
        .await;
    Answer {
        status: response.status().as_u16(),
        error_type: response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body: response.body().to_vec(),
    }
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let bytes = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("test body serializes")),
        None => bytes::Bytes::new(),
    };
    call_in_region(router, method, path, bytes, REGION, &[]).await
}

async fn submit_training(router: &MultiServiceRouter, session: &str, body: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/training-sessions/{session}/operations"),
        Some(body),
    )
    .await
}

async fn get_training(router: &MultiServiceRouter, session: &str, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/training-sessions/{session}/operations/{id}"),
        None,
    )
    .await
}

async fn submit_sampling(router: &MultiServiceRouter, session: &str, body: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/training-sessions/{session}/sampling-operations"),
        Some(body),
    )
    .await
}

async fn get_sampling(router: &MultiServiceRouter, session: &str, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/training-sessions/{session}/sampling-operations/{id}"),
        None,
    )
    .await
}

/// Submit `config` at `sequence` and require the submit to have succeeded, answering the
/// minted operation id.
async fn submit_training_ok(
    router: &MultiServiceRouter,
    session: &str,
    sequence: i64,
    config: Value,
) -> String {
    let answer = submit_training(
        router,
        session,
        json!({"sequenceNumber": sequence, "operationConfig": config}),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case seq {sequence} {config}: expected HTTP 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let id = answer.text("operationId");
    assert!(
        is_operation_id(&id, "training"),
        "case seq {sequence} {config}: expected a training operation id, got {id:?}"
    );
    id
}

/// Poll a training operation until its status is terminal, answering the last answer.
async fn poll_training_to_terminal(router: &MultiServiceRouter, session: &str, id: &str) -> Answer {
    poll_training_to_terminal_in(router, session, id, REGION).await
}

/// [`poll_training_to_terminal`], reading in `region`, so a case about what the request's
/// own Region produced reads it back under that Region.
///
/// Waits for the CONDITION rather than for a span of wall clock: the operation's
/// transition is owed at an instant the service stored, so the loop looks again until it
/// has happened or a generous budget runs out.
async fn poll_training_to_terminal_in(
    router: &MultiServiceRouter,
    session: &str,
    id: &str,
    region: &str,
) -> Answer {
    for _ in 0..POLL_BUDGET {
        let answer = call_in_region(
            router,
            "GET",
            &format!("/v1/training-sessions/{session}/operations/{id}"),
            bytes::Bytes::new(),
            region,
            &[],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {id}: a poll of a stored operation must answer HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if is_terminal(&answer.text("status")) {
            return answer;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("case {id}: the operation did not reach a terminal status within the poll budget");
}

/// The sampling counterpart of [`poll_training_to_terminal`].
async fn poll_sampling_to_terminal(router: &MultiServiceRouter, session: &str, id: &str) -> Answer {
    for _ in 0..POLL_BUDGET {
        let answer = get_sampling(router, session, id).await;
        assert_eq!(
            answer.status, 200,
            "case {id}: a poll of a stored operation must answer HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if is_terminal(&answer.text("status")) {
            return answer;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("case {id}: the operation did not reach a terminal status within the poll budget");
}

/// The terminal `RequestStatus` values, from the model's enum: "Poll this operation until
/// the status reaches a terminal state (Completed, Failed, or Skipped)."
fn is_terminal(status: &str) -> bool {
    matches!(status, "Completed" | "Failed" | "Skipped")
}

/// Whether `id` is an operation id of `namespace`: within `OperationId`'s 1..64 length
/// and `^[a-zA-Z0-9-]+$` pattern, and carrying that namespace's prefix.
fn is_operation_id(id: &str, namespace: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.starts_with(&format!("{namespace}-"))
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A `ModelInput` of one chunk carrying `tokens`.
fn model_input(tokens: &[i64]) -> Value {
    json!({"chunks": [{"tokens": tokens, "type": "encoded_text"}]})
}

/// A `forwardBackward` configuration over one datum.
fn forward_backward(loss_fn_name: &str) -> Value {
    json!({"forwardBackward": {
        "lossFnName": loss_fn_name,
        "datumList": [{"modelInput": model_input(&[1, 2, 3])}],
    }})
}

/// A `sample` configuration asking for `num_samples` samples.
fn sample_config(num_samples: i64) -> Value {
    json!({"sample": {
        "prompt": model_input(&[7, 8]),
        "numSamples": num_samples,
        "samplingParams": {"maxTokens": 512, "temperature": 0.7, "topK": 50, "topP": 0.9},
    }})
}

/// Write a checkpoint on `session` and answer its model package ARN.
///
/// `holds_optimizer_state` picks the operation that writes it: `saveState` saves "full
/// training state, including model weights and optimizer state", `saveWeights` only the
/// weights.
async fn write_checkpoint(
    router: &MultiServiceRouter,
    session: &str,
    sequence: i64,
    holds_optimizer_state: bool,
) -> String {
    let config = if holds_optimizer_state {
        json!({"saveState": {"description": "a resumable checkpoint"}})
    } else {
        json!({"saveWeights": {"mergeWeights": true, "description": "merged weights"}})
    };
    let id = submit_training_ok(router, session, sequence, config).await;
    let answer = poll_training_to_terminal(router, session, &id).await;
    assert_eq!(
        answer.text("status"),
        "Completed",
        "case seq {sequence}: a checkpoint operation must complete, got {}",
        answer.json()
    );
    let member = if holds_optimizer_state {
        "saveState"
    } else {
        "saveWeights"
    };
    let arn = answer.json()["operationResult"][member]["modelPackageArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "case seq {sequence}: {member} must report a model package ARN, got {}",
        answer.json()
    );
    arn
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 3: what each union member does
// ---------------------------------------------------------------------------

/// Every `OperationConfig` member, submitted and polled to its terminal status. The
/// result member a poll populates is the one that was submitted; a member whose
/// execution needs a model runtime ends `Failed` with a reason, and one that only moves
/// a checkpoint around ends `Completed`.
#[tokio::test]
async fn every_training_operation_member_answers_from_stored_state() {
    let router = emulator();
    let session = "member-table";
    router.create_session(session);
    // Written first so the `loadState` rows have a checkpoint to name. `saveState`
    // carries optimizer state, `saveWeights` does not.
    let state_arn = write_checkpoint(&router, session, 1, true).await;
    let weights_arn = write_checkpoint(&router, session, 2, false).await;

    // (case, config, expected status, the OperationResult member that must be populated)
    let cases: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "forward",
            json!({"forward": {
                "lossFnName": "cross_entropy",
                "datumList": [{"modelInput": model_input(&[1, 2])}],
            }}),
            "Failed",
            None,
        ),
        (
            "forwardBackward",
            forward_backward("importance_sampling"),
            "Failed",
            None,
        ),
        (
            "optimizerStep",
            json!({"optimizerStep": {"adamParamsConfig": {
                "learningRate": 0.0001, "beta1": 0.9, "beta2": 0.999,
                "eps": 0.00000001, "weightDecay": 0.01, "gradClipNorm": 1.0,
            }}}),
            "Failed",
            None,
        ),
        (
            "saveWeights",
            json!({"saveWeights": {"mergeWeights": false}}),
            "Completed",
            Some("saveWeights"),
        ),
        (
            "saveState",
            json!({"saveState": {}}),
            "Completed",
            Some("saveState"),
        ),
        (
            "loadState from a saveState checkpoint, with the optimizer",
            json!({"loadState": {"modelPackageArn": state_arn, "loadOptimizer": true}}),
            "Completed",
            Some("loadState"),
        ),
        (
            "loadState from a saveWeights checkpoint, without the optimizer",
            json!({"loadState": {"modelPackageArn": weights_arn.clone(), "loadOptimizer": false}}),
            "Completed",
            Some("loadState"),
        ),
        (
            "loadState from a saveWeights checkpoint, with the optimizer",
            json!({"loadState": {"modelPackageArn": weights_arn, "loadOptimizer": true}}),
            "Failed",
            None,
        ),
        (
            "loadState from a checkpoint this session never wrote",
            json!({"loadState": {
                "modelPackageArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/7"),
                "loadOptimizer": false,
            }}),
            "Failed",
            None,
        ),
    ];

    let mut ids = BTreeSet::new();
    for (index, (case, config, expected_status, expected_member)) in cases.iter().enumerate() {
        let sequence = 3 + index as i64;
        let id = submit_training_ok(&router, session, sequence, config.clone()).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: every submit must mint a fresh operation id, got {id:?} twice"
        );
        let answer = poll_training_to_terminal(&router, session, &id).await;
        let body = answer.json();
        assert_eq!(
            answer.text("status"),
            *expected_status,
            "case {case:?}: expected status {expected_status}, got {body}"
        );
        match expected_member {
            Some(member) => {
                assert!(
                    body["operationResult"].get(member).is_some(),
                    "case {case:?}: operationResult.{member} must be populated, got {body}"
                );
                assert!(
                    body.get("errorDetails").is_none(),
                    "case {case:?}: a completed operation reports no errorDetails, got {body}"
                );
            }
            None => {
                assert!(
                    body.get("operationResult").is_none(),
                    "case {case:?}: a failed operation reports no result, got {body}"
                );
                assert!(
                    !answer.text("errorDetails").is_empty(),
                    "case {case:?}: a failed operation must say why, got {body}"
                );
            }
        }
        // "The number of training tokens consumed by the operation": nothing put data
        // through a model, so nothing consumed training tokens.
        assert!(
            body.get("tokenUsage").is_none(),
            "case {case:?}: no token usage is reported for work that did not run, got {body}"
        );
    }
}

/// The same rule over `SampleOperationConfig`'s members.
#[tokio::test]
async fn every_sampling_operation_member_answers_from_stored_state() {
    let router = emulator();
    let session = "sampling-members";
    router.create_session(session);
    let checkpoint = write_checkpoint(&router, session, 1, false).await;

    let cases: Vec<(&str, Value, &str, Option<&str>)> = vec![
        ("sample of 1", sample_config(1), "Completed", Some("sample")),
        ("sample of 3", sample_config(3), "Completed", Some("sample")),
        (
            "sample of 1000",
            sample_config(1000),
            "Completed",
            Some("sample"),
        ),
        (
            "sample against a checkpoint this session never wrote",
            json!({"sample": {
                "prompt": model_input(&[7, 8]),
                "numSamples": 1,
                "samplingParams": {"maxTokens": 512},
                "modelPackageArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/3"),
            }}),
            "Failed",
            None,
        ),
        (
            "computeLogprobs",
            json!({"computeLogprobs": {"prompt": model_input(&[4, 5, 6])}}),
            "Completed",
            Some("computeLogprobs"),
        ),
        (
            "loadWeightsForSampling of a checkpoint the session holds",
            json!({"loadWeightsForSampling": {"modelPackageArn": checkpoint}}),
            "Completed",
            Some("loadWeightsForSampling"),
        ),
        (
            "loadWeightsForSampling of a checkpoint this session never wrote",
            json!({"loadWeightsForSampling": {
                "modelPackageArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/1"),
            }}),
            "Failed",
            None,
        ),
    ];

    for (case, config, expected_status, expected_member) in cases {
        let answer = submit_sampling(&router, session, json!({"operationConfig": config})).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let id = answer.text("operationId");
        assert!(
            is_operation_id(&id, "sampling"),
            "case {case:?}: expected a sampling operation id, got {id:?}"
        );
        let answer = poll_sampling_to_terminal(&router, session, &id).await;
        let body = answer.json();
        assert_eq!(
            answer.text("status"),
            expected_status,
            "case {case:?}: expected status {expected_status}, got {body}"
        );
        match expected_member {
            Some(member) => assert!(
                body["operationResult"].get(member).is_some(),
                "case {case:?}: operationResult.{member} must be populated, got {body}"
            ),
            None => {
                assert!(
                    body.get("operationResult").is_none(),
                    "case {case:?}: a failed operation reports no result, got {body}"
                );
                assert!(
                    !answer.text("errorDetails").is_empty(),
                    "case {case:?}: a failed operation must say why, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: checkpoint ARNs
// ---------------------------------------------------------------------------

/// A save operation's model package ARN has the documented form, carries the region the
/// request was signed for, and its version counts up once per checkpoint of the session.
#[tokio::test]
async fn checkpoint_arns_carry_the_requests_region_and_an_incrementing_version() {
    for region in ["us-west-2", "eu-central-1", "ap-southeast-3"] {
        let router = emulator();
        let session = "arn-table";
        router.create_session(session);
        for version in 1..=3i64 {
            let body = json!({
                "sequenceNumber": version,
                "operationConfig": {"saveState": {}},
            });
            let answer = call_in_region(
                &router,
                "POST",
                &format!("/v1/training-sessions/{session}/operations"),
                bytes::Bytes::from(serde_json::to_vec(&body).expect("serializes")),
                region,
                &[],
            )
            .await;
            assert_eq!(
                answer.status,
                200,
                "case {region} v{version}: expected HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let id = answer.text("operationId");
            let answer = poll_training_to_terminal_in(&router, session, &id, region).await;
            let arn = answer.json()["operationResult"]["saveState"]["modelPackageArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expected =
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-package/{session}/{version}");
            assert_eq!(
                arn, expected,
                "case {region} v{version}: expected the ARN {expected}, got {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the transition is owed to the clock
// ---------------------------------------------------------------------------

/// A checkpoint operation is non-terminal straight after the submit and terminal once
/// its stored due instant has passed; two reads of the settled operation agree, so the
/// result came from state rather than from the read.
#[tokio::test]
async fn a_checkpoint_operation_settles_only_once_its_due_instant_has_passed() {
    let router = emulator();
    let session = "clock";
    router.create_session(session);
    let id = submit_training_ok(&router, session, 1, json!({"saveState": {}})).await;

    let immediate = get_training(&router, session, &id).await;
    assert_eq!(
        immediate.text("status"),
        "InProgress",
        "case immediate poll: a submitted operation is still executing, got {}",
        immediate.json()
    );
    assert!(
        immediate.json().get("operationResult").is_none(),
        "case immediate poll: an unfinished operation has no result yet, got {}",
        immediate.json()
    );

    // Wait for the CONDITION the clock owes rather than for a span of wall clock: the
    // poll loop looks again until the transition has been applied or its generous budget
    // runs out.
    let first = poll_training_to_terminal(&router, session, &id).await;
    let second = get_training(&router, session, &id).await;
    assert_eq!(
        first.text("status"),
        "Completed",
        "case due poll: the transition the clock owed must have been applied, got {}",
        first.json()
    );
    assert_eq!(
        first.json(),
        second.json(),
        "case repeated poll: two reads of a settled operation must agree"
    );
}

/// Each queued operation's due instant is chained from its predecessor's, so a backlog
/// drains on instants the clock already owed rather than one per poll: three operations
/// submitted back to back and left alone are all terminal at the FIRST read taken after
/// three spans have passed.
#[tokio::test]
async fn a_queued_backlog_drains_on_the_instants_the_clock_owed() {
    let router = emulator();
    let session = "backlog";
    router.create_session(session);
    let mut ids = Vec::new();
    for sequence in 1..=3i64 {
        ids.push(submit_training_ok(&router, session, sequence, json!({"saveState": {}})).await);
    }
    // The LAST slot is what the whole backlog has to drain for, so that is the condition
    // waited on — and nothing in between is read, so the earlier slots were settled by
    // the instants the clock owed them and not by a poll of their own.
    let last = ids.last().expect("three operations were submitted");
    poll_training_to_terminal(&router, session, last).await;
    for (index, id) in ids.iter().enumerate() {
        let answer = get_training(&router, session, id).await;
        assert_eq!(
            answer.text("status"),
            "Completed",
            "case slot {}: the whole backlog must be settled at the first read, got {}",
            index + 1,
            answer.json()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: clientToken idempotency
// ---------------------------------------------------------------------------
/// "If you retry a request with the same client token and the same parameters, the
/// service ignores the request but does not return an error": the retry answers the id
/// the first call minted, and the operation it names is the one that was stored. The
/// same token carrying DIFFERENT parameters is not that retry, and conflicts.
#[tokio::test]
async fn a_repeated_client_token_answers_the_same_operation_id() {
    let router = emulator();
    let session = "idempotency";
    router.create_session(session);
    // Tokens spanning `ClientToken`'s 1..128 length and `^[a-zA-Z0-9-]+$` pattern.
    let tokens = [
        "a",
        "tok-1",
        "0123456789",
        "A-very-long-client-token-0123456789-0123456789-0123456789-0123456789",
    ];
    let mut minted = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate() {
        let sequence = 1 + index as i64;
        let body = json!({
            "sequenceNumber": sequence,
            "clientToken": token,
            "operationConfig": {"saveState": {}},
        });
        let first = submit_training(&router, session, body.clone()).await;
        assert_eq!(
            first.status,
            200,
            "case {token:?}: expected HTTP 200, got {} {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let id = first.text("operationId");
        assert!(
            minted.insert(id.clone()),
            "case {token:?}: a fresh token must mint a fresh id, got {id:?} twice"
        );

        // The retry carrying the SAME parameters is ignored and answers the first id.
        let again = submit_training(&router, session, body.clone()).await;
        assert_eq!(
            again.status,
            200,
            "case {token:?} replay: expected HTTP 200, got {} {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert_eq!(
            again.text("operationId"),
            id,
            "case {token:?} replay: the retry must answer the first id {id:?}"
        );

        // The same token with DIFFERENT parameters is not that retry: it is refused
        // rather than silently discarding the operation the caller asked for.
        for (case, reuse) in [
            (
                "a different sequenceNumber",
                json!({
                    "sequenceNumber": sequence + 100,
                    "clientToken": token,
                    "operationConfig": {"saveState": {}},
                }),
            ),
            (
                "a different operationConfig member",
                json!({
                    "sequenceNumber": sequence,
                    "clientToken": token,
                    "operationConfig": {"saveWeights": {"mergeWeights": true}},
                }),
            ),
            (
                "a different member value",
                json!({
                    "sequenceNumber": sequence,
                    "clientToken": token,
                    "operationConfig": {"saveState": {"description": "something else"}},
                }),
            ),
        ] {
            let conflict = submit_training(&router, session, reuse.clone()).await;
            assert_eq!(
                conflict.status,
                409,
                "case {token:?} reused with {case}: expected HTTP 409, got {} {}",
                conflict.status,
                String::from_utf8_lossy(&conflict.body)
            );
            assert_eq!(
                conflict.error_code(),
                "ConflictException",
                "case {token:?} reused with {case}: expected ConflictException, got {}",
                conflict.json()
            );
        }

        let answer = poll_training_to_terminal(&router, session, &id).await;
        assert_eq!(
            answer.text("status"),
            "Completed",
            "case {token:?}: the single stored operation must settle, got {}",
            answer.json()
        );
    }

    // One operation per token, each holding one checkpoint version: a retry created no
    // second operation, so the fourth save is version 4 and not version 7.
    let next_free_slot = tokens.len() as i64 + 1;
    let id = submit_training_ok(&router, session, next_free_slot, json!({"saveState": {}})).await;
    let answer = poll_training_to_terminal(&router, session, &id).await;
    let arn = answer.json()["operationResult"]["saveState"]["modelPackageArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.ends_with(&format!("/{session}/{}", tokens.len() + 1)),
        "case checkpoint count: {} retries must have created no operations, got {arn:?}",
        tokens.len()
    );
}

// ---------------------------------------------------------------------------
// Rule 6: sequence-number ordering
// ---------------------------------------------------------------------------

/// A slot that is taken, or that the session has already executed past, cannot accept
/// another operation.
#[tokio::test]
async fn a_taken_or_passed_sequence_number_conflicts() {
    let router = emulator();
    let session = "ordering";
    router.create_session(session);
    for sequence in [1i64, 2, 3] {
        let id = submit_training_ok(&router, session, sequence, json!({"saveState": {}})).await;
        poll_training_to_terminal(&router, session, &id).await;
    }
    // Queued but not executed: slot 9 is occupied while the session blocks on the gap.
    let queued = submit_training_ok(&router, session, 9, json!({"saveState": {}})).await;

    for (case, sequence) in [
        ("the first executed slot", 1i64),
        ("the last executed slot", 3),
        ("a slot that is queued but not executed", 9),
    ] {
        let answer = submit_training(
            &router,
            session,
            json!({"sequenceNumber": sequence, "operationConfig": {"saveState": {}}}),
        )
        .await;
        assert_eq!(
            answer.status,
            409,
            "case {case:?} (sequence {sequence}): expected HTTP 409, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ConflictException",
            "case {case:?} (sequence {sequence}): expected ConflictException, got {}",
            answer.json()
        );
    }

    // "If a gap exists, the session blocks until the missing operation arrives": the
    // queued operation does not execute while slots 4..8 are empty, however long passes.
    // What is waited on is an operation of ANOTHER session settling — a condition, not a
    // span of wall clock — which is reached only once a transition has really fallen due,
    // so the gapped operation has had that long to execute and has not.
    let elsewhere = "ordering-clock";
    router.create_session(elsewhere);
    let ticking = submit_training_ok(&router, elsewhere, 1, json!({"saveState": {}})).await;
    poll_training_to_terminal(&router, elsewhere, &ticking).await;
    let blocked = get_training(&router, session, &queued).await;
    assert_eq!(
        blocked.text("status"),
        "Pending",
        "case gap: an operation behind a gap must stay Pending, got {}",
        blocked.json()
    );

    // Filling every missing slot releases the queue, and the operation behind the gap
    // then executes.
    for sequence in 4..=8i64 {
        let id = submit_training_ok(&router, session, sequence, json!({"saveState": {}})).await;
        poll_training_to_terminal(&router, session, &id).await;
    }
    let released = poll_training_to_terminal(&router, session, &queued).await;
    assert_eq!(
        released.text("status"),
        "Completed",
        "case gap filled: the queued operation must execute once the gap closes, got {}",
        released.json()
    );
}

/// A store holding every session it retains still accepts a submit against a new session
/// name: the oldest session that nothing is owed on is released to make room.
///
/// A session whose every operation has settled goes first. Where none has settled, the
/// one released is the oldest whose queue is waiting on an operation nobody submitted —
/// that session executes nothing until a caller supplies the missing sequence number, so
/// keeping it would refuse every new session for as long as the caller never does. The
/// sessions that are still executing are kept.
#[tokio::test]
async fn a_full_store_releases_a_session_nothing_is_owed_on_rather_than_refusing() {
    use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::state::MAX_SESSIONS;

    let router = emulator();
    // Every session but one is blocked on a gap: slot 1 is never submitted, so the
    // operation queued at slot 2 can never execute and the session can never settle.
    let blocked: Vec<String> = (0..MAX_SESSIONS - 1)
        .map(|index| format!("blocked-{index:04}"))
        .collect();
    let mut blocked_ops = Vec::with_capacity(blocked.len());
    for session in &blocked {
        router.create_session(session);
        blocked_ops.push(submit_training_ok(&router, session, 2, json!({"saveState": {}})).await);
    }
    // And one session that HAS settled, which is the one a release must take first.
    router.create_session("settled");
    let settled_op = submit_training_ok(&router, "settled", 1, json!({"saveState": {}})).await;
    poll_training_to_terminal(&router, "settled", &settled_op).await;

    // The store is full. A submit against a new name is accepted, and what it released is
    // the settled session, not one of the blocked ones.
    router.create_session("fresh-one");
    let fresh_op = submit_training_ok(&router, "fresh-one", 1, json!({"saveState": {}})).await;
    let released = get_training(&router, "settled", &settled_op).await;
    assert_eq!(
        released.status,
        404,
        "case settled first: the settled session must be the one released, got {} {}",
        released.status,
        String::from_utf8_lossy(&released.body)
    );
    let kept = get_training(&router, &blocked[0], &blocked_ops[0]).await;
    assert_eq!(
        kept.text("status"),
        "Pending",
        "case settled first: a blocked session must be kept while a settled one exists, got {}",
        kept.json()
    );

    // The store is full again and nothing has settled — the operation the previous submit
    // created is still executing — so the release takes the OLDEST blocked session, and
    // the next-oldest stays.
    router.create_session("fresh-two");
    let second_op = submit_training_ok(&router, "fresh-two", 1, json!({"saveState": {}})).await;
    let oldest = get_training(&router, &blocked[0], &blocked_ops[0]).await;
    assert_eq!(
        oldest.status,
        404,
        "case blocked next: the oldest blocked session must be released, got {} {}",
        oldest.status,
        String::from_utf8_lossy(&oldest.body)
    );
    let next_oldest = get_training(&router, &blocked[1], &blocked_ops[1]).await;
    assert_eq!(
        next_oldest.text("status"),
        "Pending",
        "case blocked next: only one session is released per submit, got {}",
        next_oldest.json()
    );

    // Both new sessions hold the operation their submit created, and both settle.
    for (session, id) in [("fresh-one", &fresh_op), ("fresh-two", &second_op)] {
        let answer = poll_training_to_terminal(&router, session, id).await;
        assert!(
            is_terminal(&answer.text("status")),
            "case {session:?}: the operation the accepted submit created must settle, got {}",
            answer.json()
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: unknown sessions and operations
// ---------------------------------------------------------------------------


/// A session this emulator has never seen, and an operation that does not belong to the
/// session named, are `ResourceNotFoundException` — never a canned terminal status.
#[tokio::test]
async fn an_unknown_session_or_operation_is_not_found() {
    let router = emulator();
    // The two sessions the control plane created; every other name below is one no create
    // ever named.
    router.create_session("known");
    router.create_session("other-session");
    let training = submit_training_ok(&router, "known", 1, json!({"saveState": {}})).await;
    let sampling = submit_sampling(
        &router,
        "known",
        json!({"operationConfig": {"computeLogprobs": {"prompt": model_input(&[1])}}}),
    )
    .await
    .text("operationId");
    // A second session, so "belongs to another session" is a real case and not just an
    // id this emulator never minted.
    let other = submit_training_ok(&router, "other-session", 1, json!({"saveState": {}})).await;
    let cases: Vec<(&str, String)> = vec![
        (
            "a session never submitted to",
            format!("/v1/training-sessions/absent-session/operations/{training}"),
        ),
        (
            "an operation id never minted",
            "/v1/training-sessions/known/operations/does-not-exist".to_string(),
        ),
        (
            "a training operation of another session",
            format!("/v1/training-sessions/known/operations/{other}"),
        ),
        (
            "a sampling id on the training route",
            format!("/v1/training-sessions/known/operations/{sampling}"),
        ),
        (
            "a session never submitted to, on the sampling route",
            format!("/v1/training-sessions/absent-session/sampling-operations/{sampling}"),
        ),
        (
            "a sampling operation id never minted",
            "/v1/training-sessions/known/sampling-operations/does-not-exist".to_string(),
        ),
        (
            "a training id on the sampling route",
            format!("/v1/training-sessions/known/sampling-operations/{training}"),
        ),
    ];

    for (case, path) in cases {
        let answer = call(&router, "GET", &path, None).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected HTTP 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.json()
        );
    }

    // A read never brings a session into existence, so the same GET is still a 404 after
    // it has been tried.
    let again = call(
        &router,
        "GET",
        "/v1/training-sessions/absent-session/operations/does-not-exist",
        None,
    )
    .await;
    assert_eq!(
        again.status, 404,
        "case repeated read: a read must not materialise a session, got {} {}",
        again.status,
        String::from_utf8_lossy(&again.body)
    );
}

// ---------------------------------------------------------------------------
// Rule 8: refusals
// ---------------------------------------------------------------------------

/// Every request that breaks a modelled constraint, or a rule the model states in prose,
/// is refused with HTTP 400 `ValidationException`.
///
/// The numeric rows walk each published bound and one step past it: `sequenceNumber`
/// `@range(min: 1)`, `SampleConfig$numSamples` `@range(min: 1, max: 1000)`,
/// `SamplingParams$maxTokens` `@range(min: 512, max: 131072)`, and the
/// `clip_low_threshold` / `clip_high_threshold` ranges `ForwardBackwardConfig$lossFnConfig`
/// documents.
#[tokio::test]
async fn an_invalid_request_is_refused_with_validation_exception() {
    let router = emulator();
    let session = "refusals";
    router.create_session(session);

    let training_cases: Vec<(&str, Value)> = vec![
        (
            "sequenceNumber below its minimum",
            json!({"sequenceNumber": 0, "operationConfig": {"saveState": {}}}),
        ),
        (
            "a negative sequenceNumber",
            json!({"sequenceNumber": -1, "operationConfig": {"saveState": {}}}),
        ),
        (
            "no operationConfig member set",
            json!({"sequenceNumber": 1, "operationConfig": {}}),
        ),
        (
            "two operationConfig members set",
            json!({"sequenceNumber": 1, "operationConfig": {
                "saveState": {}, "saveWeights": {},
            }}),
        ),
        (
            "an empty datumList",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "ppo", "datumList": [],
            }}}),
        ),
        (
            "a datum whose modelInput carries no chunk",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "ppo", "datumList": [{"modelInput": {"chunks": []}}],
            }}}),
        ),
        (
            "a chunk carrying no token id",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "ppo",
                "datumList": [{"modelInput": {"chunks": [{"tokens": [], "type": "encoded_text"}]}}],
            }}}),
        ),
        (
            "a lossFnConfig key the engine does not support",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "cispo",
                "datumList": [{"modelInput": model_input(&[1])}],
                "lossFnConfig": {"clip_mid_threshold": 0.5},
            }}}),
        ),
        (
            "clip_low_threshold above its range",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "cispo",
                "datumList": [{"modelInput": model_input(&[1])}],
                "lossFnConfig": {"clip_low_threshold": 1.5},
            }}}),
        ),
        (
            "clip_high_threshold below its range",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "cispo",
                "datumList": [{"modelInput": model_input(&[1])}],
                "lossFnConfig": {"clip_high_threshold": 0.5},
            }}}),
        ),
        (
            "clip_high_threshold above its range",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "cispo",
                "datumList": [{"modelInput": model_input(&[1])}],
                "lossFnConfig": {"clip_high_threshold": 10.5},
            }}}),
        ),
        (
            "a lossFnName outside the enum",
            json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
                "lossFnName": "not_a_loss_function",
                "datumList": [{"modelInput": model_input(&[1])}],
            }}}),
        ),
        (
            "a loadState ARN that is not an ARN",
            json!({"sequenceNumber": 1, "operationConfig": {"loadState": {
                "modelPackageArn": "my-checkpoint", "loadOptimizer": false,
            }}}),
        ),
        (
            "a loadState model package GROUP ARN",
            json!({"sequenceNumber": 1, "operationConfig": {"loadState": {
                "modelPackageArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package-group/group-1"),
                "loadOptimizer": false,
            }}}),
        ),
        (
            "a loadState ARN with no version",
            json!({"sequenceNumber": 1, "operationConfig": {"loadState": {
                "modelPackageArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/no-version"),
                "loadOptimizer": false,
            }}}),
        ),
        (
            "a loadState ARN of another service",
            json!({"sequenceNumber": 1, "operationConfig": {"loadState": {
                "modelPackageArn":
                    format!("arn:aws:s3:{REGION}:{ACCOUNT}:model-package/name/1"),
                "loadOptimizer": false,
            }}}),
        ),
    ];

    for (case, body) in training_cases {
        let answer = submit_training(&router, session, body.clone()).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?} ({body}): expected HTTP 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            answer.json()
        );
    }

    let sampling_cases: Vec<(&str, Value)> = vec![
        (
            "no operationConfig member set",
            json!({"operationConfig": {}}),
        ),
        (
            "numSamples below its minimum",
            json!({"operationConfig": sample_config(0)}),
        ),
        (
            "numSamples above its maximum",
            json!({"operationConfig": sample_config(1001)}),
        ),
        (
            "maxTokens below its minimum",
            json!({"operationConfig": {"sample": {
                "prompt": model_input(&[1]), "numSamples": 1,
                "samplingParams": {"maxTokens": 511},
            }}}),
        ),
        (
            "maxTokens above its maximum",
            json!({"operationConfig": {"sample": {
                "prompt": model_input(&[1]), "numSamples": 1,
                "samplingParams": {"maxTokens": 131073},
            }}}),
        ),
        (
            "a prompt carrying no chunk",
            json!({"operationConfig": {"sample": {
                "prompt": {"chunks": []}, "numSamples": 1,
                "samplingParams": {"maxTokens": 512},
            }}}),
        ),
        (
            "a negative topkPromptLogprobs",
            json!({"operationConfig": {"sample": {
                "prompt": model_input(&[1]), "numSamples": 1,
                "samplingParams": {"maxTokens": 512}, "topkPromptLogprobs": -1,
            }}}),
        ),
        (
            "a computeLogprobs prompt carrying no chunk",
            json!({"operationConfig": {"computeLogprobs": {"prompt": {"chunks": []}}}}),
        ),
        (
            "a loadWeightsForSampling ARN that is not an ARN",
            json!({"operationConfig": {"loadWeightsForSampling": {
                "modelPackageArn": "latest",
            }}}),
        ),
    ];

    for (case, body) in sampling_cases {
        let answer = submit_sampling(&router, session, body.clone()).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?} ({body}): expected HTTP 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            answer.json()
        );
    }
}

/// The rows at each published bound are ACCEPTED, so the refusals above are the bound and
/// not a blanket rejection.
#[tokio::test]
async fn a_request_at_each_published_bound_is_accepted() {
    let router = emulator();
    let session = "bounds";
    router.create_session(session);

    for (case, num_samples, max_tokens, topk) in [
        ("the lowest numSamples and maxTokens", 1i64, 512i64, 0i64),
        ("the highest numSamples", 1000, 512, 1),
        ("the highest maxTokens", 1, 131072, 8),
    ] {
        let body = json!({"operationConfig": {"sample": {
            "prompt": model_input(&[1, 2]),
            "numSamples": num_samples,
            "samplingParams": {"maxTokens": max_tokens},
            "topkPromptLogprobs": topk,
            "includePromptLogprobs": true,
        }}});
        let answer = submit_sampling(&router, session, body.clone()).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} ({body}): expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    for (case, threshold, value) in [
        ("clip_low_threshold at its minimum", "clip_low_threshold", 0.0),
        ("clip_low_threshold at its maximum", "clip_low_threshold", 1.0),
        (
            "clip_high_threshold at its minimum",
            "clip_high_threshold",
            1.0,
        ),
        (
            "clip_high_threshold at its maximum",
            "clip_high_threshold",
            10.0,
        ),
    ] {
        let body = json!({"sequenceNumber": 1, "operationConfig": {"forwardBackward": {
            "lossFnName": "cispo",
            "datumList": [{"modelInput": model_input(&[1])}],
            "lossFnConfig": {threshold: value},
        }}});
        // A fresh session per row, so each one submits into the same free slot. The name
        // stays inside `TrainingSessionName`'s `^[a-zA-Z0-9-]+$` pattern.
        let suffix = threshold.replace('_', "-");
        let row_session = format!("{session}-{suffix}-{value}");
        router.create_session(&row_session);
        let answer = submit_training(&router, &row_session, body).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a value at the published bound must be accepted, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 9: the synchronous inference call
// ---------------------------------------------------------------------------

/// `SampleTrainingSession` resolves the checkpoint from stored session state, and answers
/// a JSON body that reports what was served rather than the request's own payload.
#[tokio::test]
async fn sample_training_session_answers_from_the_sessions_checkpoint() {
    let router = emulator();
    let session = "inference";
    router.create_session(session);

    // A session this emulator has never seen.
    let absent = call_in_region(
        &router,
        "POST",
        "/training-sessions/absent-session/sample",
        bytes::Bytes::from_static(b"{\"prompt\":\"hi\"}"),
        REGION,
        &[],
    )
    .await;
    assert_eq!(
        absent.status,
        404,
        "case unknown session: expected HTTP 404, got {} {}",
        absent.status,
        String::from_utf8_lossy(&absent.body)
    );
    // Both inference operations resolve a session through the same lookup, so both
    // report the identity it failed on the same way.
    assert_eq!(
        absent.text("message"),
        format!("Session not found: absent-session-{ACCOUNT}-{REGION}"),
        "case unknown session: expected the session identity the lookup failed on, got {}",
        String::from_utf8_lossy(&absent.body)
    );

    // A session that exists but holds no checkpoint: there is no model to sample.
    submit_training_ok(&router, session, 1, forward_backward("ppo")).await;
    let no_model = call_in_region(
        &router,
        "POST",
        &format!("/training-sessions/{session}/sample"),
        bytes::Bytes::from_static(b"{}"),
        REGION,
        &[],
    )
    .await;
    assert_eq!(
        no_model.status,
        409,
        "case no checkpoint: expected HTTP 409, got {} {}",
        no_model.status,
        String::from_utf8_lossy(&no_model.body)
    );
    assert_eq!(
        no_model.error_code(),
        "ConflictException",
        "case no checkpoint: expected ConflictException, got {}",
        no_model.json()
    );

    let checkpoint = write_checkpoint(&router, session, 2, true).await;

    // Payloads of several sizes: the answer reports the size that arrived and is never
    // the payload itself.
    for payload in [
        bytes::Bytes::new(),
        bytes::Bytes::from_static(b"{}"),
        bytes::Bytes::from(vec![b'x'; 4096]),
    ] {
        let size = payload.len();
        let answer = call_in_region(
            &router,
            "POST",
            &format!("/training-sessions/{session}/sample"),
            payload.clone(),
            REGION,
            &[],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {size}-byte payload: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some("application/json"),
            "case {size}-byte payload: expected the Content-Type of the body served, got {:?}",
            answer.content_type
        );
        assert_ne!(
            answer.body, payload,
            "case {size}-byte payload: the response must not be the request body echoed back"
        );
        let body = answer.json();
        assert_eq!(
            body["modelPackageArn"].as_str(),
            Some(checkpoint.as_str()),
            "case {size}-byte payload: expected the session's checkpoint, got {body}"
        );
        assert_eq!(
            body["trainingSessionName"].as_str(),
            Some(session),
            "case {size}-byte payload: expected the session named, got {body}"
        );
        assert_eq!(
            body["requestBodyBytes"].as_u64(),
            Some(size as u64),
            "case {size}-byte payload: expected the served payload size, got {body}"
        );
        assert!(
            !answer.text("message").is_empty(),
            "case {size}-byte payload: the body must say what it is, got {body}"
        );
    }

    // The emulator serves its own JSON envelope whatever the caller's `Accept` names:
    // the model's own response type is not knowable without the model, and the operation
    // declares no error for a content type it cannot serve.
    for (case, accept) in [
        ("any type", "*/*"),
        ("the type served", "application/json"),
        ("a type with parameters", "application/json; charset=utf-8"),
        ("one of several types", "text/plain, application/json"),
        ("a type the emulator does not produce", "text/event-stream"),
    ] {
        let answer = call_in_region(
            &router,
            "POST",
            &format!("/training-sessions/{session}/sample"),
            bytes::Bytes::from_static(b"{}"),
            REGION,
            &[("accept", accept.to_string())],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} (accept {accept:?}): expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: the Content-Type must describe the body served, got {:?}",
            answer.content_type
        );
    }

    // The model package header names which checkpoint serves the request.
    for (case, arn, expected) in [
        ("a checkpoint the session holds", checkpoint.clone(), 200u16),
        (
            "a well-formed ARN the session never wrote",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/1"),
            404,
        ),
        ("an ARN that is not one", "checkpoint-1".to_string(), 400),
    ] {
        let answer = call_in_region(
            &router,
            "POST",
            &format!("/training-sessions/{session}/sample"),
            bytes::Bytes::from_static(b"{}"),
            REGION,
            &[("x-amzn-sagemaker-model-package", arn.clone())],
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?} ({arn}): expected HTTP {expected}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 10: the streaming inference call
// ---------------------------------------------------------------------------

/// The `data:` payloads of a Server-Sent Events body, in the order they arrive.
///
/// `SampleResponseStream`: "Each event is a `data: {JSON}` line containing a chat
/// completion chunk, and the stream is terminated by `data: [DONE]`."
fn sse_events(body: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(body).to_string();
    text.split("\n\n")
        .map(str::trim_end)
        .filter(|frame| !frame.is_empty())
        .map(|frame| {
            frame
                .strip_prefix("data: ")
                .unwrap_or_else(|| {
                    panic!("every event of a text/event-stream body is a `data: ` line, got {frame:?}")
                })
                .to_string()
        })
        .collect()
}

/// Send one streaming inference request.
async fn sample_with_response_stream(
    router: &MultiServiceRouter,
    session: &str,
    payload: bytes::Bytes,
    headers: &[(&str, String)],
) -> Answer {
    call_in_region(
        router,
        "POST",
        &format!("/training-sessions/{session}/sample-with-response-stream"),
        payload,
        REGION,
        headers,
    )
    .await
}

/// `SampleTrainingSessionWithResponseStream` frames its answer as Server-Sent Events —
/// one `data:` chunk and the `data: [DONE]` terminator — whatever the caller's `Accept`
/// names and whatever size the payload is, and the chunk reports what was served from
/// stored session state rather than the request's own payload.
#[tokio::test]
async fn the_streaming_inference_call_frames_server_sent_events() {
    let router = emulator();
    let session = "streaming";
    router.create_session(session);
    let checkpoint = write_checkpoint(&router, session, 1, true).await;

    // Payloads of several sizes: the chunk reports the size that arrived and the body is
    // never the payload re-framed as if a model had generated it.
    for payload in [
        bytes::Bytes::new(),
        bytes::Bytes::from_static(b"{\"prompt\":\"hi\"}"),
        bytes::Bytes::from(vec![b'x'; 4096]),
    ] {
        let size = payload.len();
        let answer = sample_with_response_stream(&router, session, payload.clone(), &[]).await;
        assert_eq!(
            answer.status,
            200,
            "case {size}-byte payload: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some("text/event-stream"),
            "case {size}-byte payload: expected the streamed media type, got {:?}",
            answer.content_type
        );
        assert_ne!(
            answer.body, payload,
            "case {size}-byte payload: the stream must not be the request body echoed back"
        );

        let events = sse_events(&answer.body);
        assert!(
            events.len() >= 2,
            "case {size}-byte payload: expected a chunk event and a terminator, got {events:?}"
        );
        assert_eq!(
            events.last().map(String::as_str),
            Some("[DONE]"),
            "case {size}-byte payload: the stream must end with the [DONE] marker, got {events:?}"
        );
        for (index, event) in events.iter().take(events.len() - 1).enumerate() {
            let chunk: Value = serde_json::from_str(event).unwrap_or_else(|e| {
                panic!("case {size}-byte payload: event {index} is not JSON ({e}): {event:?}")
            });
            assert_eq!(
                chunk["trainingSessionName"].as_str(),
                Some(session),
                "case {size}-byte payload: expected the session named, got {chunk}"
            );
            assert_eq!(
                chunk["modelPackageArn"].as_str(),
                Some(checkpoint.as_str()),
                "case {size}-byte payload: expected the session's checkpoint, got {chunk}"
            );
            assert_eq!(
                chunk["requestBodyBytes"].as_u64(),
                Some(size as u64),
                "case {size}-byte payload: expected the served payload size, got {chunk}"
            );
            assert!(
                chunk["message"].as_str().is_some_and(|m| !m.is_empty()),
                "case {size}-byte payload: the chunk must say what it is, got {chunk}"
            );
        }
    }

    // The stream's media type is the one the model puts on `SampleResponseStream`,
    // whatever the caller's `Accept` names: the operation declares no error for a content
    // type it cannot serve.
    for (case, accept) in [
        ("any type", "*/*"),
        ("the type served", "text/event-stream"),
        ("the type with parameters", "text/event-stream; charset=utf-8"),
        ("one of several types", "application/json, text/event-stream"),
        ("a type the emulator does not produce", "application/cbor"),
    ] {
        let answer = sample_with_response_stream(
            &router,
            session,
            bytes::Bytes::from_static(b"{}"),
            &[("accept", accept.to_string())],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} (accept {accept:?}): expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some("text/event-stream"),
            "case {case:?}: the Content-Type must describe the body served, got {:?}",
            answer.content_type
        );
    }
}

/// The streaming call resolves its target, and refuses, exactly as the synchronous one
/// does: "Errors that occur before streaming starts are returned as the operation's
/// errors", so each refusal is the modelled error and carries no `data:` frame.
#[tokio::test]
async fn the_streaming_inference_call_refuses_what_the_synchronous_one_refuses() {
    let router = emulator();
    let session = "streaming-refusals";
    router.create_session(session);

    // A session this emulator has never seen is not materialised by an inference call,
    // and the refusal reports the whole identity the lookup failed on: the session name,
    // the account that would own it and the region it would live in.
    for (case, name, region) in [
        ("a name never submitted", "absent-session", REGION),
        ("another absent name", "absent-session-2", REGION),
        ("the same name in another region", "absent-session", "eu-west-1"),
    ] {
        let absent = call_in_region(
            &router,
            "POST",
            &format!("/training-sessions/{name}/sample-with-response-stream"),
            bytes::Bytes::from_static(b"{}"),
            region,
            &[],
        )
        .await;
        assert_eq!(
            (absent.status, absent.error_code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {} {} {}",
            absent.status,
            absent.error_code(),
            String::from_utf8_lossy(&absent.body)
        );
        assert_eq!(
            absent.text("message"),
            format!("Session not found: {name}-{ACCOUNT}-{region}"),
            "case {case:?}: expected the session identity the lookup failed on, got {}",
            String::from_utf8_lossy(&absent.body)
        );
    }

    // A session that exists but holds no checkpoint: there is no model to sample.
    submit_training_ok(&router, session, 1, forward_backward("ppo")).await;
    let no_model =
        sample_with_response_stream(&router, session, bytes::Bytes::from_static(b"{}"), &[]).await;
    assert_eq!(
        (no_model.status, no_model.error_code().as_str()),
        (409, "ConflictException"),
        "case no checkpoint: expected a 409 ConflictException, got {} {} {}",
        no_model.status,
        no_model.error_code(),
        String::from_utf8_lossy(&no_model.body)
    );

    // The model package header names which checkpoint serves the request.
    let checkpoint = write_checkpoint(&router, session, 2, false).await;
    for (case, arn, expected_status, expected_code) in [
        (
            "a checkpoint the session holds",
            checkpoint.clone(),
            200u16,
            "",
        ),
        (
            "a well-formed ARN the session never wrote",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/1"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN that is not one",
            "checkpoint-1".to_string(),
            400,
            "ValidationException",
        ),
    ] {
        let answer = sample_with_response_stream(
            &router,
            session,
            bytes::Bytes::from_static(b"{}"),
            &[("x-amzn-sagemaker-model-package", arn.clone())],
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?} ({arn}): expected HTTP {expected_status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 200 {
            assert_eq!(
                sse_events(&answer.body).last().map(String::as_str),
                Some("[DONE]"),
                "case {case:?}: a served stream ends with the [DONE] marker, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        } else {
            assert_eq!(
                answer.error_code(),
                expected_code,
                "case {case:?} ({arn}): expected {expected_code}, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                !String::from_utf8_lossy(&answer.body).contains("data: "),
                "case {case:?}: a refusal before the stream starts carries no event, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

/// The session store is bounded AND always makes room: past its bound the oldest
/// session is released, so sessions nothing can ever settle — a queue blocked on a
/// missing sequence number never reaches a terminal status — cannot fill the store and
/// leave every later submit refused for the life of the process.
#[tokio::test]
async fn the_session_store_still_accepts_a_submit_once_it_is_full_of_blocked_sessions() {
    use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::state::MAX_SESSIONS;

    let router = emulator();
    // One session per slot, each holding a single operation parked behind a gap: the
    // queue starts at sequence number 1, so a slot above that never executes and the
    // session never settles.
    for index in 0..MAX_SESSIONS {
        let session = format!("blocked-{index}");
        router.create_session(&session);
        let answer = submit_training(
            &router,
            &session,
            json!({
                "sequenceNumber": 5,
                "operationConfig": {"saveState": {}},
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case blocked-{index}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The store is full of sessions that can never settle; a submit naming a fresh
    // session is still served, and so is the one after it.
    for name in ["after-the-bound", "after-the-bound-two"] {
        router.create_session(name);
        let answer = submit_training(
            &router,
            name,
            json!({
                "sequenceNumber": 1,
                "operationConfig": {"saveState": {}},
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {name}: a full store releases its oldest session rather than refusing \
             for ever, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let id = answer.text("operationId");
        assert!(
            !id.is_empty(),
            "case {name}: expected the submit to mint an operation id, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        // The operation the accepted submit stored is readable back, so the session it
        // was made in is the one the store kept.
        let poll = call(
            &router,
            "GET",
            &format!("/v1/training-sessions/{name}/operations/{id}"),
            None,
        )
        .await;
        assert_eq!(
            poll.status,
            200,
            "case {name}: expected the stored operation to be readable, got {} {}",
            poll.status,
            String::from_utf8_lossy(&poll.body)
        );
    }
}
