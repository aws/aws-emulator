// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_assistant` operation, HAND-AUTHORED: the
//! op-stub generator declined `InvokeAssistant` because its output carries a `@streaming`
//! event-stream member it cannot frame (`../SKIPPED_STUBS.md`), and an operation with no
//! types and no handler is not in the service's routing table at all — so nothing served
//! `POST /assistant/invocation`.
//!
//! The split is the one the other hand-authored streaming surfaces in this workspace use:
//! the members the framework's own codec can read are declared, so this operation's
//! `@required`, `@length` and `@pattern` members are checked against the MODEL rather than
//! by hand, and the event stream sits in `unsupported = [..]`, which keeps it out of the
//! schema. The EVENTS the stream carries are declared here too, because the handler frames
//! them itself.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iotsitewise-awsiotsitewise.json");

/// `InvokeAssistantRequest`: the message of this turn, the conversation it belongs to and
/// whether the stream carries the assistant's trace.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#InvokeAssistantRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeAssistantInput {
    pub conversation_id: Option<String>,
    pub message: String,
    pub enable_trace: bool,
}

/// `InvokeAssistantResponse`: `body` is the `@streaming` `@httpPayload`, which the handler
/// frames itself, so the declared half of the output carries only the `conversationId` the
/// turn was recorded under — the member the model binds to a response HEADER.
#[smithy_macros::smithy(
    shape = "com.amazonaws.iotsitewise#InvokeAssistantResponse",
    unsupported = ["body"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeAssistantOutput {
    pub conversation_id: String,
}

/// The `trace` member of the response stream: one step of the assistant's reasoning and
/// data access.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#Trace")]
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub text: Option<String>,
}

/// The `output` member of the response stream: the assistant's answer and what it cites.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#InvocationOutput")]
#[derive(Debug, Clone, Default)]
pub struct InvocationOutput {
    pub message: Option<String>,
    pub citations: Option<Vec<Citation>>,
}

/// One citation of an answer: the text cited and the source it was read from.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#Citation")]
#[derive(Debug, Clone, Default)]
pub struct Citation {
    pub reference: Option<Reference>,
    pub content: Option<Content>,
}

/// What a citation refers to.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#Reference")]
#[derive(Debug, Clone, Default)]
pub struct Reference {
    pub dataset: Option<DataSetReference>,
}

/// The dataset a citation refers to, and the source that dataset's data came from.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#DataSetReference")]
#[derive(Debug, Clone, Default)]
pub struct DataSetReference {
    pub dataset_arn: Option<String>,
    pub source: Option<Source>,
}

/// The source a cited dataset's data came from.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#Source")]
#[derive(Debug, Clone, Default)]
pub struct Source {
    pub arn: Option<String>,
    pub location: Option<Location>,
}

/// Where the cited text is originally stored.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#Location")]
#[derive(Debug, Clone, Default)]
pub struct Location {
    pub uri: Option<String>,
}

/// The cited text itself.
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#Content")]
#[derive(Debug, Clone, Default)]
pub struct Content {
    pub text: Option<String>,
}
