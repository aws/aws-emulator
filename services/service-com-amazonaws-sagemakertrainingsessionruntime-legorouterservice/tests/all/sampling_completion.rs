// SPDX-License-Identifier: CC0-1.0

//! What a `sample` operation of this data plane SERVES, end to end over the wire.
//!
//! RULE: a sampling operation this emulator can execute settles in the terminal status
//! that outcome is — `Completed`, with the `SampleOperationResult` member matching the
//! `operationConfig` member that was submitted — and the modelled failure channel
//! (`Failed` with `errorDetails`) is reserved for an operation that GENUINELY failed, such
//! as one naming a model package the session does not hold. For the `sample` member that
//! means `operationResult.sample.sequences` carries one sequence per `numSamples`, each
//! with a non-empty `tokens`, a `stopReason` from the modelled enum, and a `tokenUsage`
//! whose `prefill` is the number of prompt tokens the request submitted and whose `decode`
//! is the number of tokens served.
//!
//! No language model runs in this emulator, so what those tokens DECODE to says exactly
//! that: the sequence is the byte-pair encoding of a notice plus the prompt's own token
//! count, which is why every case here decodes what it was served and reads it rather than
//! trusting its shape.
//!
//! The rows span what the rule generalises over rather than one recorded call: the number
//! of prompt chunks and tokens in them, `numSamples` at its modelled minimum, in between
//! and at its maximum, `maxTokens` at both published bounds, a token ceiling at and past
//! the length of what is generated, stop sequences the output does reach and does not
//! reach, and a named model package the session holds as well as one it does not.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::state;
use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region every call here signs for.
const REGION: &str = "us-west-2";

/// How many polls a terminal status is waited for, and how long each one waits. Far above
/// the service's own execution span, so a loaded host still reaches the condition: the
/// loop waits for the STATUS, never for a fixed time.
const POLL_BUDGET: u32 = 200;
/// See [`POLL_BUDGET`].
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
                // The session's base model decides the context window its sampling
                // operations are served over; this one is of a family this emulator
                // publishes no window for, so the rows below are about what they say and
                // not about that ceiling.
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

/// A `sample` configuration over `chunks`, asking for `num_samples` samples under a
/// `maxTokens` ceiling and the given stop sequences.
fn sample_config(
    chunks: &[Vec<i32>],
    num_samples: i64,
    max_tokens: i64,
    stop: &[&str],
) -> Value {
    let mut params = json!({"maxTokens": max_tokens, "temperature": 0.7});
    if !stop.is_empty() {
        params["stop"] = json!(stop);
    }
    json!({"sample": {
        "prompt": prompt(chunks),
        "numSamples": num_samples,
        "samplingParams": params,
    }})
}

/// The tokenizer the decoding below reads with, built once: loading a vocabulary is the
/// expensive part and no case may pay it twice.
fn tokenizer() -> &'static tiktoken_rs::CoreBPE {
    static TOKENIZER: std::sync::OnceLock<tiktoken_rs::CoreBPE> = std::sync::OnceLock::new();
    TOKENIZER.get_or_init(|| tiktoken_rs::cl100k_base().expect("the shipped vocabulary is readable"))
}

/// The text `tokens` decode to, read with the same published vocabulary a client of this
/// emulator would: this is what makes a served sequence READABLE rather than opaque ids.
fn decode(tokens: &[i64], case: &str) -> String {
    let ids: Vec<u32> = tokens
        .iter()
        .map(|token| {
            u32::try_from(*token).unwrap_or_else(|_| {
                panic!("case {case:?}: a served token id must be a vocabulary id, got {token}")
            })
        })
        .collect();
    let bytes = tokenizer()
        .decode_bytes(&ids)
        .unwrap_or_else(|error| panic!("case {case:?}: the served tokens must decode, got {error}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The `tokens` of the sequence at `index` of a served `sample` result.
fn sequence_tokens(body: &Value, index: usize) -> Vec<i64> {
    body["operationResult"]["sample"]["sequences"][index]["tokens"]
        .as_array()
        .map(|tokens| tokens.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// One row of the table: a submitted `sample` configuration and the prompt behind it.
struct Row {
    case: &'static str,
    chunks: Vec<Vec<i32>>,
    num_samples: i64,
    max_tokens: i64,
}

/// RULE: a `sample` operation submitted against a session the control plane holds is
/// SERVED — it completes, reports one sequence per `numSamples`, and each sequence carries
/// non-empty tokens that decode to what was generated, a modelled stop reason and the
/// token usage the request really incurred.
#[tokio::test]
async fn a_served_sample_reports_a_sequence_per_sample_with_the_usage_it_incurred() {
    let emulator = Emulator::new();
    let session = "served";
    emulator.create_session(session);

    // The rows differ along the prompt's shape (one chunk, several chunks, a single token,
    // a hundred tokens), `numSamples` at its modelled minimum, in between and at its
    // maximum of 1000, and `maxTokens` at both published bounds of 512..131072
    // (smithy_query(query="shape",shape="SampleConfig,SamplingParams")).
    let rows = vec![
        Row {
            case: "one chunk of five prompt tokens",
            chunks: vec![vec![791, 6864, 315, 9822, 374]],
            num_samples: 1,
            max_tokens: 512,
        },
        Row {
            case: "a single prompt token",
            chunks: vec![vec![1]],
            num_samples: 1,
            max_tokens: 512,
        },
        Row {
            case: "three chunks of differing lengths",
            chunks: vec![vec![1, 2], vec![3], vec![4, 5, 6]],
            num_samples: 3,
            max_tokens: 512,
        },
        Row {
            case: "a hundred prompt tokens at the highest maxTokens",
            chunks: vec![(1..=100).collect()],
            num_samples: 1,
            max_tokens: 131_072,
        },
        Row {
            case: "the highest numSamples",
            chunks: vec![vec![7, 8]],
            num_samples: 1_000,
            max_tokens: 512,
        },
    ];

    for row in rows {
        let case = row.case;
        let prefill: i64 = row
            .chunks
            .iter()
            .map(|chunk| chunk.len() as i64)
            .sum();
        let id = submit(
            &emulator,
            session,
            sample_config(&row.chunks, row.num_samples, row.max_tokens, &[]),
            case,
        )
        .await;
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
        let sequences = body["operationResult"]["sample"]["sequences"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            sequences.len() as i64,
            row.num_samples,
            "case {case:?}: expected one sequence per numSamples ({}), got {}",
            row.num_samples,
            sequences.len()
        );
        for (index, sequence) in sequences.iter().enumerate() {
            let tokens = sequence_tokens(&body, index);
            assert!(
                !tokens.is_empty(),
                "case {case:?} sequence {index}: expected a non-empty token sequence, got \
                 {sequence}"
            );
            let stop_reason = sequence["stopReason"].as_str().unwrap_or_default();
            assert_eq!(
                stop_reason, "stop",
                "case {case:?} sequence {index}: a sequence that finished inside its token \
                 ceiling stops for the `stop` reason, got {stop_reason:?}"
            );
            assert_eq!(
                sequence["tokenUsage"]["prefill"].as_i64(),
                Some(prefill),
                "case {case:?} sequence {index}: expected prefill to be the {prefill} prompt \
                 token(s) submitted, got {sequence}"
            );
            assert_eq!(
                sequence["tokenUsage"]["decode"].as_i64(),
                Some(tokens.len() as i64),
                "case {case:?} sequence {index}: expected decode to be the {} token(s) \
                 served, got {sequence}",
                tokens.len()
            );
            // The ids are a real encoding of real text, so they decode: what they say is
            // that no model ran, and how many prompt tokens were prefilled.
            let served = decode(&tokens, case);
            assert!(
                served.to_lowercase().contains("no language model"),
                "case {case:?} sequence {index}: the served tokens must decode to text that \
                 says no model generated them, got {served:?}"
            );
            assert!(
                served.contains(&format!("{prefill} token(s)")),
                "case {case:?} sequence {index}: the served text must report the {prefill} \
                 prompt token(s) it was given, got {served:?}"
            );
        }
    }
}

/// RULE: `samplingParams.stop` terminates generation where the earliest of the sequences
/// appears in the output, and a stop sequence the output never reaches changes nothing.
#[tokio::test]
async fn a_stop_sequence_the_output_reaches_halts_it_there() {
    let emulator = Emulator::new();
    let session = "stops";
    emulator.create_session(session);
    let chunks = vec![vec![7, 8]];

    let baseline_id = submit(
        &emulator,
        session,
        sample_config(&chunks, 1, 512, &[]),
        "no stop sequence",
    )
    .await;
    let baseline = poll_to_terminal(&emulator, session, &baseline_id, "no stop sequence").await;
    let unhalted = sequence_tokens(&baseline.json(), 0);
    let unhalted_text = decode(&unhalted, "no stop sequence");
    // A stop sequence is only a stop sequence if it is IN what would otherwise be
    // generated; this one is taken from the unhalted output rather than written in.
    let reached = unhalted_text
        .split_whitespace()
        .last()
        .map(str::to_string)
        .expect("the unhalted output carries words");

    for (case, stop, halts) in [
        ("a stop sequence the output reaches", reached.clone(), true),
        (
            "a stop sequence the output never reaches",
            "zqx-never-generated".to_string(),
            false,
        ),
    ] {
        let id = submit(
            &emulator,
            session,
            sample_config(&chunks, 1, 512, &[stop.as_str()]),
            case,
        )
        .await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let body = answer.json();
        assert_eq!(
            answer.member("status"),
            "Completed",
            "case {case:?}: a stop sequence does not fail the operation, got {body}"
        );
        let tokens = sequence_tokens(&body, 0);
        let served = decode(&tokens, case);
        if halts {
            assert!(
                tokens.len() < unhalted.len(),
                "case {case:?}: generation must end at {stop:?}, so fewer than the {} \
                 unhalted token(s) are served, got {}",
                unhalted.len(),
                tokens.len()
            );
            assert!(
                !served.contains(stop.as_str()),
                "case {case:?}: the stop sequence terminates generation and is not part of \
                 what was served, got {served:?}"
            );
        } else {
            assert_eq!(
                tokens, unhalted,
                "case {case:?}: a stop sequence the output never reaches changes nothing, \
                 got {served:?}"
            );
        }
        assert_eq!(
            body["operationResult"]["sample"]["sequences"][0]["tokenUsage"]["decode"].as_i64(),
            Some(tokens.len() as i64),
            "case {case:?}: decode must be the token count actually served, got {body}"
        );
    }
}

/// RULE: the token ceiling is what reports `length` — "A value of `length` indicates the
/// maximum token limit was reached" — and a sequence that fits its ceiling reports `stop`.
///
/// `SamplingParams$maxTokens` is `@range(min: 512)`, so a ceiling tight enough to cut the
/// output cannot reach the wire: the rule is driven against the state function the handler
/// itself uses, at the ceiling exactly, one below it and one above it.
#[test]
fn the_token_ceiling_is_what_reports_length() {
    // The full sequence a prompt of this size is served, with no ceiling in the way.
    let prefill = 5;
    let full = state::emulated_sequence(prefill, i32::MAX, &[])
        .expect("the served sequence encodes");
    let length = i32::try_from(full.tokens.len()).expect("the served sequence is short");
    assert!(
        length > 1,
        "the served sequence must carry more than one token for this table to mean \
         anything, got {length}"
    );

    for (case, ceiling, expected_tokens, expected_reason) in [
        ("a ceiling of zero tokens", 0, 0, "length"),
        ("a ceiling of one token", 1, 1, "length"),
        (
            "a ceiling one token below what is generated",
            length - 1,
            length - 1,
            "length",
        ),
        (
            "a ceiling at exactly what is generated",
            length,
            length,
            "stop",
        ),
        (
            "a ceiling one token above what is generated",
            length + 1,
            length,
            "stop",
        ),
        (
            "the published minimum ceiling",
            512,
            length,
            "stop",
        ),
    ] {
        let sampled = state::emulated_sequence(prefill, ceiling, &[])
            .unwrap_or_else(|| panic!("case {case:?}: the served sequence encodes"));
        assert_eq!(
            i32::try_from(sampled.tokens.len()).unwrap_or(i32::MAX),
            expected_tokens,
            "case {case:?}: expected {expected_tokens} token(s) under a ceiling of \
             {ceiling}, got {}",
            sampled.tokens.len()
        );
        assert_eq!(
            sampled.stop_reason, expected_reason,
            "case {case:?}: expected the stop reason {expected_reason:?} under a ceiling of \
             {ceiling}, got {:?}",
            sampled.stop_reason
        );
        assert!(
            sampled.tokens.iter().all(|token| *token >= 0),
            "case {case:?}: every served id is a vocabulary id, got {:?}",
            sampled.tokens
        );
    }
}

/// RULE: `modelPackageArn` names the checkpoint the sample runs against and "must be a
/// valid SageMaker model package ARN"; one the session holds is served, and one it does
/// not hold is the operation GENUINELY failing — which is the only thing `errorDetails`
/// reports.
#[tokio::test]
async fn a_sample_runs_against_a_checkpoint_the_session_holds() {
    let emulator = Emulator::new();
    let session = "checkpointed";
    emulator.create_session(session);

    // Written by a real training operation, so the ARN the rows name is the session's own
    // rather than one the test made up.
    let body = serde_json::to_vec(&json!({
        "sequenceNumber": 1,
        "operationConfig": {"saveWeights": {"mergeWeights": false}},
    }))
    .expect("serializes");
    let submit_answer = call(
        &emulator,
        "POST",
        &format!("/v1/training-sessions/{session}/operations"),
        &body,
    )
    .await;
    assert_eq!(
        submit_answer.status,
        200,
        "the checkpoint submit must be accepted, got {} {}",
        submit_answer.status,
        submit_answer.text()
    );
    let save_id = submit_answer.member("operationId");
    let mut held = String::new();
    for _ in 0..POLL_BUDGET {
        let answer = call(
            &emulator,
            "GET",
            &format!("/v1/training-sessions/{session}/operations/{save_id}"),
            &[],
        )
        .await;
        if answer.member("status") == "Completed" {
            held = answer.json()["operationResult"]["saveWeights"]["modelPackageArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        !held.is_empty(),
        "the save operation must publish a checkpoint for this table to name one"
    );

    for (case, arn, completes) in [
        ("the checkpoint the session wrote", held.clone(), true),
        (
            "a checkpoint of another session",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:model-package/elsewhere/1"),
            false,
        ),
    ] {
        let config = json!({"sample": {
            "prompt": prompt(&[vec![7, 8]]),
            "numSamples": 1,
            "samplingParams": {"maxTokens": 512},
            "modelPackageArn": arn,
        }});
        let id = submit(&emulator, session, config, case).await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let body = answer.json();
        if completes {
            assert_eq!(
                answer.member("status"),
                "Completed",
                "case {case:?}: a sample against a checkpoint the session holds is served, \
                 got {body}"
            );
            assert!(
                !sequence_tokens(&body, 0).is_empty(),
                "case {case:?}: the served sequence carries tokens, got {body}"
            );
        } else {
            assert_eq!(
                answer.member("status"),
                "Failed",
                "case {case:?}: a sample naming a checkpoint the session does not hold \
                 fails, got {body}"
            );
            assert!(
                body.get("operationResult").is_none(),
                "case {case:?}: a failed operation reports no result, got {body}"
            );
            assert!(
                answer.member("errorDetails").contains(&arn),
                "case {case:?}: the failure must name the checkpoint it could not find, \
                 got {body}"
            );
        }
    }
}

/// RULE: the result is reported from stored state, so two polls of one operation agree and
/// two identical submits are served identically — "the same seed with the same input
/// produces deterministic outputs".
#[tokio::test]
async fn the_served_result_comes_from_state_and_two_identical_submits_agree() {
    let emulator = Emulator::new();
    let session = "deterministic";
    emulator.create_session(session);
    let chunks = vec![vec![791, 6864, 315]];

    let mut served = Vec::new();
    for case in ["the first submit", "the second submit"] {
        let config = json!({"sample": {
            "prompt": prompt(&chunks),
            "numSamples": 1,
            "samplingParams": {"maxTokens": 512, "seed": 42},
        }});
        let id = submit(&emulator, session, config, case).await;
        let first_poll = poll_to_terminal(&emulator, session, &id, case).await;
        let second_poll = poll_to_terminal(&emulator, session, &id, case).await;
        assert_eq!(
            first_poll.json(),
            second_poll.json(),
            "case {case:?}: two polls of a settled operation must answer the same, got {} \
             then {}",
            first_poll.text(),
            second_poll.text()
        );
        served.push(sequence_tokens(&first_poll.json(), 0));
    }
    assert_eq!(
        served[0], served[1],
        "case two identical submits: the same input must be served the same tokens, got \
         {:?} then {:?}",
        served[0], served[1]
    );
}
