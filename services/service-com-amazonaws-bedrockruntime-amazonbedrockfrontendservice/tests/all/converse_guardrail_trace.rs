// SPDX-License-Identifier: CC0-1.0

//! The `trace` member of a `Converse` response: the guardrail assessments a turn
//! reports, asserted where a client sees them — on the wire.
//!
//! # The rule
//!
//! A Converse-API turn that names a guardrail reports that guardrail's assessments in
//! the response's modelled `trace.guardrail` exactly when the request's
//! `guardrailConfig.trace` asks for them — `enabled` or `enabled_full`, never the
//! modelled `disabled` default — keyed, on both the input and the output side, by the
//! id of the guardrail that was applied, and reported whether or not the guardrail
//! placed anything: the assessment is the evidence that the turn was evaluated, so a
//! turn the guardrail passed carries it as much as one it intervened on.
//!
//! `enabled_full` asks for the whole of the evaluation rather than what it placed, so
//! an entry the policies evaluated and placed nothing of is reported there and not
//! under `enabled`.
//!
//! # What the tables below generalise over
//!
//! The rows differ along the `trace` value the request carries (every value of the
//! modelled `GuardrailTrace` enum, plus a configuration stating none and a request
//! naming no guardrail at all), the revision of the guardrail that is applied (the
//! draft and a published version), whether the submitted text carries what the
//! policy configures, what the matched entry DOES (blocking the turn, or reporting
//! the detection and leaving it), and the output scope the trace is asked for. The
//! recorded case — one `trace: 'enabled'` turn the guardrail passed — is ONE row.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::guardrail_registry::{
    GuardrailPolicies, PiiEntry, PolicyAction, PublishedGuardrail, Sides, WordEntry, DRAFT_VERSION,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_runtime_inference::post_json;

/// The guardrail every row here applies.
const GUARDRAIL_ID: &str = "abcd1234efgh";

/// A version the control plane published beside the draft, so a row can apply a
/// revision that is not the draft.
const PUBLISHED_VERSION: &str = "1";

/// The word the guardrail's word policy configures.
const CONFIGURED_WORD: &str = "zephyrine";

/// What the guardrail answers content it blocked on the input side with.
const BLOCKED_INPUT: &str = "Sorry, the model cannot answer this question.";
const BLOCKED_OUTPUT: &str = "Sorry, the model cannot answer with this text.";

/// A model this service serves the Converse API for.
const MODEL_ID: &str = "amazon.nova-lite-v1:0";

/// The emulator under test: the production router with this service mounted on a
/// shared engine, plus the engine itself, so a row can publish the guardrail the
/// control plane would have published.
fn emulator_with_engine() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// A revision carrying the two blocked-messaging strings a guardrail is created
/// with, which each row then configures the policy it is about on top of.
fn revision() -> GuardrailPolicies {
    GuardrailPolicies {
        blocked_input_messaging: BLOCKED_INPUT.to_string(),
        blocked_outputs_messaging: BLOCKED_OUTPUT.to_string(),
        ..GuardrailPolicies::default()
    }
}

/// Publish `policies` as both the guardrail's DRAFT and its published version, the
/// way the control plane publishes a created guardrail and a version of it.
fn publish(engine: &engine::InMemoryEngine, policies: GuardrailPolicies) {
    let revisions = BTreeMap::from([
        (DRAFT_VERSION.to_string(), policies.clone()),
        (PUBLISHED_VERSION.to_string(), policies),
    ]);
    assert!(
        state::guardrail_registry::publish(
            engine,
            PublishedGuardrail {
                id: GUARDRAIL_ID.to_string(),
                arn: engine::emulator_arn(
                    "bedrock",
                    "us-east-1",
                    &format!("guardrail/{GUARDRAIL_ID}")
                ),
                revisions,
            }
        ),
        "the guardrail must be publishable on the engine the service holds"
    );
}

/// The `Converse` request body: one user turn, and the guardrail configuration a row
/// asks for.
fn converse_body(prompt: &str, guardrail: Option<Value>) -> Value {
    let mut body = json!({
        "messages": [{ "role": "user", "content": [{ "text": prompt }] }],
        "inferenceConfig": { "maxTokens": 20, "temperature": 0.0 },
    });
    if let Some(guardrail) = guardrail {
        body["guardrailConfig"] = guardrail;
    }
    body
}

/// One served `Converse` turn.
async fn converse(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let call = post_json(router, &format!("/model/{MODEL_ID}/converse"), body).await;
    assert_eq!(
        call.status,
        200,
        "case {case:?}: the turn must be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()
}

/// The text the turn answered with, its content blocks concatenated.
fn answered(turn: &Value) -> String {
    turn["output"]["message"]["content"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|block| block["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The custom words one assessment's word policy placed, as `(match, action,
/// detected)`.
fn placed_words(assessment: &Value) -> Vec<(String, String, bool)> {
    assessment["wordPolicy"]["customWords"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|word| {
            (
                word["match"].as_str().unwrap_or_default().to_string(),
                word["action"].as_str().unwrap_or_default().to_string(),
                word["detected"].as_bool().unwrap_or_default(),
            )
        })
        .collect()
}

/// One row of the trace table.
struct Row {
    case: &'static str,
    /// The `guardrailConfig` the request carries: `None` for a request that names no
    /// guardrail, otherwise the version to apply and the `trace` value to ask for
    /// (`None` for a configuration that states no trace at all).
    guardrail: Option<(&'static str, Option<&'static str>)>,
    prompt: &'static str,
    /// What the configured word entry does where it matches.
    configured: PolicyAction,
    /// Whether the prompt carries the configured word.
    placed: bool,
    /// Whether the response must carry the `trace` member.
    traced: bool,
}

/// RULE: the turn reports the named guardrail's assessments in `trace.guardrail`
/// exactly where `guardrailConfig.trace` asked for them, keyed by the applied
/// guardrail's id on both sides, and reports them whether or not the guardrail placed
/// anything.
#[tokio::test]
async fn the_turn_reports_the_guardrail_trace_exactly_where_the_request_asked_for_it() {
    // The `trace` values below are the whole of the modelled `GuardrailTrace` enum
    // (smithy_query shape GuardrailTrace: enabled, disabled, enabled_full), plus the
    // two ways a request asks for none: a configuration stating no trace (the
    // member's `disabled` default) and no guardrail configuration at all.
    let rows: [Row; 9] = [
        Row {
            case: "trace enabled on a turn the guardrail passed",
            guardrail: Some((DRAFT_VERSION, Some("enabled"))),
            prompt: "Tell me about page tokens.",
            configured: PolicyAction::Block,
            placed: false,
            traced: true,
        },
        Row {
            case: "trace enabled on a turn the guardrail blocked",
            guardrail: Some((DRAFT_VERSION, Some("enabled"))),
            prompt: "Tell me about zephyrine.",
            configured: PolicyAction::Block,
            placed: true,
            traced: true,
        },
        Row {
            case: "trace enabled where the matched entry only reports",
            guardrail: Some((DRAFT_VERSION, Some("enabled"))),
            prompt: "Tell me about zephyrine.",
            configured: PolicyAction::Detect,
            placed: true,
            traced: true,
        },
        Row {
            case: "trace enabled on a published version of the guardrail",
            guardrail: Some((PUBLISHED_VERSION, Some("enabled"))),
            prompt: "Tell me about page tokens.",
            configured: PolicyAction::Block,
            placed: false,
            traced: true,
        },
        Row {
            case: "trace enabled_full on a turn the guardrail passed",
            guardrail: Some((DRAFT_VERSION, Some("enabled_full"))),
            prompt: "Tell me about page tokens.",
            configured: PolicyAction::Block,
            placed: false,
            traced: true,
        },
        Row {
            case: "trace enabled_full on a turn the guardrail blocked",
            guardrail: Some((PUBLISHED_VERSION, Some("enabled_full"))),
            prompt: "Tell me about zephyrine.",
            configured: PolicyAction::Block,
            placed: true,
            traced: true,
        },
        Row {
            case: "trace disabled",
            guardrail: Some((DRAFT_VERSION, Some("disabled"))),
            prompt: "Tell me about zephyrine.",
            configured: PolicyAction::Block,
            placed: true,
            traced: false,
        },
        Row {
            case: "a configuration stating no trace",
            guardrail: Some((DRAFT_VERSION, None)),
            prompt: "Tell me about zephyrine.",
            configured: PolicyAction::Block,
            placed: true,
            traced: false,
        },
        Row {
            case: "no guardrail named at all",
            guardrail: None,
            prompt: "Tell me about zephyrine.",
            configured: PolicyAction::Block,
            placed: false,
            traced: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                words: vec![WordEntry {
                    text: CONFIGURED_WORD.to_string(),
                    sides: Sides::both(row.configured),
                }],
                ..revision()
            },
        );

        let config = row.guardrail.map(|(version, trace)| {
            let mut config = json!({
                "guardrailIdentifier": GUARDRAIL_ID,
                "guardrailVersion": version,
            });
            if let Some(trace) = trace {
                config["trace"] = json!(trace);
            }
            config
        });
        let turn = converse(&router, case, &converse_body(row.prompt, config)).await;

        // What the guardrail DID to the turn, which the trace is reported beside
        // rather than instead of.
        let blocked = row.placed && row.configured.blocks();
        let expected_stop = if blocked {
            "guardrail_intervened"
        } else {
            "end_turn"
        };
        assert_eq!(
            turn["stopReason"].as_str(),
            Some(expected_stop),
            "case {case:?}: expected the turn to stop on {expected_stop}, got {turn}"
        );
        let text = answered(&turn);
        if blocked {
            assert_eq!(
                text, BLOCKED_INPUT,
                "case {case:?}: a blocked turn answers the guardrail's input messaging, \
                 got {text:?}"
            );
        }

        if !row.traced {
            assert!(
                turn.get("trace").is_none(),
                "case {case:?}: a request that asked for no trace carries none, got {}",
                turn["trace"]
            );
            continue;
        }

        let guardrail = &turn["trace"]["guardrail"];
        assert!(
            guardrail.is_object(),
            "case {case:?}: the turn must report trace.guardrail, got {turn}"
        );
        // The assessments are keyed by the id of the guardrail that was applied, on
        // both sides.
        let input = &guardrail["inputAssessment"][GUARDRAIL_ID];
        assert!(
            input.is_object(),
            "case {case:?}: the input assessment must be keyed by {GUARDRAIL_ID}, got {}",
            guardrail["inputAssessment"]
        );
        // The evidence that the turn was EVALUATED: an assessment the evaluation
        // filled, carried even where nothing was placed, naming the revision that was
        // applied and what it read.
        let applied_version = row
            .guardrail
            .map(|(version, _)| version)
            .unwrap_or(DRAFT_VERSION);
        let names_the_revision = |side: &str, assessment: &Value| {
            let details = &assessment["appliedGuardrailDetails"];
            assert_eq!(
                details["guardrailId"].as_str(),
                Some(GUARDRAIL_ID),
                "case {case:?}: the {side} assessment names the guardrail it applied, \
                 got {assessment}"
            );
            assert_eq!(
                details["guardrailVersion"].as_str(),
                Some(applied_version),
                "case {case:?}: the {side} assessment names the revision it applied, \
                 got {assessment}"
            );
            assert!(
                assessment["invocationMetrics"]["usage"]["wordPolicyUnits"].is_number(),
                "case {case:?}: the {side} assessment reports what the evaluation read, \
                 got {assessment}"
            );
        };
        names_the_revision("input", input);
        // A turn the guardrail SERVED was assessed on the output side too, and that
        // assessment is reported — keyed by the same guardrail id, and carried whether
        // or not the output side placed anything, which is the only way a caller can
        // tell an evaluated completion from an unevaluated one. The text that was
        // assessed is reported beside it.
        if !blocked {
            let outputs = guardrail["outputAssessments"][GUARDRAIL_ID]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                outputs.len(),
                1,
                "case {case:?}: one guardrail applied to the turn reports one output \
                 assessment under {GUARDRAIL_ID}, got {}",
                guardrail["outputAssessments"]
            );
            names_the_revision("output", &outputs[0]);
            let model_output: Vec<String> = guardrail["modelOutput"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|text| text.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                model_output,
                vec![text.clone()],
                "case {case:?}: a turn that was served reports the text it served as the \
                 assessed output, got {model_output:?}"
            );
        }
        // What the word policy placed on the side the text was submitted on, with the
        // action the configured entry takes.
        let expected_words: Vec<(String, String, bool)> = if row.placed {
            vec![(
                CONFIGURED_WORD.to_string(),
                row.configured.assessed().to_string(),
                true,
            )]
        } else {
            Vec::new()
        };
        let actual_words = placed_words(input);
        assert_eq!(
            actual_words, expected_words,
            "case {case:?}: expected the input assessment to place {expected_words:?}, \
             got {actual_words:?} in {input}"
        );
    }
}

/// RULE (the scope half of it): `enabled_full` asks for every entry the policies
/// evaluated rather than only the ones they placed, so an entry that placed nothing is
/// reported in the trace under `enabled_full` and nowhere under `enabled`.
#[tokio::test]
async fn the_full_trace_reports_entries_the_policies_evaluated_and_placed_nothing() {
    // (case, the trace asked for, the undetected entry reported)
    let rows: [(&str, &str, bool); 2] = [
        ("trace enabled", "enabled", false),
        ("trace enabled_full", "enabled_full", true),
    ];
    for (case, trace, reported) in rows {
        let (engine, router) = emulator_with_engine();
        publish(
            &engine,
            GuardrailPolicies {
                // A modelled `GuardrailPiiEntityType` the submitted text carries
                // nothing of, so the entry is one the policy evaluated and placed
                // nothing of.
                pii_entities: vec![PiiEntry {
                    entity_type: "EMAIL".to_string(),
                    sides: Sides::both(PolicyAction::Anonymize),
                }],
                ..revision()
            },
        );

        let turn = converse(
            &router,
            case,
            &converse_body(
                "Tell me about page tokens.",
                Some(json!({
                    "guardrailIdentifier": GUARDRAIL_ID,
                    "guardrailVersion": DRAFT_VERSION,
                    "trace": trace,
                })),
            ),
        )
        .await;

        let guardrail = &turn["trace"]["guardrail"];
        let outputs = guardrail["outputAssessments"][GUARDRAIL_ID]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            outputs.len(),
            1,
            "case {case:?}: the turn must report one output assessment under \
             {GUARDRAIL_ID}, got {turn}"
        );
        for (side, assessment) in [
            ("input", guardrail["inputAssessment"][GUARDRAIL_ID].clone()),
            ("output", outputs[0].clone()),
        ] {
            let entries = assessment["sensitiveInformationPolicy"]["piiEntities"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let actual: Vec<(String, bool)> = entries
                .iter()
                .map(|entry| {
                    (
                        entry["type"].as_str().unwrap_or_default().to_string(),
                        entry["detected"].as_bool().unwrap_or_default(),
                    )
                })
                .collect();
            let expected: Vec<(String, bool)> = if reported {
                vec![("EMAIL".to_string(), false)]
            } else {
                Vec::new()
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the {side} assessment to report {expected:?}, \
                 got {actual:?} in {assessment}"
            );
        }
    }
}
