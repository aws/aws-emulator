// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/account_enforced_guardrail.rs"]
mod account_enforced_guardrail;
#[path = "all/apply_guardrail.rs"]
mod apply_guardrail;
#[path = "all/async_invoke_model_contract.rs"]
mod async_invoke_model_contract;
#[path = "all/bedrock_cloudwatch_request_metrics.rs"]
mod bedrock_cloudwatch_request_metrics;
#[path = "all/bedrock_cloudwatch_token_metric_dimensioning.rs"]
mod bedrock_cloudwatch_token_metric_dimensioning;
#[path = "all/bedrock_cloudwatch_token_metrics.rs"]
mod bedrock_cloudwatch_token_metrics;
#[path = "all/bedrock_runtime_inference.rs"]
mod bedrock_runtime_inference;
#[path = "all/bedrock_runtime_request_claim.rs"]
mod bedrock_runtime_request_claim;
#[path = "all/bedrock_runtime_streaming.rs"]
mod bedrock_runtime_streaming;
#[path = "all/catalogued_model_inference.rs"]
mod catalogued_model_inference;
#[path = "all/converse_guardrail_trace.rs"]
mod converse_guardrail_trace;
#[path = "all/converse_prompt_cache.rs"]
mod converse_prompt_cache;
#[path = "all/converse_role_sequence_and_stream_increments.rs"]
mod converse_role_sequence_and_stream_increments;
#[path = "all/converse_tool_choice.rs"]
mod converse_tool_choice;
#[path = "all/converse_tool_use_arguments.rs"]
mod converse_tool_use_arguments;
#[path = "all/guardrail_checks_conduct_categories.rs"]
mod guardrail_checks_conduct_categories;
#[path = "all/guardrail_checks_evaluation.rs"]
mod guardrail_checks_evaluation;
#[path = "all/guardrail_checks_inline.rs"]
mod guardrail_checks_inline;
#[path = "all/guardrail_checks_phone_numbers.rs"]
mod guardrail_checks_phone_numbers;
#[path = "all/guardrail_checks_scores.rs"]
mod guardrail_checks_scores;
#[path = "all/inference_generation_and_refusal_wording.rs"]
mod inference_generation_and_refusal_wording;
#[path = "all/inference_input_limits.rs"]
mod inference_input_limits;
#[path = "all/inference_stop_sequences.rs"]
mod inference_stop_sequences;
#[path = "all/inline_guardrail_error_taxonomy.rs"]
mod inline_guardrail_error_taxonomy;
#[path = "all/invoke_model_malformed_body_refusal.rs"]
mod invoke_model_malformed_body_refusal;
#[path = "all/resource_reference_refusals.rs"]
mod resource_reference_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
