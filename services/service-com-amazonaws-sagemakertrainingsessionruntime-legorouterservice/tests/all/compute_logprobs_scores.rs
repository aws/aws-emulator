// SPDX-License-Identifier: CC0-1.0

//! What a `computeLogprobs` sampling operation SERVES, end to end over the wire.
//!
//! RULE: a sampling operation this emulator can execute settles in the terminal status
//! that outcome is — `Completed`, with the `SampleOperationResult` member matching the
//! `operationConfig` member that was submitted, and no `errorDetails`, which
//! `GetSamplingOperationResult` populates "only when the status is Failed". For the
//! `computeLogprobs` member that means `operationResult.computeLogprobs.logprobs` carries
//! exactly one score per prompt token counted across every chunk, in prompt order: the
//! first token has no preceding context, so no conditional log-probability is defined for
//! it and none is reported, and every later token carries a finite log-probability — a
//! non-positive double, since a probability is at most one.
//!
//! No language model runs in this emulator, so the score a token carries is the one the
//! maximum-entropy distribution over the served vocabulary gives it (see the crate's
//! `state::prompt_logprobs`): what is asserted here is therefore the modelled SHAPE of the
//! answer and the invariants of a log-probability, plus the one thing the request decides —
//! that the chunking of a prompt is framing and not content, so the same token sequence
//! submitted as one chunk or as several is scored identically.
//!
//! The rows span what the rule generalises over rather than one recorded call: a prompt of
//! a single token, of a few, of many; one chunk and several; chunks of differing lengths;
//! and a named model package the session holds as well as one it does not.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The Region every call here signs for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// How many polls a terminal status is waited for, and how long each one waits. Far above
/// the service's own execution span, so a loaded host still reaches the condition: the
/// loop waits for the STATUS, never for a fixed time.
const POLL_BUDGET: u32 = 400;
/// See [`POLL_BUDGET`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One emulator: a shared engine with this service mounted on the production router.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Register `name` the way the SageMaker control plane's `CreateTrainingSession` does,
    /// which is the only thing that makes a training session exist.
    fn create_session(&self, name: &str) {
        self.engine
            .state::<service_common::training_sessions::TrainingSessionDirectory>()
            .lock()
            .register(service_common::training_sessions::TrainingSessionRecord {
                name: name.to_string(),
                arn: engine::emulator_arn("sagemaker", REGION, &format!("training-session/{name}")),
                // A session whose compute the control plane has brought up: "The session
                // must be in Running or Blocked state to accept requests."
                lifecycle: service_common::training_sessions::TrainingSessionLifecycle::settled(
                    "Running",
                ),
                // Of a base model family this emulator publishes no context window for, so
                // the prompts below are scored rather than refused for their length (the
                // ceiling has its own suite).
                base_model_arn: format!(
                    "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
                     meta-textgeneration-llama-3-8b/1.0.0"
                ),
            });
    }
}

/// What the emulator answered.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn member(&self, member: &str) -> String {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn call(emulator: &Emulator, method: &str, path: &str, body: &[u8]) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
         SignedHeaders=host, Signature=00"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://training-session-runtime.sagemaker.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request builds");
    let response = emulator.router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().to_vec(),
    }
}

/// Submit one sampling operation and answer the operation id it minted.
async fn submit(emulator: &Emulator, session: &str, config: Value, case: &str) -> String {
    let body = serde_json::to_vec(&json!({"operationConfig": config})).expect("serializes");
    let answer = call(
        emulator,
        "POST",
        &format!("/v1/training-sessions/{session}/sampling-operations"),
        &body,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the submit must be accepted, got {} {}",
        answer.status,
        answer.text()
    );
    let id = answer.member("operationId");
    assert!(
        !id.is_empty(),
        "case {case:?}: the submit must mint an operation id, got {}",
        answer.text()
    );
    id
}

/// Poll `id` until it reaches a terminal status, and answer that poll.
async fn poll_to_terminal(emulator: &Emulator, session: &str, id: &str, case: &str) -> Answer {
    for _ in 0..POLL_BUDGET {
        let answer = call(
            emulator,
            "GET",
            &format!("/v1/training-sessions/{session}/sampling-operations/{id}"),
            &[],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a poll of a stored operation must answer HTTP 200, got {} {}",
            answer.status,
            answer.text()
        );
        // "Poll this operation until the status reaches a terminal state (Completed,
        // Failed, or Skipped)" — the wait is for the CONDITION, not for a duration.
        if matches!(answer.member("status").as_str(), "Completed" | "Failed" | "Skipped") {
            return answer;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the operation reached no terminal status within the poll budget");
}

/// A `prompt` of the given chunks, each carrying its token ids.
fn prompt(chunks: &[Vec<i32>]) -> Value {
    let chunks: Vec<Value> = chunks
        .iter()
        .map(|tokens| json!({"tokens": tokens, "type": "encoded_text"}))
        .collect();
    json!({"chunks": chunks})
}

/// A `computeLogprobs` configuration over `chunks`.
fn logprobs_config(chunks: &[Vec<i32>]) -> Value {
    json!({"computeLogprobs": {"prompt": prompt(chunks)}})
}

/// The `logprobs` list a served `computeLogprobs` result carries, as it arrived on the
/// wire: `None` for a JSON `null` entry, so the absence of a score is distinguishable from
/// a score of zero.
fn logprobs(body: &Value) -> Vec<Option<f64>> {
    body["operationResult"]["computeLogprobs"]["logprobs"]
        .as_array()
        .map(|entries| entries.iter().map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// RULE: a `computeLogprobs` operation submitted against a session the control plane holds
/// is SERVED — it completes, reports the `computeLogprobs` member of the result union with
/// one score per prompt token across all chunks, leaves the first token unscored because it
/// has no context, and gives every later token a finite non-positive log-probability.
#[tokio::test]
async fn a_served_compute_logprobs_scores_every_prompt_token() {
    let emulator = Emulator::new();
    let session = "scored";
    emulator.create_session(session);

    // The rows differ along what the rule generalises over: how many chunks the prompt
    // carries, how many tokens each chunk carries, and how many tokens there are in all.
    let rows: Vec<(&str, Vec<Vec<i32>>)> = vec![
        ("a single prompt token", vec![vec![464]]),
        ("one chunk of six tokens", vec![vec![464, 3290, 4062, 318, 257, 1332]]),
        ("two chunks of equal length", vec![vec![464, 3290], vec![4062, 318]]),
        (
            "three chunks of differing lengths",
            vec![vec![1], vec![2, 3, 4], vec![5, 6]],
        ),
        ("a hundred tokens in one chunk", vec![(1..=100).collect()]),
        (
            "a hundred tokens spread over ten chunks",
            (0..10).map(|chunk| ((chunk * 10 + 1)..=(chunk * 10 + 10)).collect()).collect(),
        ),
    ];

    for (case, chunks) in rows {
        let tokens: usize = chunks.iter().map(Vec::len).sum();
        let id = submit(&emulator, session, logprobs_config(&chunks), case).await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let body = answer.json();
        assert_eq!(
            answer.member("status"),
            "Completed",
            "case {case:?}: a sampling operation the emulator serves must complete, got {body}"
        );
        assert!(
            body.get("errorDetails").is_none(),
            "case {case:?}: a completed operation reports no errorDetails, got {body}"
        );
        assert!(
            body["operationResult"].get("computeLogprobs").is_some(),
            "case {case:?}: the result must carry the member that was submitted, got {body}"
        );
        let scores = logprobs(&body);
        assert_eq!(
            scores.len(),
            tokens,
            "case {case:?}: expected one score per {tokens} prompt token(s), got {} in {body}",
            scores.len()
        );
        assert_eq!(
            scores.first().copied(),
            Some(None),
            "case {case:?}: the first token has no context, so it carries no score, got {body}"
        );
        for (index, score) in scores.iter().enumerate().skip(1) {
            let Some(value) = score else {
                panic!("case {case:?}: token {index} must carry a score, got {body}");
            };
            assert!(
                value.is_finite() && *value <= 0.0,
                "case {case:?}: token {index} must carry a finite log-probability no greater \
                 than zero, got {value}"
            );
        }
        assert_eq!(
            scores.iter().skip(1).any(|score| score.is_some_and(|value| value < 0.0)),
            tokens > 1,
            "case {case:?}: every token after the first carries a strictly negative \
             log-probability, so a prompt of {tokens} token(s) must have {}, got {scores:?}",
            if tokens > 1 { "one" } else { "none" }
        );
    }
}

/// RULE: a prompt is "the ordered list of encoded text chunks forming the input sequence",
/// so how the caller split that sequence is framing and not content: the same token
/// sequence scores the same however many chunks it arrived in.
#[tokio::test]
async fn the_chunking_of_a_prompt_does_not_change_its_scores() {
    let emulator = Emulator::new();
    let session = "chunked";
    emulator.create_session(session);

    let sequence: Vec<i32> = vec![464, 3290, 4062, 318, 257, 1332, 329, 262, 2746];
    // The same nine tokens, split every way this rule generalises over: whole, in two
    // halves, a token at a time, and in uneven pieces.
    let splittings: Vec<(&str, Vec<Vec<i32>>)> = vec![
        ("one chunk", vec![sequence.clone()]),
        (
            "two chunks",
            vec![sequence[..4].to_vec(), sequence[4..].to_vec()],
        ),
        (
            "three uneven chunks",
            vec![
                sequence[..1].to_vec(),
                sequence[1..7].to_vec(),
                sequence[7..].to_vec(),
            ],
        ),
        (
            "one chunk per token",
            sequence.iter().map(|token| vec![*token]).collect(),
        ),
    ];

    let mut baseline: Option<Vec<Option<f64>>> = None;
    for (case, chunks) in splittings {
        let id = submit(&emulator, session, logprobs_config(&chunks), case).await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let scores = logprobs(&answer.json());
        assert_eq!(
            scores.len(),
            sequence.len(),
            "case {case:?}: expected one score per {} token(s), got {}",
            sequence.len(),
            scores.len()
        );
        match &baseline {
            None => baseline = Some(scores),
            Some(expected) => assert_eq!(
                &scores, expected,
                "case {case:?}: the same token sequence must score the same however it is \
                 chunked, expected {expected:?}, got {scores:?}"
            ),
        }
    }
}

/// RULE: the result is reported from stored state, so two polls of one settled operation
/// answer the same thing — a read reports, it does not recompute.
#[tokio::test]
async fn two_polls_of_a_settled_compute_logprobs_agree() {
    let emulator = Emulator::new();
    let session = "stable";
    emulator.create_session(session);

    let case = "two polls of one operation";
    let id = submit(
        &emulator,
        session,
        logprobs_config(&[vec![464, 3290, 4062]]),
        case,
    )
    .await;
    let first = poll_to_terminal(&emulator, session, &id, case).await;
    let second = poll_to_terminal(&emulator, session, &id, case).await;
    assert_eq!(
        first.json(),
        second.json(),
        "case {case:?}: two polls of a settled operation must answer the same, got {} then {}",
        first.text(),
        second.text()
    );
}

/// RULE: `modelPackageArn` names the checkpoint the operation runs against — "when absent,
/// the operation runs against the session's latest checkpoint" — so a model package the
/// session does not hold is the operation GENUINELY failing, which is the only thing
/// `errorDetails` reports.
#[tokio::test]
async fn compute_logprobs_runs_against_a_checkpoint_the_session_holds() {
    let emulator = Emulator::new();
    let session = "checkpointed";
    emulator.create_session(session);

    for (case, arn, completes) in [
        (
            "a model package of another session",
            Some(format!(
                "arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/1"
            )),
            false,
        ),
        ("no model package at all", None, true),
    ] {
        let mut config = json!({"prompt": prompt(&[vec![464, 3290, 4062]])});
        if let Some(arn) = arn.as_deref() {
            config["modelPackageArn"] = json!(arn);
        }
        let id = submit(&emulator, session, json!({"computeLogprobs": config}), case).await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let body = answer.json();
        if completes {
            assert_eq!(
                answer.member("status"),
                "Completed",
                "case {case:?}: an operation running against the session's own checkpoint \
                 is served, got {body}"
            );
            assert!(
                !logprobs(&body).is_empty(),
                "case {case:?}: the served result carries the prompt's scores, got {body}"
            );
        } else {
            assert_eq!(
                answer.member("status"),
                "Failed",
                "case {case:?}: an operation naming a model package the session does not \
                 hold fails, got {body}"
            );
            assert!(
                body.get("operationResult").is_none(),
                "case {case:?}: a failed operation reports no result, got {body}"
            );
            let details = answer.member("errorDetails");
            assert!(
                arn.as_deref().is_some_and(|arn| details.contains(arn)),
                "case {case:?}: the failure must name the checkpoint it could not find, \
                 got {body}"
            );
        }
    }
}
