// SPDX-License-Identifier: CC0-1.0

//! The input a Bedrock Runtime inference request may carry, and what a finished
//! asynchronous invocation delivers — end to end over HTTP.
//!
//! Three rules are asserted here, each over a SET of inputs rather than over the one
//! request that showed it:
//!
//! * a Converse-API `Message` whose `content` list is EMPTY carries nothing for the model
//!   to read, so the turn is refused `400 ValidationException` naming that message's own
//!   index in the submitted `messages` list;
//! * a model reads a BOUNDED prompt, so an input whose counted tokens are past the invoked
//!   model's context window is refused `400 ValidationException` — never served against a
//!   prompt the service shortened — on the invoke surfaces and on the Converse API, which
//!   states the same sentence as the model-side error it is;
//! * an accepted asynchronous invocation reaches the modelled `Completed` status by
//!   WRITING its output under the location the submission configured, so a read that
//!   reports it finished is a read whose output Amazon S3 itself already holds; one whose
//!   configured location cannot be written ends in `Failed` carrying the reason.
//!
//! The first two drive the harness the other suites of this target drive. The third mounts
//! Amazon S3 beside this service on one engine — the way the assembled emulator runs them
//! — because the output an invocation delivers is an object of that store, read back
//! through Amazon S3's own `GetObject`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice as bedrock;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_s3_amazons3 as s3;
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_runtime_inference::{emulator, encode_path, get, post_json, send, Call};

/// The region every request below is signed for: the bedrock harness signs `us-east-1`,
/// and the object store is addressed in the region of the request that writes it.
const REGION: &str = "us-east-1";

/// The sentence Amazon Bedrock refuses an input past the model's context window with, as
/// the recorded `InvokeModel` refusal carries it. Stated as a literal here so the WORDING
/// is pinned by this test and not merely by the service agreeing with itself.
const INPUT_TOKENS_EXCEEDED: &str = "Input Tokens Exceeded: Number of input tokens exceeds \
                                     maximum length. Please update the input to try again.";

/// What the Converse API puts in front of that sentence, which is how it reports an error
/// the model raised for a turn it was asked to serve.
const MODEL_ERRORS_PREFIX: &str = "The model returned the following errors: ";

/// The bucket every asynchronous invocation below writes into.
const OUTPUT_BUCKET: &str = "bedrock-async-out";

/// A bucket no test below ever creates, so an invocation configured to write there has
/// nowhere to deliver its output.
const ABSENT_BUCKET: &str = "bedrock-async-absent";

// ---------------------------------------------------------------------------
// Cause: a Message whose content list is empty
// ---------------------------------------------------------------------------

/// A model the Converse API serves, for the conversation-shape rows.
const CONVERSE_MODEL: &str = "amazon.nova-lite-v1:0";

/// One conversation and what the empty-content rule makes of it: `Some(index)` for the
/// message the refusal must name, `None` for a conversation every turn of which carries
/// content.
struct ConversationRow {
    case: &'static str,
    messages: Value,
    empty_at: Option<usize>,
}

/// The conversation table: rows differ along WHICH turn carries no content (the only one,
/// a later one, the last of three), how many do, and the controls whose every turn does —
/// including one whose turns carry several blocks, so "has content" is not read as "has
/// exactly one block".
fn conversation_rows() -> Vec<ConversationRow> {
    vec![
        ConversationRow {
            case: "the only turn carries no content",
            messages: json!([{"role": "user", "content": []}]),
            empty_at: Some(0),
        },
        ConversationRow {
            case: "the second turn of two carries no content",
            messages: json!([
                {"role": "user", "content": [{"text": "What is a page token?"}]},
                {"role": "assistant", "content": []},
            ]),
            empty_at: Some(1),
        },
        ConversationRow {
            case: "the third turn of three carries no content",
            messages: json!([
                {"role": "user", "content": [{"text": "What is a page token?"}]},
                {"role": "assistant", "content": [{"text": "An opaque cursor."}]},
                {"role": "user", "content": []},
            ]),
            empty_at: Some(2),
        },
        ConversationRow {
            case: "two turns carry no content",
            messages: json!([
                {"role": "user", "content": []},
                {"role": "user", "content": []},
            ]),
            empty_at: Some(0),
        },
        ConversationRow {
            case: "one turn with content",
            messages: json!([{"role": "user", "content": [{"text": "Name one planet."}]}]),
            empty_at: None,
        },
        ConversationRow {
            case: "three turns, each with content",
            messages: json!([
                {"role": "user", "content": [{"text": "What is a page token?"}]},
                {"role": "assistant", "content": [{"text": "An opaque cursor."}]},
                {"role": "user", "content": [{"text": "Why opaque?"}]},
            ]),
            empty_at: None,
        },
        ConversationRow {
            case: "several content blocks in one turn",
            messages: json!([{
                "role": "user",
                "content": [{"text": "List three primes."}, {"text": "Then explain why."}],
            }]),
            empty_at: None,
        },
    ]
}

/// RULE: a Converse-API `Message` whose `content` list is empty carries no content block
/// for that turn, so the request is refused `400 ValidationException` naming the ZERO-BASED
/// index of the offending message in the submitted `messages` list — on both surfaces that
/// bind the shape, and with the FIRST offending message the one named.
#[tokio::test]
async fn the_converse_surfaces_refuse_a_message_whose_content_list_is_empty() {
    let router = emulator();

    for row in conversation_rows() {
        let case = row.case;
        for surface in ["converse", "converse-stream"] {
            let call = post_json(
                &router,
                &format!("/model/{CONVERSE_MODEL}/{surface}"),
                &json!({ "messages": row.messages }),
            )
            .await;
            match row.empty_at {
                Some(index) => {
                    assert_eq!(
                        call.status,
                        400,
                        "case {case:?} on {surface}: expected 400, got {} / {}",
                        call.status,
                        String::from_utf8_lossy(&call.body)
                    );
                    assert_eq!(
                        call.error_code(),
                        "ValidationException",
                        "case {case:?} on {surface}: expected ValidationException, got {:?}",
                        call.error_code()
                    );
                    let stated = message_of(&call);
                    let expected = format!(
                        "The content field in the Message object at messages.{index} is empty. \
                         Add a ContentBlock object to the content field and try again."
                    );
                    assert_eq!(
                        stated, expected,
                        "case {case:?} on {surface}: expected {expected:?}, got {stated:?}"
                    );
                }
                None => assert_eq!(
                    call.status,
                    200,
                    "case {case:?} on {surface}: expected the turn to be served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cause: an input past the model's context window
// ---------------------------------------------------------------------------

/// One catalogued model, the context window published for it, and how its own
/// `InvokeModel` request body carries a prompt.
///
/// The windows are the ones the catalogue carries from "Supported foundation models in
/// Amazon Bedrock" (the context-window column): Titan Text Lite 4K, Cohere Command (text)
/// v14 4K, Llama 3 8K and Nova Lite 300K. The rows differ along the family's request
/// format AND along the size of the window, so no row is the rule on its own, and three of
/// the four are models the report that found this never named.
struct WindowRow {
    case: &'static str,
    model_id: &'static str,
    context_window: usize,
    /// The family's own body carrying `prompt` as its prompt text.
    body: fn(&str) -> Value,
}

fn window_rows() -> Vec<WindowRow> {
    vec![
        WindowRow {
            case: "Titan Text Lite",
            model_id: "amazon.titan-text-lite-v1",
            context_window: 4_000,
            body: |prompt| json!({"inputText": prompt}),
        },
        WindowRow {
            case: "Cohere Command text",
            model_id: "cohere.command-text-v14",
            context_window: 4_000,
            body: |prompt| json!({"prompt": prompt, "max_tokens": 16}),
        },
        WindowRow {
            case: "Llama 3",
            model_id: "meta.llama3-8b-instruct-v1:0",
            context_window: 8_000,
            body: |prompt| json!({"prompt": prompt, "max_gen_len": 16}),
        },
        WindowRow {
            case: "Nova Lite",
            model_id: "amazon.nova-lite-v1:0",
            context_window: 300_000,
            body: |prompt| json!({
                "messages": [{"role": "user", "content": [{"text": prompt}]}],
                "inferenceConfig": {"maxTokens": 16},
            }),
        },
    ]
}

/// A prompt of exactly `tokens` tokens: this emulator's tokenizer makes one token of a run
/// of at most [`state::MAX_CHARS_PER_TOKEN`] alphanumerics, so one four-character word per
/// token counts exactly.
fn prompt_of_tokens(tokens: usize) -> String {
    let built = "word ".repeat(tokens);
    assert_eq!(
        state::count_text_tokens(&built) as usize,
        tokens,
        "the test's own prompt builder must produce exactly {tokens} tokens"
    );
    built
}

/// RULE: a model reads a bounded prompt, so an `InvokeModel` body whose counted prompt
/// tokens are past the invoked model's published context window is refused
/// `400 ValidationException` with the model-side sentence, while a body AT the window —
/// and any body under it — is served. Nothing shortens a prompt to fit: the only answer
/// to an oversized input is the refusal.
#[tokio::test]
async fn invoke_model_refuses_an_input_past_the_models_context_window() {
    let router = emulator();

    for row in window_rows() {
        // The published bound, one token past it, and a prompt far inside it.
        let cases: [(&str, usize, bool); 3] = [
            ("one token", 1, true),
            ("at the context window", row.context_window, true),
            ("one token past the context window", row.context_window + 1, false),
        ];
        for (size, tokens, served) in cases {
            let case = format!("{} / {size}", row.case);
            let body = (row.body)(&prompt_of_tokens(tokens));
            let call = send(
                &router,
                "POST",
                &format!("/model/{}/invoke", encode_path(row.model_id)),
                &[("content-type", "application/json")],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await;
            if served {
                assert_eq!(
                    call.status,
                    200,
                    "case {case:?}: expected the input inside the window to be served, got {} \
                     / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                continue;
            }
            assert_eq!(
                call.status,
                400,
                "case {case:?}: expected 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                call.error_code()
            );
            let stated = message_of(&call);
            assert_eq!(
                stated, INPUT_TOKENS_EXCEEDED,
                "case {case:?}: expected {INPUT_TOKENS_EXCEEDED:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: the Converse API applies the same bound to the conversation it is handed, and
/// reports it as the model-side error it is: the refusal is `400 ValidationException`
/// whose message is the model-errors prefix followed by that same sentence, on both
/// surfaces that bind the shape.
#[tokio::test]
async fn the_converse_surfaces_refuse_an_input_past_the_models_context_window() {
    let router = emulator();
    let expected = format!("{MODEL_ERRORS_PREFIX}{INPUT_TOKENS_EXCEEDED}");

    // The Converse-API rows: the two catalogued models with the smallest published windows
    // that the Converse API serves, at the bound and one token past it.
    let rows: [(&str, usize); 2] = [
        ("amazon.titan-text-lite-v1", 4_000),
        ("meta.llama3-8b-instruct-v1:0", 8_000),
    ];
    for (model_id, context_window) in rows {
        for (size, tokens, served) in [
            ("at the context window", context_window, true),
            ("one token past the context window", context_window + 1, false),
        ] {
            let prompt = prompt_of_tokens(tokens);
            for surface in ["converse", "converse-stream"] {
                let case = format!("{model_id} / {size} / {surface}");
                let call = post_json(
                    &router,
                    &format!("/model/{}/{surface}", encode_path(model_id)),
                    &json!({"messages": [{"role": "user", "content": [{"text": prompt}]}]}),
                )
                .await;
                if served {
                    assert_eq!(
                        call.status,
                        200,
                        "case {case:?}: expected the input inside the window to be served, got \
                         {} / {}",
                        call.status,
                        String::from_utf8_lossy(&call.body)
                    );
                    continue;
                }
                assert_eq!(
                    call.status,
                    400,
                    "case {case:?}: expected 400, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                assert_eq!(
                    call.error_code(),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    call.error_code()
                );
                let stated = message_of(&call);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected {expected:?}, got {stated:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cause: an accepted asynchronous invocation never reaches Completed
// ---------------------------------------------------------------------------

/// One asynchronous invocation: the model it names, the `modelInput` that model's own
/// published schema takes, the output location it configures, and where its output then
/// has to be.
struct AsyncRow {
    case: &'static str,
    model_id: &'static str,
    model_input: Value,
    s3_uri: String,
    /// The prefix the manifest's key starts with, empty for a location that addresses the
    /// bucket root.
    prefix: &'static str,
}

/// The `modelInput` the Amazon Nova Reel schema takes.
fn nova_reel_input(text: &str) -> Value {
    json!({
        "taskType": "TEXT_VIDEO",
        "textToVideoParams": {"text": text},
        "videoGenerationConfig": {"durationSeconds": 6, "fps": 24, "dimension": "1280x720"},
    })
}

/// The async rows: they differ along the model invoked and along the SHAPE of the
/// configured location — the bucket root, one segment, the same segment written with a
/// trailing key separator, and a deep prefix — because the invocation's own segment is cut
/// into whichever of them was submitted.
fn async_rows() -> Vec<AsyncRow> {
    vec![
        AsyncRow {
            case: "the bucket root",
            model_id: "amazon.nova-reel-v1:0",
            model_input: nova_reel_input("A blue cube rotating slowly."),
            s3_uri: format!("s3://{OUTPUT_BUCKET}"),
            prefix: "",
        },
        AsyncRow {
            case: "one prefix segment",
            model_id: "amazon.nova-reel-v1:1",
            model_input: nova_reel_input("A red cube rotating slowly."),
            s3_uri: format!("s3://{OUTPUT_BUCKET}/output"),
            prefix: "output",
        },
        AsyncRow {
            case: "a trailing key separator",
            model_id: "luma.ray-v2:0",
            model_input: json!({"prompt": "A green cube rotating slowly."}),
            s3_uri: format!("s3://{OUTPUT_BUCKET}/output/"),
            prefix: "output",
        },
        AsyncRow {
            case: "a deep prefix",
            model_id: "luma.ray-v2:0",
            model_input: json!({"prompt": "A yellow cube rotating slowly."}),
            s3_uri: format!("s3://{OUTPUT_BUCKET}/renders/2026/january"),
            prefix: "renders/2026/january",
        },
    ]
}

/// RULE: an accepted asynchronous invocation leaves `InProgress` only when the clock makes
/// the transition due, and it reaches the modelled `Completed` status by WRITING its output
/// under its own segment of the configured location — so a read that reports it finished
/// carries no `failureMessage`, and the object is one this emulator's Amazon S3 answers for
/// through its own `GetObject`.
#[tokio::test]
async fn an_accepted_async_invocation_completes_by_writing_its_output_to_the_configured_location()
{
    let router = emulator_with_object_store();
    create_bucket(&router, OUTPUT_BUCKET).await;

    for row in async_rows() {
        let case = row.case;
        let arn = submit(&router, case, row.model_id, &row.model_input, &row.s3_uri).await;
        let invocation_id = arn
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();

        let record = poll_to_terminal(&router, case, &arn).await;
        assert_eq!(
            record["status"].as_str(),
            Some("Completed"),
            "case {case:?}: expected Completed, got {record}"
        );
        assert!(
            record["failureMessage"].is_null(),
            "case {case:?}: expected no failureMessage on the success path, got {:?}",
            record["failureMessage"]
        );
        assert!(
            !record["endTime"].is_null(),
            "case {case:?}: expected an endTime on a finished invocation, got {record}"
        );

        // The output is there to read, where the submitted location puts this
        // invocation's own segment — read back through Amazon S3's own GetObject, so what
        // is asserted is what the owning store answers for.
        let key = if row.prefix.is_empty() {
            format!("{invocation_id}/manifest.json")
        } else {
            format!("{}/{invocation_id}/manifest.json", row.prefix)
        };
        let object = s3_call(&router, "GET", &format!("/{OUTPUT_BUCKET}/{key}"), &[]).await;
        assert_eq!(
            object.status,
            200,
            "case {case:?}: expected the manifest at {key:?} to be readable, got {} / {}",
            object.status,
            String::from_utf8_lossy(&object.body)
        );
        let manifest: Value = serde_json::from_slice(&object.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the manifest at {key:?} is not JSON ({error}): {}",
                String::from_utf8_lossy(&object.body)
            )
        });
        assert_eq!(
            manifest["jobId"].as_str(),
            Some(invocation_id.as_str()),
            "case {case:?}: expected the manifest to name this invocation, got {manifest}"
        );
        assert_eq!(
            manifest["modelId"].as_str(),
            Some(row.model_id),
            "case {case:?}: expected the manifest to name the invoked model, got {manifest}"
        );

        // A terminal record is terminal: reading it again reports the identical record and
        // writes nothing new.
        let again = poll_to_terminal(&router, case, &arn).await;
        assert_eq!(
            again, record,
            "case {case:?}: expected a second read to report the identical record, got {again}"
        );
    }
}

/// RULE: the `Completed` status is backed by the write. An invocation whose configured
/// location this emulator's Amazon S3 holds no bucket for carried out none of the work it
/// was accepted for, so it ends in the modelled `Failed` status carrying the reason, and
/// nothing appears at the location it named.
#[tokio::test]
async fn an_async_invocation_whose_output_location_cannot_be_written_fails_with_the_reason() {
    let router = emulator_with_object_store();
    create_bucket(&router, OUTPUT_BUCKET).await;

    // The rows differ along the SHAPE of the unwritable location: a bucket this emulator
    // does not hold addressed at its root, and the same absent bucket under a deep prefix.
    // (A value that names no bucket at all never reaches this stage: the modelled `S3Uri`
    // pattern refuses it at the boundary.)
    let rows: [(&str, String); 2] = [
        (
            "a bucket the store does not hold",
            format!("s3://{ABSENT_BUCKET}"),
        ),
        (
            "an absent bucket under a deep prefix",
            format!("s3://{ABSENT_BUCKET}/renders/2026/january"),
        ),
    ];
    for (case, s3_uri) in rows {
        let arn = submit(
            &router,
            case,
            "amazon.nova-reel-v1:0",
            &nova_reel_input("A cube nothing renders."),
            &s3_uri,
        )
        .await;
        let record = poll_to_terminal(&router, case, &arn).await;
        assert_eq!(
            record["status"].as_str(),
            Some("Failed"),
            "case {case:?}: expected Failed, got {record}"
        );
        assert_eq!(
            record["failureMessage"].as_str(),
            Some(state::ASYNC_INVOKE_FAILURE_MESSAGE),
            "case {case:?}: expected the service's reason, got {:?}",
            record["failureMessage"]
        );
    }

    // Nothing was written into the bucket that DOES exist either: the failing rows named
    // another location, so this one holds no manifest of theirs.
    let listed = s3_call(
        &router,
        "GET",
        &format!("/{OUTPUT_BUCKET}?list-type=2"),
        &[],
    )
    .await;
    let body = String::from_utf8_lossy(&listed.body).to_string();
    assert!(
        !body.contains("manifest.json"),
        "case \"an unwritable destination\": expected no output in {OUTPUT_BUCKET:?}, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The `message` an error response states.
fn message_of(call: &Call) -> String {
    call.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The emulator with this service AND Amazon S3 on one shared engine, behind the
/// production router — the way `core/router` assembles them, which is what lets an
/// invocation's output be written into the store Amazon S3 answers reads from.
fn emulator_with_object_store() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(s3::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One Amazon S3 request on the same router, path-style and scoped to the `s3` signing
/// name, which is what routes it to the object store rather than to this service.
async fn s3_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> Call {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR000000000/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed S3 request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create `bucket` the way a caller would before submitting an invocation that writes
/// into it.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let answer = s3_call(router, "PUT", &format!("/{bucket}"), &[]).await;
    assert!(
        (200..300).contains(&answer.status),
        "the output bucket {bucket:?} must be created, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// Submit one asynchronous invocation and return its `invocationArn`.
async fn submit(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    model_input: &Value,
    s3_uri: &str,
) -> String {
    let call = post_json(
        router,
        "/async-invoke",
        &json!({
            "modelId": model_id,
            "modelInput": model_input,
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": s3_uri}},
        }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "case {case:?}: expected StartAsyncInvoke to accept the invocation, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", call.json()))
        .to_string()
}

/// How long a poll below waits for an invocation to leave `InProgress` before it gives up.
///
/// The transition is due a fixed span after the submitting call and NOTHING in the
/// emulator sleeps for it, so the test looks again until it holds. The budget is far above
/// that span, so a loaded or slow machine changes nothing about the outcome.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll waits between two looks: short enough that the test does not idle,
/// and it bounds nothing the test asserts.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Read the invocation `arn` names until it leaves `InProgress`, and report the record it
/// settled into.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let call = get(router, &format!("/async-invoke/{}", encode_path(arn))).await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected 200 from GetAsyncInvoke, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let record = call.json();
        if record["status"].as_str() != Some("InProgress") {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the invocation never left InProgress, last read {record}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}
