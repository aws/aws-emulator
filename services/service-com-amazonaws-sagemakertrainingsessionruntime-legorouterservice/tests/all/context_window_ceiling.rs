// SPDX-License-Identifier: CC0-1.0

//! What a sampling operation whose input does not fit the base model does, end to end over
//! the wire.
//!
//! RULE: a sampling operation is served by the model the training session was created
//! around, and a model attends over a bounded number of tokens at once; an operation whose
//! prompt plus whatever it asks the model to generate needs more than that base model's
//! context window is accepted at submit — the submit mints an operation id like any other
//! — and then settles in the terminal status `Failed` with the reason the service states
//! for that condition and NO `operationResult`, which `GetSamplingOperationResult`
//! populates "only when the status is Completed". The status and the reason come from
//! stored state, so every later read of that operation id reports them again, and an input
//! that does fit the window is served as usual.
//!
//! The rows span what the rule generalises over rather than one recorded call: the sum at
//! the window exactly, one token past it and far past it; the prompt alone over the window
//! and the requested `maxTokens` alone over it; a prompt spread over several chunks, since
//! what counts is the tokens and not the framing; both operation members that carry a
//! prompt (`sample`, which also occupies the window with what it generates, and
//! `computeLogprobs`, which generates nothing); and a session whose base model this
//! emulator publishes no window for, where nothing is compared.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::state;
use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService;

/// The Region every call here signs for.
const REGION: &str = "us-west-2";

/// How many polls a terminal status is waited for, and how long each one waits. Far above
/// the service's own execution span, so a loaded host still reaches the condition.
const POLL_BUDGET: u32 = 400;
/// See [`POLL_BUDGET`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A public-hub base model of the family this emulator publishes a context window for, and
/// one of a family it does not. A training session names its base model as
/// `TrainingSessionModelConfig$BaseModelArn`, whose resource path carries the model name.
fn windowed_base_model() -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
         openai-reasoning-gpt-oss-120b/1.0.0"
    )
}

/// See [`windowed_base_model`].
fn unwindowed_base_model() -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
         meta-textgeneration-llama-3-8b/1.0.0"
    )
}

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

    /// Register `name` with the base model `base_model_arn`, the way the SageMaker control
    /// plane's `CreateTrainingSession` does.
    fn create_session(&self, name: &str, base_model_arn: &str) {
        self.engine
            .state::<service_common::training_sessions::TrainingSessionDirectory>()
            .lock()
            .register(service_common::training_sessions::TrainingSessionRecord {
                name: name.to_string(),
                arn: engine::emulator_arn("sagemaker", REGION, &format!("training-session/{name}")),
                lifecycle: service_common::training_sessions::TrainingSessionLifecycle::settled(
                    "Running",
                ),
                base_model_arn: base_model_arn.to_string(),
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

/// Submit one sampling operation and answer the operation id it minted: an input the model
/// cannot hold is still ACCEPTED, so every row submits the same way.
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
        if matches!(answer.member("status").as_str(), "Completed" | "Failed" | "Skipped") {
            return answer;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the operation reached no terminal status within the poll budget");
}

/// A prompt of `tokens` token ids spread over `chunks` chunks, as `ModelInput` carries it.
///
/// The ids themselves are irrelevant to this rule — what is compared is how MANY there are
/// — so they are a cheap ascending cycle inside the vocabulary.
fn prompt_of(tokens: usize, chunks: usize) -> Value {
    let chunks = chunks.max(1);
    let mut built: Vec<Vec<i32>> = vec![Vec::new(); chunks];
    for index in 0..tokens {
        built[index % chunks].push((index % 50_000) as i32 + 1);
    }
    let chunks: Vec<Value> = built
        .into_iter()
        .filter(|tokens| !tokens.is_empty())
        .map(|tokens| json!({"tokens": tokens, "type": "encoded_text"}))
        .collect();
    json!({"chunks": chunks})
}

/// A `sample` configuration whose prompt carries `tokens` tokens over `chunks` chunks and
/// which asks for `max_tokens` generated ones.
fn sample_config(tokens: usize, chunks: usize, max_tokens: i64) -> Value {
    json!({"sample": {
        "prompt": prompt_of(tokens, chunks),
        "numSamples": 1,
        "samplingParams": {"maxTokens": max_tokens},
    }})
}

/// The reason a sampling operation reports for an input the base model cannot hold, from
/// the one place this service words it.
fn over_context_reason() -> String {
    state::context_length_exceeded()
}

/// One row of the ceiling table.
struct Row {
    case: &'static str,
    /// How many prompt tokens are submitted, over how many chunks.
    tokens: usize,
    chunks: usize,
    /// `samplingParams.maxTokens`, inside its modelled `@range(min: 512, max: 131072)`.
    max_tokens: i64,
    /// Whether the operation must settle `Failed` with the context-length reason.
    over_window: bool,
}

/// The context window the emulator publishes for the base model the rows below name,
/// which is what their sums are built around.
fn window() -> i64 {
    i64::from(
        state::base_model_context_window(&windowed_base_model())
            .expect("the suite's base model has a published context window"),
    )
}

/// RULE: a `sample` whose prompt plus its requested `maxTokens` exceeds the base model's
/// context window settles `Failed` with the context-length reason and no result, while a
/// sum the window admits is served.
#[tokio::test]
async fn a_sample_over_the_context_window_settles_failed() {
    let emulator = Emulator::new();
    let session = "windowed";
    emulator.create_session(session, &windowed_base_model());
    let window = window();
    let lowest_max_tokens = 512;

    // The rows walk the sum across the window: at it, one past it, far past it, and the
    // two ways the sum gets there (a long prompt, or a large request for generated
    // tokens). The published bounds of `maxTokens` are 512 and 131072.
    let rows = vec![
        Row {
            case: "a short prompt well inside the window",
            tokens: 6,
            chunks: 1,
            max_tokens: lowest_max_tokens,
            over_window: false,
        },
        Row {
            case: "a sum at the window exactly",
            tokens: (window - lowest_max_tokens) as usize,
            chunks: 1,
            max_tokens: lowest_max_tokens,
            over_window: false,
        },
        Row {
            case: "a sum one token past the window",
            tokens: (window - lowest_max_tokens + 1) as usize,
            chunks: 1,
            max_tokens: lowest_max_tokens,
            over_window: true,
        },
        Row {
            case: "a prompt far past the window on its own",
            tokens: (window + 4_464) as usize,
            chunks: 1,
            max_tokens: lowest_max_tokens,
            over_window: true,
        },
        Row {
            case: "a prompt past the window spread over several chunks",
            tokens: (window + 1) as usize,
            chunks: 7,
            max_tokens: lowest_max_tokens,
            over_window: true,
        },
        Row {
            case: "the highest maxTokens asked of this window",
            tokens: 6,
            chunks: 1,
            max_tokens: 131_072,
            over_window: true,
        },
    ];

    for row in rows {
        let Row {
            case,
            tokens,
            chunks,
            max_tokens,
            over_window,
        } = row;
        let id = submit(
            &emulator,
            session,
            sample_config(tokens, chunks, max_tokens),
            case,
        )
        .await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let body = answer.json();
        if over_window {
            assert_eq!(
                (answer.member("status"), answer.member("errorDetails")),
                ("Failed".to_string(), over_context_reason()),
                "case {case:?}: {tokens} prompt token(s) plus {max_tokens} requested of a \
                 {window}-token window must fail with the context-length reason, got {body}"
            );
            assert!(
                body.get("operationResult").is_none(),
                "case {case:?}: an operation the model could not process reports no result, \
                 got {body}"
            );
            // The status and the reason are stored, not recomputed: a second read of the
            // same operation id answers the same.
            let reread = poll_to_terminal(&emulator, session, &id, case).await;
            assert_eq!(
                reread.json(),
                body,
                "case {case:?}: every later read must report the same failure, got {}",
                reread.text()
            );
        } else {
            assert_eq!(
                answer.member("status"),
                "Completed",
                "case {case:?}: {tokens} prompt token(s) plus {max_tokens} requested fit the \
                 {window}-token window, so the operation is served, got {}",
                answer.member("errorDetails")
            );
            assert!(
                body["operationResult"].get("sample").is_some(),
                "case {case:?}: a served sample carries its result, got {body}"
            );
        }
    }
}

/// RULE: the same ceiling holds for every sampling member that submits a prompt, and what
/// it compares is what that member puts through the model — `computeLogprobs` scores the
/// prompt "without generating new text", so only the prompt occupies the window.
#[tokio::test]
async fn compute_logprobs_over_the_context_window_settles_failed() {
    let emulator = Emulator::new();
    let session = "scored";
    emulator.create_session(session, &windowed_base_model());
    let window = window();

    // (case, prompt tokens, chunks, over the window)
    let rows: Vec<(&str, usize, usize, bool)> = vec![
        ("a prompt at the window exactly", window as usize, 1, false),
        ("a prompt one token past the window", window as usize + 1, 1, true),
        (
            "a prompt past the window over several chunks",
            window as usize + 9,
            3,
            true,
        ),
    ];

    for (case, tokens, chunks, over_window) in rows {
        let config = json!({"computeLogprobs": {"prompt": prompt_of(tokens, chunks)}});
        let id = submit(&emulator, session, config, case).await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        let body = answer.json();
        if over_window {
            assert_eq!(
                (answer.member("status"), answer.member("errorDetails")),
                ("Failed".to_string(), over_context_reason()),
                "case {case:?}: {tokens} prompt token(s) over a {window}-token window must \
                 fail with the context-length reason, got {body}"
            );
            assert!(
                body.get("operationResult").is_none(),
                "case {case:?}: an operation the model could not process reports no result, \
                 got {body}"
            );
        } else {
            assert_eq!(
                answer.member("status"),
                "Completed",
                "case {case:?}: a prompt the window admits is scored, got {body}"
            );
            assert_eq!(
                body["operationResult"]["computeLogprobs"]["logprobs"]
                    .as_array()
                    .map(Vec::len),
                Some(tokens),
                "case {case:?}: expected one score per prompt token"
            );
        }
    }
}

/// RULE: the window compared against is the BASE MODEL's, so a session whose base model
/// this emulator publishes no window for has no ceiling applied — the same request that
/// fails against a windowed base model is served there.
#[tokio::test]
async fn a_base_model_with_no_published_window_has_no_ceiling() {
    let emulator = Emulator::new();
    emulator.create_session("windowed", &windowed_base_model());
    emulator.create_session("unwindowed", &unwindowed_base_model());

    // One request, over the window the `gpt-oss` family is served with, against two
    // sessions that differ only in the base model they adapt.
    let tokens = (window() + 1) as usize;
    for (case, session, expected) in [
        ("a base model with a published window", "windowed", "Failed"),
        (
            "a base model this emulator publishes no window for",
            "unwindowed",
            "Completed",
        ),
    ] {
        let id = submit(&emulator, session, sample_config(tokens, 1, 512), case).await;
        let answer = poll_to_terminal(&emulator, session, &id, case).await;
        assert_eq!(
            answer.member("status"),
            expected,
            "case {case:?}: expected {expected}, got {}",
            answer.text()
        );
    }
}

/// RULE: the window is a published property of the base model family an ARN names, read
/// off that ARN and not off the request — so every model of a published family resolves to
/// the same window, whatever the case or the version the ARN carries, and a family with no
/// published window resolves to none.
#[test]
fn the_published_window_is_read_off_the_base_model_arn() {
    let published = state::base_model_context_window(&windowed_base_model());
    assert!(
        published.is_some_and(|window| window > 0),
        "the suite's base model must have a published window, got {published:?}"
    );

    // (case, the base model ARN, whether a window is published for it)
    let rows: Vec<(&str, String, Option<i32>)> = vec![
        (
            "another size of the same family",
            format!(
                "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
                 openai-reasoning-gpt-oss-20b/1.0.0"
            ),
            published,
        ),
        (
            "the same family in another case",
            format!(
                "arn:aws:sagemaker:{REGION}:aws:hub-content/SageMakerPublicHub/Model/\
                 OpenAI-Reasoning-GPT-OSS-20B/2.1.0"
            ),
            published,
        ),
        (
            "a family with no published window",
            unwindowed_base_model(),
            None,
        ),
        ("no base model at all", String::new(), None),
    ];
    for (case, arn, expected) in rows {
        let actual = state::base_model_context_window(&arn);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the window {expected:?} for {arn}, got {actual:?}"
        );
    }
}
