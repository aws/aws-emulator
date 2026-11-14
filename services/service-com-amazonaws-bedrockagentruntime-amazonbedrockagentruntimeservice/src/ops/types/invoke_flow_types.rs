// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_flow` operation, HAND-AUTHORED: the
//! op-stub generator declined `InvokeFlow` because its `responseStream` output member is a
//! `@streaming` event stream it cannot frame (`../SKIPPED_STUBS.md`), and an operation with
//! no types and no handler is in no routing table at all — so no request ever reached a
//! bedrock-agent-runtime handler on `POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}`.
//!
//! The split follows the one the other hand-authored streaming surfaces of this service use
//! (`optimize_prompt_types.rs`): the members the framework's own codec can read are
//! declared, so this operation's `@required`, `@length` and `@pattern` members — including
//! the two URI labels — are checked against the MODEL rather than by hand, and the event
//! stream sits in `unsupported = [..]`, which keeps it out of the schema. The EVENTS the
//! stream carries are declared here, because the handler frames them itself; the modelled
//! exceptions the same stream can carry are the shapes `shared_types` already declares.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `InvokeFlowRequest`: the two URI labels that name the flow and its alias, and the one
/// input the flow is entered with (`inputs` is `@length(1, 1)`).
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#InvokeFlowRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeFlowInput {
    pub flow_identifier: String,
    pub flow_alias_identifier: String,
    pub inputs: Vec<FlowInput>,
    pub enable_trace: Option<bool>,
    pub model_performance_configuration: Option<ModelPerformanceConfiguration>,
    pub execution_id: Option<String>,
}

/// `InvokeFlowResponse`: `responseStream` is the `@streaming` `@httpPayload`, which the
/// handler frames itself, so the declared half carries the one modelled response header.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InvokeFlowResponse",
    unsupported = ["responseStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeFlowOutput {
    pub execution_id: Option<String>,
}

/// One input into a flow: the input node it enters at, and the document it carries.
///
/// Declared in `start_flow_execution_types`, which reaches the same three shapes
/// (`FlowInput`, `FlowInputContent`, `ModelPerformanceConfiguration`) — each shape is
/// declared exactly once across this service's `types/` modules, so this operation reads
/// them from there rather than redeclaring them.
/// The `flowOutputEvent` member of the response stream: one value that left the flow.
///
/// `nodeType` is in `unsupported = [..]` rather than declared: the model marks it
/// `@required`, and a served `InvokeFlow` event carries `nodeName` and `content` alone, so
/// declaring it would put a key on the wire the operation does not publish. The node's TYPE
/// is not information the event adds either — every value of this stream leaves at a node of
/// the one output kind — and the member is read from the flow's own definition by any caller
/// that wants it.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#FlowOutputEvent",
    unsupported = ["nodeType"]
)]
#[derive(Debug, Clone)]
pub struct FlowOutputEvent {
    pub node_name: String,
    pub content: FlowOutputContent,
}

/// The content of an output from a flow.
#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentruntime#FlowOutputContent")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum FlowOutputContent {
    Document(::aws_smithy_types::Document),
}

/// The `flowCompletionEvent` member of the response stream: why the flow completed.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#FlowCompletionEvent")]
#[derive(Debug, Clone, Default)]
pub struct FlowCompletionEvent {
    pub completion_reason: String,
}

/// The `flowTraceEvent` member of the response stream: one trace of the run, which a
/// request that asked for the trace is served beside its outputs.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#FlowTraceEvent")]
#[derive(Debug, Clone)]
pub struct FlowTraceEvent {
    pub trace: FlowTrace,
}

/// What one trace is about. The three members this service declares none of trace work no
/// node of a flow it can run does — a condition node's result, an action a node called and
/// a node's internal dependency — so they are in `unsupported = [..]`: the union carries
/// only the two kinds of trace this plane really produces.
#[smithy_macros::smithy(
    union_for = "com.amazonaws.bedrockagentruntime#FlowTrace",
    unsupported = ["conditionNodeResultTrace", "nodeActionTrace", "nodeDependencyTrace"]
)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum FlowTrace {
    nodeInputTrace(FlowTraceNodeInputEvent),
    nodeOutputTrace(FlowTraceNodeOutputEvent),
}

/// What entered one node of the flow, and when.
///
/// Its schema and codec come from [`session_schema_and_codec!`] rather than
/// `#[smithy_macros::smithy(shape = ...)]` because `timestamp` carries
/// `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="FlowTraceNodeInputEvent")`), which the derive does
/// not put on a member schema — see `types/date_time_wire.rs` for that whole story.
///
/// The member list below is the model's, member for member: same shape ids, same wire
/// names, same order, same shape types.
#[derive(Debug, Clone)]
pub struct FlowTraceNodeInputEvent {
    pub node_name: String,
    pub timestamp: ::aws_smithy_types::DateTime,
    pub fields: Vec<FlowTraceNodeInputField>,
}

session_schema_and_codec!(
    FlowTraceNodeInputEvent,
    FLOW_TRACE_NODE_INPUT_EVENT_SCHEMA,
    "com.amazonaws.bedrockagentruntime#FlowTraceNodeInputEvent",
    "com.amazonaws.bedrockagentruntime",
    "FlowTraceNodeInputEvent",
    (M_NODE_NAME, node_name, "nodeName", 0, req, string),
    (M_TIMESTAMP, timestamp, "timestamp", 1, req, date_time),
    (
        M_FIELDS,
        fields,
        "fields",
        2,
        req,
        list_of(FlowTraceNodeInputField)
    ),
);

/// One field of what entered a node: which input it arrived at, and the content that
/// arrived.
///
/// The four members this service declares none of describe work no node of a flow it can
/// run does — where the value came from, the declared data type and category of the field,
/// and the path through nested nodes — so they are in `unsupported = [..]` rather than sent
/// empty.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#FlowTraceNodeInputField",
    unsupported = ["source", "type", "category", "executionChain"]
)]
#[derive(Debug, Clone)]
pub struct FlowTraceNodeInputField {
    pub node_input_name: String,
    pub content: FlowTraceNodeInputContent,
}

/// The content of what entered a node.
#[smithy_macros::smithy(
    union_for = "com.amazonaws.bedrockagentruntime#FlowTraceNodeInputContent"
)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum FlowTraceNodeInputContent {
    Document(::aws_smithy_types::Document),
}

/// What left one node of the flow, and when. Its schema and codec come from
/// [`session_schema_and_codec!`] for the same reason [`FlowTraceNodeInputEvent`]'s do.
#[derive(Debug, Clone)]
pub struct FlowTraceNodeOutputEvent {
    pub node_name: String,
    pub timestamp: ::aws_smithy_types::DateTime,
    pub fields: Vec<FlowTraceNodeOutputField>,
}

session_schema_and_codec!(
    FlowTraceNodeOutputEvent,
    FLOW_TRACE_NODE_OUTPUT_EVENT_SCHEMA,
    "com.amazonaws.bedrockagentruntime#FlowTraceNodeOutputEvent",
    "com.amazonaws.bedrockagentruntime",
    "FlowTraceNodeOutputEvent",
    (M_NODE_NAME, node_name, "nodeName", 0, req, string),
    (M_TIMESTAMP, timestamp, "timestamp", 1, req, date_time),
    (
        M_FIELDS,
        fields,
        "fields",
        2,
        req,
        list_of(FlowTraceNodeOutputField)
    ),
);

/// One field of what left a node: which output it left by, and the content that left.
///
/// `next` and `type` are in `unsupported = [..]` for the reason
/// [`FlowTraceNodeInputField`]'s absent members are.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#FlowTraceNodeOutputField",
    unsupported = ["next", "type"]
)]
#[derive(Debug, Clone)]
pub struct FlowTraceNodeOutputField {
    pub node_output_name: String,
    pub content: FlowTraceNodeOutputContent,
}

/// The content of what left a node.
#[smithy_macros::smithy(
    union_for = "com.amazonaws.bedrockagentruntime#FlowTraceNodeOutputContent"
)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum FlowTraceNodeOutputContent {
    Document(::aws_smithy_types::Document),
}
