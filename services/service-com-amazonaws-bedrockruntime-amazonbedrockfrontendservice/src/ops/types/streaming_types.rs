// SPDX-License-Identifier: CC0-1.0

//! Type definitions for the two `@streaming` operations this crate hand-authors,
//! `ConverseStream` and `InvokeModelWithResponseStream` (`src/ops/SKIPPED_STUBS.md`
//! records why the op-stub generator declined them).
//!
//! Three groups:
//!
//! * the request shapes, declared in full so the framework's own
//!   `HttpBindingDeserializer` fills them from the URI label, the headers and the
//!   JSON body and `constraints::validate` checks them against the model's
//!   `@length` / `@pattern` / enum-membership traits — the request half is validated
//!   by the model, not by hand;
//! * the `@httpHeader`-bound half of `InvokeModelWithResponseStreamResponse`, whose
//!   remaining member is the event-stream payload (`ConverseStreamResponse` binds no
//!   header at all, so it needs no such struct);
//! * the event structures each operation's output union carries, one per frame the
//!   response emits.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules: the shapes `Converse` already declares (`Message`, `TokenUsage`,
//! `InferenceConfiguration`, `ToolConfiguration`, `PerformanceConfiguration`,
//! `ServiceTier`, `OutputConfig`, `GuardrailTraceAssessment`, `PromptRouterTrace`, …)
//! are reused from `converse_types` / `shared_types` rather than re-declared here.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json");

// =============================================================================
// ConverseStream — request
// =============================================================================

/// `ConverseStreamRequest`. Identical to `ConverseRequest` except for
/// `guardrailConfig`, which is the STREAM configuration (it carries
/// `streamProcessingMode`).
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ConverseStreamRequest")]
#[derive(Debug, Clone, Default)]
pub struct ConverseStreamInput {
    pub model_id: String,
    pub messages: Option<Vec<Message>>,
    pub system: Option<Vec<SystemContentBlock>>,
    pub inference_config: Option<InferenceConfiguration>,
    pub tool_config: Option<ToolConfiguration>,
    pub guardrail_config: Option<GuardrailStreamConfiguration>,
    pub project_identifier: Option<String>,
    pub additional_model_request_fields: Option<::aws_smithy_types::Document>,
    pub prompt_variables: Option<std::collections::HashMap<String, PromptVariableValues>>,
    pub additional_model_response_field_paths: Option<Vec<String>>,
    pub request_metadata: Option<std::collections::HashMap<String, String>>,
    pub performance_config: Option<PerformanceConfiguration>,
    pub service_tier: Option<ServiceTier>,
    pub output_config: Option<OutputConfig>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#GuardrailStreamConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct GuardrailStreamConfiguration {
    pub guardrail_identifier: Option<String>,
    pub guardrail_version: Option<String>,
    pub trace: String,
    pub stream_processing_mode: String,
}

// =============================================================================
// ConverseStream — the events `ConverseStreamOutput` carries
// =============================================================================

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#MessageStartEvent")]
#[derive(Debug, Clone, Default)]
pub struct MessageStartEvent {
    pub role: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ContentBlockStartEvent")]
#[derive(Debug, Clone)]
pub struct ContentBlockStartEvent {
    pub start: ContentBlockStart,
    pub content_block_index: i32,
}

/// `ContentBlockStart`. Only the `toolUse` member is declared: the others start a
/// tool result or a streamed image block, neither of which this emulator produces.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockruntime#ContentBlockStart",
    unsupported = ["toolResult", "image"]
)]
#[derive(Debug, Clone)]
pub enum ContentBlockStart {
    toolUse(ToolUseBlockStart),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ToolUseBlockStart")]
#[derive(Debug, Clone, Default)]
pub struct ToolUseBlockStart {
    pub tool_use_id: String,
    pub name: String,
    pub r#type: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ToolUseBlockDelta")]
#[derive(Debug, Clone, Default)]
pub struct ToolUseBlockDelta {
    pub input: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ContentBlockDeltaEvent")]
#[derive(Debug, Clone)]
pub struct ContentBlockDeltaEvent {
    pub delta: ContentBlockDelta,
    pub content_block_index: i32,
}

/// `ContentBlockDelta`. Only the `text` and `toolUse` members are declared: the
/// others carry a tool result, reasoning content, a citation or an image delta,
/// none of which this emulator produces (no model runs in it), and a union member
/// that is never constructed would be dead weight on every frame. Whoever teaches
/// this service to stream one of them declares it then, beside the code that fills
/// it.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockruntime#ContentBlockDelta",
    unsupported = ["toolResult", "reasoningContent", "citation", "image"]
)]
#[derive(Debug, Clone)]
pub enum ContentBlockDelta {
    Text(String),
    toolUse(ToolUseBlockDelta),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ContentBlockStopEvent")]
#[derive(Debug, Clone, Default)]
pub struct ContentBlockStopEvent {
    pub content_block_index: i32,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#MessageStopEvent")]
#[derive(Debug, Clone, Default)]
pub struct MessageStopEvent {
    pub stop_reason: String,
    pub additional_model_response_fields: Option<::aws_smithy_types::Document>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ConverseStreamMetadataEvent")]
#[derive(Debug, Clone)]
pub struct ConverseStreamMetadataEvent {
    pub usage: TokenUsage,
    pub metrics: ConverseStreamMetrics,
    pub trace: Option<ConverseStreamTrace>,
    pub performance_config: Option<PerformanceConfiguration>,
    pub service_tier: Option<ServiceTier>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ConverseStreamMetrics")]
#[derive(Debug, Clone, Default)]
pub struct ConverseStreamMetrics {
    pub latency_ms: i64,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ConverseStreamTrace")]
#[derive(Debug, Clone, Default)]
pub struct ConverseStreamTrace {
    pub guardrail: Option<GuardrailTraceAssessment>,
    pub prompt_router: Option<PromptRouterTrace>,
}

// =============================================================================
// InvokeModelWithResponseStream
// =============================================================================

/// `InvokeModelWithResponseStreamRequest`. The same members as
/// `InvokeModelRequest`, except that `accept` binds `X-Amzn-Bedrock-Accept` rather
/// than `Accept`.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockruntime#InvokeModelWithResponseStreamRequest"
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeModelWithResponseStreamInput {
    pub body: Option<Vec<u8>>,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub model_id: String,
    pub trace: Option<String>,
    pub guardrail_identifier: Option<String>,
    pub guardrail_version: Option<String>,
    pub performance_config_latency: String,
    pub service_tier: Option<String>,
    pub request_metadata: Option<String>,
    pub project_identifier: Option<String>,
}

/// The `@httpHeader`-bound half of `InvokeModelWithResponseStreamResponse`; `body` is
/// the `ResponseStream` event-stream payload and is framed separately.
///
/// `contentType` is `@required` and binds `X-Amzn-Bedrock-Content-Type`, so a client
/// learns the MIME type of the payload the frames carry.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockruntime#InvokeModelWithResponseStreamResponse",
    unsupported = ["body"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeModelWithResponseStreamHeaders {
    pub content_type: String,
    pub performance_config_latency: Option<String>,
    pub service_tier: Option<String>,
}

/// The `chunk` member of `ResponseStream`: one piece of the model's own response
/// payload, in the MIME type the response's `contentType` header reports.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#PayloadPart")]
#[derive(Debug, Clone, Default)]
pub struct PayloadPart {
    pub bytes: Option<Vec<u8>>,
}

/// `ModelStreamErrorException` (`@error client`, `@httpError 424`): the error both
/// streaming operations declare and that their output unions can also carry in
/// band. Declared here because no non-streaming operation of this service reaches
/// it, so the scaffold emitted no type for it.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockruntime#ModelStreamErrorException")]
#[derive(Debug, Clone, Default)]
pub struct ModelStreamErrorException {
    pub message: Option<String>,
    pub original_status_code: Option<i32>,
    pub original_message: Option<String>,
}
