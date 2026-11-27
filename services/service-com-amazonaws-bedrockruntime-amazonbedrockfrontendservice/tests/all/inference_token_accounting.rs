// SPDX-License-Identifier: CC0-1.0

//! What a synchronous inference call REPORTS about the turn it served: the token
//! accounting in its usage envelope, the members that envelope carries, and the
//! generation cap that bounds the turn — end to end over HTTP.
//!
//! Three rules are asserted here, each over a SET of inputs rather than over the one
//! request that showed it:
//!
//! * the prompt tokens a call reports are what a BYTE-PAIR tokenizer makes of the prompt
//!   text — a word the vocabulary holds is one token however long it is, a word it does
//!   not hold is cut into the pieces it does hold — and every surface that counts an input
//!   (`CountTokens`, `Converse`, `InvokeModel`) reports the same number for the same text,
//!   with `totalTokens` the sum of what was counted in and what was generated;
//! * a `Converse` response's usage envelope reports exactly the members `TokenUsage`
//!   declares — the three required counts on every turn, and the cache members only on a
//!   turn that engaged the prompt cache — and no member the shape does not declare;
//! * the generation cap a request carries is read under EVERY name the invoked family's
//!   request format publishes for it, and the cap is reported as the cause of the stop
//!   exactly when the served tokens spend the whole of it with more left to say.

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_runtime_inference::{emulator, post_json, send};

/// A model the Converse API serves, for the rows that do not vary the model.
const CONVERSE_MODEL: &str = "amazon.nova-lite-v1:0";

/// A model `CountTokens` serves: AWS publishes that operation for Anthropic Claude models,
/// and this emulator's inventory follows it, so the counting surface is driven on one of
/// them. The count is the tokenizer's, not the model's, so it is the same number the other
/// surfaces report for the same text.
const COUNTING_MODEL: &str = "anthropic.claude-3-sonnet-20240229-v1:0";

// ---------------------------------------------------------------------------
// The prompt-token count
// ---------------------------------------------------------------------------

/// One prompt and the number of tokens the vocabulary makes of it.
struct CountRow {
    case: &'static str,
    /// The pieces of prompt text the request carries, in order.
    pieces: &'static [&'static str],
    /// The tokens the whole prompt is. Measured with the tokenizer's own published
    /// vocabulary, not with a character rule: a word the vocabulary holds costs one
    /// token however long it is.
    tokens: i64,
}

/// The counting table: rows differ along what the count generalises over — the length of
/// a word, whether the vocabulary holds it whole, punctuation, a script outside ASCII,
/// and how many pieces of text the prompt is assembled from.
const COUNT_ROWS: &[CountRow] = &[
    CountRow {
        case: "one short word",
        pieces: &["hello"],
        tokens: 1,
    },
    CountRow {
        // Eight characters, so a four-character rule would count two; the vocabulary
        // holds the word, so it is one token.
        case: "one long common word",
        pieces: &["language"],
        tokens: 1,
    },
    CountRow {
        case: "a word the vocabulary does not hold whole",
        pieces: &["zephyrine"],
        tokens: 3,
    },
    CountRow {
        case: "a sentence with punctuation",
        pieces: &["Name one planet."],
        tokens: 4,
    },
    CountRow {
        case: "a sentence whose words carry their leading space",
        pieces: &["The request carried one message."],
        tokens: 6,
    },
    CountRow {
        // Characters the vocabulary holds no whole word of, so each costs more than one
        // token of its own.
        case: "a script outside ASCII",
        pieces: &["日本語"],
        tokens: 4,
    },
    CountRow {
        // Each piece of prompt text is counted on its own, so a prompt assembled from
        // several pieces costs the sum of their counts.
        case: "two pieces of prompt text",
        pieces: &["first part,", "second part."],
        tokens: 6,
    },
];

/// RULE: the prompt tokens a synchronous inference call reports are the tokens a byte-pair
/// tokenizer makes of the prompt text the service was given — so a word the vocabulary
/// holds costs ONE token whatever its length — and `CountTokens`, `Converse` and
/// `InvokeModel` report that one number for the same text, with `totalTokens` equal to the
/// input plus the generated count.
#[tokio::test]
async fn the_prompt_tokens_reported_are_the_byte_pair_count_of_the_prompt_text() {
    let router = emulator();

    for row in COUNT_ROWS {
        let case = row.case;
        // The tokenizer itself, over each piece and over the pieces together.
        let counted: i64 = row
            .pieces
            .iter()
            .map(|piece| i64::from(state::count_text_tokens(piece)))
            .sum();
        assert_eq!(
            counted, row.tokens,
            "case {case:?}: expected the vocabulary to make {} tokens of {:?}, got {counted}",
            row.tokens, row.pieces
        );

        // The Converse surface: one user turn carrying the row's pieces as its blocks.
        let content: Vec<Value> = row.pieces.iter().map(|text| json!({"text": text})).collect();
        let messages = json!([{ "role": "user", "content": content }]);
        let converse = post_json(
            &router,
            &format!("/model/{CONVERSE_MODEL}/converse"),
            &json!({ "messages": messages }),
        )
        .await;
        assert_eq!(
            converse.status,
            200,
            "case {case:?}: expected the turn to be served, got {} / {}",
            converse.status,
            String::from_utf8_lossy(&converse.body)
        );
        let usage = converse.json()["usage"].clone();
        assert_eq!(
            usage["inputTokens"].as_i64(),
            Some(row.tokens),
            "case {case:?}: expected Converse to report {} input tokens, got {usage}",
            row.tokens
        );
        assert_eq!(
            usage["totalTokens"].as_i64(),
            Some(row.tokens + usage["outputTokens"].as_i64().unwrap_or_default()),
            "case {case:?}: expected totalTokens to be the input plus the output count, \
             got {usage}"
        );

        // `CountTokens` over the same Converse-shaped input.
        let counting = post_json(
            &router,
            &format!("/model/{COUNTING_MODEL}/count-tokens"),
            &json!({ "input": { "converse": { "messages": messages } } }),
        )
        .await;
        assert_eq!(
            counting.status,
            200,
            "case {case:?}: expected CountTokens to answer, got {} / {}",
            counting.status,
            String::from_utf8_lossy(&counting.body)
        );
        assert_eq!(
            counting.json()["inputTokens"].as_i64(),
            Some(row.tokens),
            "case {case:?}: expected CountTokens to report {} input tokens, got {}",
            row.tokens,
            counting.json()
        );

        // The invoke surface, whose body carries the same pieces as its own prompt text.
        let body = json!({
            "messages": [{ "role": "user", "content": content }],
        });
        let invoke = send(
            &router,
            "POST",
            &format!("/model/{CONVERSE_MODEL}/invoke"),
            &[("content-type", "application/json")],
            serde_json::to_vec(&body).expect("a serializable body"),
        )
        .await;
        assert_eq!(
            invoke.status,
            200,
            "case {case:?}: expected the invocation to be served, got {} / {}",
            invoke.status,
            String::from_utf8_lossy(&invoke.body)
        );
        let invoked = invoke.json();
        assert_eq!(
            invoked["usage"]["inputTokens"].as_i64(),
            Some(row.tokens),
            "case {case:?}: expected InvokeModel to report {} input tokens, got {invoked}",
            row.tokens
        );
    }
}

// ---------------------------------------------------------------------------
// The members of the usage envelope
// ---------------------------------------------------------------------------

/// One Converse request, for the rule about which members its usage envelope carries.
struct UsageRow {
    case: &'static str,
    request: fn() -> Value,
    /// Whether the turn engages the prompt cache, which is what the cache members are
    /// reported for.
    caches: bool,
}

/// The members `TokenUsage` declares: the three `@required` counts every turn reports, and
/// the three that report the prompt cache. Source:
/// `smithy_query(query="shape", shape="TokenUsage")`.
const REQUIRED_USAGE_MEMBERS: &[&str] = &["inputTokens", "outputTokens", "totalTokens"];
const CACHE_USAGE_MEMBERS: &[&str] = &[
    "cacheReadInputTokens",
    "cacheWriteInputTokens",
    "cacheDetails",
];

/// The table: rows differ along what the turn DOES — plain text, a system prompt, a turn
/// obliged to request a tool, a turn the generation cap cuts off, and a turn carrying a
/// cache checkpoint — since the envelope's membership is the shape's, whatever the turn did.
fn usage_rows() -> Vec<UsageRow> {
    vec![
        UsageRow {
            case: "a plain turn",
            request: || json!({ "messages": [{"role": "user", "content": [{"text": "hello"}]}] }),
            caches: false,
        },
        UsageRow {
            case: "a turn with a system prompt",
            request: || {
                json!({
                    "system": [{"text": "You are terse."}],
                    "messages": [{"role": "user", "content": [{"text": "Name one planet."}]}],
                })
            },
            caches: false,
        },
        UsageRow {
            case: "a turn obliged to request a tool",
            request: || {
                json!({
                    "messages": [{"role": "user", "content": [{"text": "What is AMZN at?"}]}],
                    "toolConfig": {
                        "tools": [{"toolSpec": {
                            "name": "quote",
                            "description": "Look up a quote.",
                            "inputSchema": {"json": {
                                "type": "object",
                                "properties": {"symbol": {"type": "string"}},
                            }},
                        }}],
                        "toolChoice": {"any": {}},
                    },
                })
            },
            caches: false,
        },
        UsageRow {
            case: "a turn the generation cap cuts off",
            request: || {
                json!({
                    "messages": [{"role": "user", "content": [{"text": "Name one planet."}]}],
                    "inferenceConfig": {"maxTokens": 2},
                })
            },
            caches: false,
        },
        UsageRow {
            case: "a turn carrying a cache checkpoint",
            request: || {
                json!({
                    "messages": [{"role": "user", "content": [
                        {"text": "Everything before the checkpoint."},
                        {"cachePoint": {"type": "default"}},
                        {"text": "Name one planet."},
                    ]}],
                })
            },
            caches: true,
        },
    ]
}

/// RULE: a `Converse` response's usage envelope carries exactly the members the modelled
/// `TokenUsage` declares — the three required counts on every turn whatever it did, the
/// cache members only on a turn that engaged the prompt cache — and no member the shape
/// does not declare, so a client reading the envelope finds every count the shape promises
/// and nothing the shape no longer carries.
#[tokio::test]
async fn a_converse_turn_reports_exactly_the_members_the_usage_shape_declares() {
    let router = emulator();

    for row in usage_rows() {
        let case = row.case;
        let call = post_json(
            &router,
            &format!("/model/{CONVERSE_MODEL}/converse"),
            &(row.request)(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected the turn to be served, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let usage = call.json()["usage"].clone();
        let reported: Vec<String> = usage
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the usage envelope must be an object"))
            .keys()
            .cloned()
            .collect();
        for member in REQUIRED_USAGE_MEMBERS {
            assert!(
                usage.get(member).and_then(Value::as_i64).is_some(),
                "case {case:?}: expected {member} to be reported as a count, got {usage}"
            );
        }
        for member in CACHE_USAGE_MEMBERS {
            assert_eq!(
                usage.get(member).is_some(),
                row.caches,
                "case {case:?}: expected {member} to be reported only for a cached turn, got \
                 {usage}"
            );
        }
        let declared: Vec<&str> = REQUIRED_USAGE_MEMBERS
            .iter()
            .chain(CACHE_USAGE_MEMBERS)
            .copied()
            .collect();
        let undeclared: Vec<&String> = reported
            .iter()
            .filter(|member| !declared.contains(&member.as_str()))
            .collect();
        assert!(
            undeclared.is_empty(),
            "case {case:?}: the envelope must carry no member TokenUsage does not declare, \
             got {undeclared:?} in {usage}"
        );
    }
}

// ---------------------------------------------------------------------------
// The generation cap
// ---------------------------------------------------------------------------

/// One family, one of the names its request format publishes for the generation cap, and
/// where its response reports the stop reason and the generated count.
struct CapSpellingRow {
    case: &'static str,
    model_id: &'static str,
    /// The request body with `CAP` standing where the cap's value goes.
    template: &'static str,
    /// The same body with no cap at all.
    uncapped: &'static str,
    stop_pointer: &'static str,
    output_tokens_pointer: &'static str,
    /// What this family calls a turn generation ran into the ceiling of, and a turn that
    /// finished on its own — each family's own vocabulary, written out here so the table
    /// states what it expects rather than asking the service what it would say.
    length_stop: &'static str,
    finished_stop: &'static str,
}

/// The cap-spelling table: every name a family's own request format binds its cap to.
///
/// Source: the per-family request formats in "Inference request parameters and response
/// fields for foundation models" (Amazon Bedrock User Guide) — Amazon Nova's
/// `inferenceConfig.max_new_tokens` beside the `inferenceConfig.maxTokens` of the
/// Converse-shaped request its body is — and, for the provider Bedrock serves on the
/// OpenAI-compatible chat completions envelope, that API's `max_tokens` and the
/// `max_completion_tokens` that replaced it.
const CAP_SPELLING_ROWS: &[CapSpellingRow] = &[
    CapSpellingRow {
        case: "nova inferenceConfig.max_new_tokens",
        model_id: "amazon.nova-lite-v1:0",
        template: r#"{"messages":[{"role":"user","content":[{"text":"Name one planet."}]}],"inferenceConfig":{"max_new_tokens":CAP}}"#,
        uncapped: r#"{"messages":[{"role":"user","content":[{"text":"Name one planet."}]}]}"#,
        stop_pointer: "/stopReason",
        output_tokens_pointer: "/usage/outputTokens",
        length_stop: "max_tokens",
        finished_stop: "end_turn",
    },
    CapSpellingRow {
        case: "nova inferenceConfig.maxTokens",
        model_id: "amazon.nova-pro-v1:0",
        template: r#"{"messages":[{"role":"user","content":[{"text":"Name one planet."}]}],"inferenceConfig":{"maxTokens":CAP}}"#,
        uncapped: r#"{"messages":[{"role":"user","content":[{"text":"Name one planet."}]}]}"#,
        stop_pointer: "/stopReason",
        output_tokens_pointer: "/usage/outputTokens",
        length_stop: "max_tokens",
        finished_stop: "end_turn",
    },
    CapSpellingRow {
        case: "openai-compatible max_tokens",
        model_id: "moonshotai.kimi-k2.5",
        template: r#"{"messages":[{"role":"user","content":"Name one planet."}],"max_tokens":CAP}"#,
        uncapped: r#"{"messages":[{"role":"user","content":"Name one planet."}]}"#,
        stop_pointer: "/choices/0/finish_reason",
        output_tokens_pointer: "/usage/completion_tokens",
        length_stop: "length",
        finished_stop: "stop",
    },
    CapSpellingRow {
        case: "openai-compatible max_completion_tokens",
        model_id: "moonshotai.kimi-k2.5",
        template: r#"{"messages":[{"role":"user","content":"Name one planet."}],"max_completion_tokens":CAP}"#,
        uncapped: r#"{"messages":[{"role":"user","content":"Name one planet."}]}"#,
        stop_pointer: "/choices/0/finish_reason",
        output_tokens_pointer: "/usage/completion_tokens",
        length_stop: "length",
        finished_stop: "stop",
    },
];

/// One invoke call with the body as given.
async fn invoke(router: &MultiServiceRouter, model_id: &str, body: String, case: &str) -> Value {
    let call = send(
        router,
        "POST",
        &format!("/model/{model_id}/invoke"),
        &[("content-type", "application/json")],
        body.into_bytes(),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "case {case:?}: expected the invocation to be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()
}

/// RULE: the generation cap bounds the turn under every name the invoked family's request
/// format publishes for it — the generated tokens never exceed it — and the family's
/// length stop reason is reported exactly when the served tokens spend the whole cap with
/// more of the completion left to say, while a cap that leaves budget unspent, or no cap at
/// all, leaves the turn reported as finished.
#[tokio::test]
async fn the_generation_cap_is_honoured_under_every_name_its_family_publishes() {
    let router = emulator();
    // The shortest completion this emulator serves is what the caps below are placed
    // around: a cap at or below it cannot hold anything beside it, so the cap is spent in
    // full; one token more leaves budget unspent.
    let floor = i64::from(state::count_text_tokens(state::EMULATED_COMPLETION_FLOOR));

    for row in CAP_SPELLING_ROWS {
        for (size, cap, spent) in [
            ("one token", 1, true),
            ("one token below the shortest completion", floor - 1, true),
            ("exactly the shortest completion", floor, true),
            ("one token above the shortest completion", floor + 1, false),
            ("far above any completion", 512, false),
        ] {
            let case = format!("{} / a cap of {cap} ({size})", row.case);
            let served = invoke(
                &router,
                row.model_id,
                row.template.replace("CAP", &cap.to_string()),
                &case,
            )
            .await;
            let output = served
                .pointer(row.output_tokens_pointer)
                .and_then(Value::as_i64)
                .unwrap_or_default();
            assert!(
                output <= cap,
                "case {case:?}: expected at most {cap} generated tokens, got {output} in \
                 {served}"
            );
            let expected = match spent {
                true => row.length_stop,
                false => row.finished_stop,
            };
            assert_eq!(
                served.pointer(row.stop_pointer).and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected the stop reason {expected:?} at {}, got {served}",
                row.stop_pointer
            );
        }

        // The control: the same body with no cap at all is answered with the whole
        // completion and the family's finished stop reason, so the rows above report what
        // the CAP did and not what the completion does anyway.
        let case = format!("{} / no cap at all", row.case);
        let served = invoke(&router, row.model_id, row.uncapped.to_string(), &case).await;
        let output = served
            .pointer(row.output_tokens_pointer)
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            output > floor,
            "case {case:?}: expected an uncapped turn to generate more than the \
             {floor}-token shortest completion, got {output} in {served}"
        );
        let expected = row.finished_stop;
        assert_eq!(
            served.pointer(row.stop_pointer).and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the stop reason {expected:?} at {}, got {served}",
            row.stop_pointer
        );
    }
}
