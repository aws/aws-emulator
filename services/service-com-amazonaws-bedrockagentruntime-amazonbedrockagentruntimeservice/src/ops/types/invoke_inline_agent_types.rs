// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_inline_agent` operation, HAND-AUTHORED:
//! the op-stub generator declined `InvokeInlineAgent` because its output carries a
//! `@streaming` event-stream member it cannot frame (`../SKIPPED_STUBS.md`), and an
//! operation with no types and no handler is not in the service's routing table at all —
//! so nothing validated this operation's own request.
//!
//! The split follows the one the other hand-authored streaming surfaces in this tree use
//! (`agentic_retrieve_stream_types.rs`): the members the framework's own codec can read are
//! declared, so this operation's `@required`, `@length`, `@range`, `@pattern` and ENUM
//! members are checked against the MODEL rather than by hand, and the event stream sits in
//! `unsupported = [..]`, which keeps it out of the schema. The EVENT the stream carries is
//! declared here, because the handler frames it itself.
//!
//! Every configuration the handler has to REFUSE or report on is declared rather than left
//! unsupported: a member the schema drops is one the handler cannot see, and a request
//! asking for work this emulator performs none of has to be refused rather than served with
//! the member ignored. Where only the PRESENCE of such a configuration decides what the
//! handler does, its deeper members are unsupported — that is what each
//! `unsupported = [..]` list below is.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `InvokeInlineAgentRequest`: the `sessionId` URI label, the model and instruction the
/// inline agent is composed from, and every configuration the request may carry.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#InvokeInlineAgentRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeInlineAgentInput {
    pub session_id: String,
    pub foundation_model: String,
    pub instruction: String,
    pub customer_encryption_key_arn: Option<String>,
    pub idle_session_ttl_in_seconds: Option<i32>,
    pub agent_collaboration: Option<String>,
    pub agent_name: Option<String>,
    pub end_session: Option<bool>,
    pub enable_trace: Option<bool>,
    pub input_text: Option<String>,
    pub orchestration_type: Option<String>,
    pub action_groups: Option<Vec<AgentActionGroup>>,
    pub knowledge_bases: Option<Vec<KnowledgeBase>>,
    pub guardrail_configuration: Option<GuardrailConfigurationWithArn>,
    pub prompt_override_configuration: Option<PromptOverrideConfiguration>,
    pub collaborator_configurations: Option<Vec<CollaboratorConfiguration>>,
    pub collaborators: Option<Vec<Collaborator>>,
    pub inline_session_state: Option<InlineSessionState>,
    pub streaming_configurations: Option<StreamingConfigurations>,
    pub prompt_creation_configurations: Option<PromptCreationConfigurations>,
    pub bedrock_model_configurations: Option<InlineBedrockModelConfigurations>,
    pub custom_orchestration: Option<CustomOrchestration>,
}

/// The header half of `InvokeInlineAgentResponse`; `completion` is the `@streaming`
/// `@httpPayload`, which the handler frames itself, so the declared half carries the two
/// modelled response headers.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InvokeInlineAgentResponse",
    unsupported = ["completion"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeInlineAgentOutput {
    pub content_type: String,
    pub session_id: String,
}

/// One action group of the inline agent. The three members that say HOW an action is
/// carried out — `actionGroupExecutor`, `apiSchema`, `functionSchema` — and the computer-use
/// parameters are unsupported: an action is carried out only once a model elicits one, and
/// no model runs here, so the handler reads the action group's identity and nothing deeper.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgentActionGroup",
    unsupported = [
        "actionGroupExecutor",
        "apiSchema",
        "functionSchema",
        "parentActionGroupSignatureParams"
    ]
)]
#[derive(Debug, Clone, Default)]
pub struct AgentActionGroup {
    pub action_group_name: String,
    pub description: Option<String>,
    pub parent_action_group_signature: Option<String>,
}

/// One knowledge base the inline agent may query.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#KnowledgeBase")]
#[derive(Debug, Clone, Default)]
pub struct KnowledgeBase {
    pub knowledge_base_id: String,
    pub description: String,
    pub retrieval_configuration: Option<KnowledgeBaseRetrievalConfiguration>,
}

/// The guardrail an inline agent invocation asks to be governed by.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#GuardrailConfigurationWithArn")]
#[derive(Debug, Clone, Default)]
pub struct GuardrailConfigurationWithArn {
    pub guardrail_identifier: String,
    pub guardrail_version: String,
}

/// The prompt templates an invocation overrides. `promptConfigurations` is unsupported: the
/// handler only has to see THAT a request overrides a prompt, because no prompt is read
/// here, so which part of the sequence is overridden changes nothing it could answer.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#PromptOverrideConfiguration",
    unsupported = ["promptConfigurations"]
)]
#[derive(Debug, Clone, Default)]
pub struct PromptOverrideConfiguration {
    pub override_lambda: Option<String>,
}

/// The settings of one inline collaborator agent.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#CollaboratorConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct CollaboratorConfiguration {
    pub collaborator_name: String,
    pub collaborator_instruction: String,
    pub agent_alias_arn: Option<String>,
    pub relay_conversation_history: Option<String>,
}

/// One inline collaborator agent of a supervisor invocation.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#Collaborator")]
#[derive(Debug, Clone, Default)]
pub struct Collaborator {
    pub customer_encryption_key_arn: Option<String>,
    pub foundation_model: String,
    pub instruction: String,
    pub idle_session_ttl_in_seconds: Option<i32>,
    pub action_groups: Option<Vec<AgentActionGroup>>,
    pub knowledge_bases: Option<Vec<KnowledgeBase>>,
    pub guardrail_configuration: Option<GuardrailConfigurationWithArn>,
    pub prompt_override_configuration: Option<PromptOverrideConfiguration>,
    pub agent_collaboration: Option<String>,
    pub collaborator_configurations: Option<Vec<CollaboratorConfiguration>>,
    pub agent_name: Option<String>,
}

/// `InlineSessionState`, declared for its PRESENCE alone: every member of it is
/// unsupported, because what a session state does — replace the placeholders of a prompt
/// nothing reads, answer an action-group invocation this service returned control for
/// nowhere, carry files into a code interpreter, supply a conversation history — is work
/// nothing in this emulator performs, so a request carrying one is refused rather than
/// served with the member ignored.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InlineSessionState",
    unsupported = [
        "sessionAttributes",
        "promptSessionAttributes",
        "returnControlInvocationResults",
        "invocationId",
        "files",
        "conversationHistory"
    ]
)]
#[derive(Debug, Clone, Default)]
pub struct InlineSessionState {}

/// `InlineBedrockModelConfigurations`: the settings for the model the inline agent calls.
/// Its one member names a latency profile, which does not change what an answer says.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InlineBedrockModelConfigurations"
)]
#[derive(Debug, Clone, Default)]
pub struct InlineBedrockModelConfigurations {
    pub performance_config: Option<PerformanceConfiguration>,
}

/// `CustomOrchestration`, declared for its PRESENCE alone: its `executor` names the
/// function that would drive the orchestration, and nothing orchestrates here, so the
/// handler only has to see that a request configured one.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#CustomOrchestration",
    unsupported = ["executor"]
)]
#[derive(Debug, Clone, Default)]
pub struct CustomOrchestration {}

/// The `chunk` member of the inline agent's response stream: one part of the agent's
/// answer, and the citations for it.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#InlineAgentPayloadPart")]
#[derive(Debug, Clone, Default)]
pub struct InlineAgentPayloadPart {
    pub bytes: Option<Vec<u8>>,
    pub attribution: Option<Attribution>,
}
