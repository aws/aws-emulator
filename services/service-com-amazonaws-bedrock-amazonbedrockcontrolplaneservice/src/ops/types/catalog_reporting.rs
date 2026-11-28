// SPDX-License-Identifier: CC0-1.0

//! The CAPABILITY records a foundation model listing and a foundation model read report
//! beside the members the on-disk shapes declare.
//!
//! WHY THIS MODULE EXISTS, AND WHY IT IS HAND-WRITTEN. Amazon Bedrock answers
//! `ListFoundationModels` and `GetFoundationModel` with the capabilities of each model —
//! the family it belongs to, the invocation APIs it is reachable through, the features
//! published for it, and what a conversation with it may carry — and the on-disk model
//! declares none of them: `smithy_query(query="shape", shape="FoundationModelSummary")`
//! lists modelArn/modelId/modelName/providerName/inputModalities/outputModalities/
//! responseStreamingSupported/customizationsSupported/inferenceTypesSupported/
//! modelLifecycle and nothing else, and no shape of the model is named for one of the
//! records below. A client decides from them whether it may stream, converse, guard,
//! cache, route, batch or customise a model it picked out of the listing, so a listing
//! that omits them is undecidable — which is why this service reports them.
//!
//! A shape the model does not declare cannot be rendered from it, so these structures are
//! written out here: the member list, the schema and the codec, through
//! [`bedrock_schema_and_codec!`] (`types/date_time_wire.rs`), which is the same macro every
//! other hand-written wire shape of this crate takes. The module sits at a path the scaffold
//! op-stub generator does not write and carries no `@generated` marker, so a refreshed model
//! neither overwrites nor deletes it; `types/mod.rs` declares it beside the rendered modules
//! and re-exports it, so each name is reached as `crate::ops::types::…` like every other
//! shape.
//!
//! WHAT FILLS THEM is read off the published catalog row and nowhere else —
//! `crate::state::foundation_model_summary` and the functions beside it — so a capability is
//! a fact about the model rather than a value invented per request, and the listing and the
//! read report one and the same mapping.
//!
//! DELETE A SHAPE FROM HERE when a refreshed model declares it: the rendered declaration is
//! then the one to keep, and the member list, the constraints and the codec all come from the
//! model again.

#![allow(unused_imports)]

use super::*;

/// One capability of a model as a listing reports it: the record naming whether that
/// capability may be used with the model.
///
/// A record rather than a bare boolean because that is the form Amazon Bedrock reports a
/// per-model capability in, which leaves room for what else is published about it;
/// `crate::state::capability_support` builds one.
#[derive(Debug, Clone, Default)]
pub struct CapabilitySupport {
    pub is_supported: Option<bool>,
}

bedrock_schema_and_codec!(
    CapabilitySupport,
    CAPABILITY_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#CapabilitySupport",
    "com.amazonaws.bedrock",
    "CapabilitySupport",
    (M_IS_SUPPORTED, is_supported, "isSupported", 0, opt, boolean),
);

/// The calling modes the Converse API offers for a model: it answers a turn in one piece,
/// and it streams that answer back.
#[derive(Debug, Clone, Default)]
pub struct ConverseApiSupport {
    pub streaming: Option<bool>,
    pub sync: Option<bool>,
}

bedrock_schema_and_codec!(
    ConverseApiSupport,
    CONVERSE_API_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#ConverseApiSupport",
    "com.amazonaws.bedrock",
    "ConverseApiSupport",
    (M_STREAMING, streaming, "streaming", 0, opt, boolean),
    (M_SYNC, sync, "sync", 1, opt, boolean),
);

/// The calling modes the `InvokeModel` family offers for a model: it answers in one piece,
/// it streams an answer back, and it carries a stream in both directions.
#[derive(Debug, Clone, Default)]
pub struct InvokeModelApiSupport {
    pub bidirectional_streaming: Option<bool>,
    pub response_streaming: Option<bool>,
    pub sync: Option<bool>,
}

bedrock_schema_and_codec!(
    InvokeModelApiSupport,
    INVOKE_MODEL_API_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#InvokeModelApiSupport",
    "com.amazonaws.bedrock",
    "InvokeModelApiSupport",
    (
        M_BIDIRECTIONAL_STREAMING,
        bidirectional_streaming,
        "bidirectionalStreaming",
        0,
        opt,
        boolean
    ),
    (
        M_RESPONSE_STREAMING,
        response_streaming,
        "responseStreaming",
        1,
        opt,
        boolean
    ),
    (M_SYNC, sync, "sync", 2, opt, boolean),
);

/// The invocation APIs a model is reachable through: one member per API, each naming the
/// calling modes that API offers FOR THIS MODEL, beside the two OpenAI-compatible endpoints
/// Amazon Bedrock serves a published set of models on.
#[derive(Debug, Clone, Default)]
pub struct InferenceApisSupport {
    pub async_invoke: Option<bool>,
    pub converse: Option<ConverseApiSupport>,
    pub invoke_model: Option<InvokeModelApiSupport>,
    pub open_ai_chat_completions: Option<bool>,
    pub open_ai_responses: Option<bool>,
}

bedrock_schema_and_codec!(
    InferenceApisSupport,
    INFERENCE_APIS_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#InferenceApisSupport",
    "com.amazonaws.bedrock",
    "InferenceApisSupport",
    (M_ASYNC_INVOKE, async_invoke, "asyncInvoke", 0, opt, boolean),
    (
        M_CONVERSE,
        converse,
        "converse",
        1,
        opt,
        struct_of(ConverseApiSupport)
    ),
    (
        M_INVOKE_MODEL,
        invoke_model,
        "invokeModel",
        2,
        opt,
        struct_of(InvokeModelApiSupport)
    ),
    (
        M_OPEN_AI_CHAT_COMPLETIONS,
        open_ai_chat_completions,
        "openAiChatCompletions",
        3,
        opt,
        boolean
    ),
    (
        M_OPEN_AI_RESPONSES,
        open_ai_responses,
        "openAiResponses",
        4,
        opt,
        boolean
    ),
);

/// Whether a model invocation job may run a model over a dataset, per REGION SCOPE: batch
/// inference publishes one supported-models table per scope, so a model one table lists is
/// not thereby listed by the other.
#[derive(Debug, Clone, Default)]
pub struct BatchInferenceSupport {
    pub cross_region: Option<bool>,
    pub in_region: Option<bool>,
}

bedrock_schema_and_codec!(
    BatchInferenceSupport,
    BATCH_INFERENCE_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#BatchInferenceSupport",
    "com.amazonaws.bedrock",
    "BatchInferenceSupport",
    (M_CROSS_REGION, cross_region, "crossRegion", 0, opt, boolean),
    (M_IN_REGION, in_region, "inRegion", 1, opt, boolean),
);

/// The Amazon Bedrock features published for a model, one member per feature, so a client
/// tests the one it is about to use by name.
#[derive(Debug, Clone, Default)]
pub struct FeaturesSupport {
    pub batch_inference: Option<BatchInferenceSupport>,
    pub count_tokens: Option<bool>,
    pub intelligent_prompt_routing: Option<bool>,
    pub latency_optimization: Option<bool>,
    pub prompt_caching: Option<bool>,
    pub prompt_optimization: Option<bool>,
    pub provisioned_throughput: Option<bool>,
}

bedrock_schema_and_codec!(
    FeaturesSupport,
    FEATURES_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#FeaturesSupport",
    "com.amazonaws.bedrock",
    "FeaturesSupport",
    (
        M_BATCH_INFERENCE,
        batch_inference,
        "batchInference",
        0,
        opt,
        struct_of(BatchInferenceSupport)
    ),
    (M_COUNT_TOKENS, count_tokens, "countTokens", 1, opt, boolean),
    (
        M_INTELLIGENT_PROMPT_ROUTING,
        intelligent_prompt_routing,
        "intelligentPromptRouting",
        2,
        opt,
        boolean
    ),
    (
        M_LATENCY_OPTIMIZATION,
        latency_optimization,
        "latencyOptimization",
        3,
        opt,
        boolean
    ),
    (
        M_PROMPT_CACHING,
        prompt_caching,
        "promptCaching",
        4,
        opt,
        boolean
    ),
    (
        M_PROMPT_OPTIMIZATION,
        prompt_optimization,
        "promptOptimization",
        5,
        opt,
        boolean
    ),
    (
        M_PROVISIONED_THROUGHPUT,
        provisioned_throughput,
        "provisionedThroughput",
        6,
        opt,
        boolean
    ),
);

/// Whether a model invocation job may run a model over a dataset, as a listing reports it: a
/// job runs the base model, runs it from another region, and runs a CUSTOMIZATION of it, and
/// each of the three is its own answer.
#[derive(Debug, Clone, Default)]
pub struct BatchSupport {
    pub base_model_supported: Option<bool>,
    pub cross_region_supported: Option<bool>,
    pub custom_model_supported: Option<bool>,
}

bedrock_schema_and_codec!(
    BatchSupport,
    BATCH_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#BatchSupport",
    "com.amazonaws.bedrock",
    "BatchSupport",
    (
        M_BASE_MODEL_SUPPORTED,
        base_model_supported,
        "baseModelSupported",
        0,
        opt,
        boolean
    ),
    (
        M_CROSS_REGION_SUPPORTED,
        cross_region_supported,
        "crossRegionSupported",
        1,
        opt,
        boolean
    ),
    (
        M_CUSTOM_MODEL_SUPPORTED,
        custom_model_supported,
        "customModelSupported",
        2,
        opt,
        boolean
    ),
);

/// Whether EXTENDED REASONING may be asked of a model, and whether that reasoning arrives
/// EMBEDDED in the answer text rather than in a block of its own.
#[derive(Debug, Clone, Default)]
pub struct ReasoningSupport {
    pub is_supported: Option<bool>,
    pub embedded: Option<bool>,
}

bedrock_schema_and_codec!(
    ReasoningSupport,
    REASONING_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#ReasoningSupport",
    "com.amazonaws.bedrock",
    "ReasoningSupport",
    (M_IS_SUPPORTED, is_supported, "isSupported", 0, opt, boolean),
    (M_EMBEDDED, embedded, "embedded", 1, opt, boolean),
);

/// What the Converse API supports for a model: what a turn may carry to it, how large an
/// answer may be asked of it, and whether reasoning may be asked of it.
///
/// Reported for a model that answers a WRITTEN turn of a conversation and for no other, which
/// is `crate::state::model_converse_support`.
#[derive(Debug, Clone, Default)]
pub struct ConverseSupport {
    pub supported: Option<bool>,
    pub streaming_supported: Option<bool>,
    pub additional_request_fields_schema: Option<String>,
    pub max_tokens_default: Option<i32>,
    pub max_tokens_maximum: Option<i32>,
    pub reasoning_supported: Option<ReasoningSupport>,
    pub stop_sequences_default: Option<Vec<String>>,
    pub system_role_supported: Option<bool>,
    pub user_document_types_supported: Option<Vec<String>>,
    pub user_image_types_supported: Option<Vec<String>>,
    pub user_video_types_supported: Option<Vec<String>>,
}

bedrock_schema_and_codec!(
    ConverseSupport,
    CONVERSE_SUPPORT_SCHEMA,
    "com.amazonaws.bedrock#ConverseSupport",
    "com.amazonaws.bedrock",
    "ConverseSupport",
    (M_SUPPORTED, supported, "supported", 0, opt, boolean),
    (
        M_STREAMING_SUPPORTED,
        streaming_supported,
        "streamingSupported",
        1,
        opt,
        boolean
    ),
    (
        M_ADDITIONAL_REQUEST_FIELDS_SCHEMA,
        additional_request_fields_schema,
        "additionalRequestFieldsSchema",
        2,
        opt,
        string
    ),
    (
        M_MAX_TOKENS_DEFAULT,
        max_tokens_default,
        "maxTokensDefault",
        3,
        opt,
        integer
    ),
    (
        M_MAX_TOKENS_MAXIMUM,
        max_tokens_maximum,
        "maxTokensMaximum",
        4,
        opt,
        integer
    ),
    (
        M_REASONING_SUPPORTED,
        reasoning_supported,
        "reasoningSupported",
        5,
        opt,
        struct_of(ReasoningSupport)
    ),
    (
        M_STOP_SEQUENCES_DEFAULT,
        stop_sequences_default,
        "stopSequencesDefault",
        6,
        opt,
        list_of_string
    ),
    (
        M_SYSTEM_ROLE_SUPPORTED,
        system_role_supported,
        "systemRoleSupported",
        7,
        opt,
        boolean
    ),
    (
        M_USER_DOCUMENT_TYPES_SUPPORTED,
        user_document_types_supported,
        "userDocumentTypesSupported",
        8,
        opt,
        list_of_string
    ),
    (
        M_USER_IMAGE_TYPES_SUPPORTED,
        user_image_types_supported,
        "userImageTypesSupported",
        9,
        opt,
        list_of_string
    ),
    (
        M_USER_VIDEO_TYPES_SUPPORTED,
        user_video_types_supported,
        "userVideoTypesSupported",
        10,
        opt,
        list_of_string
    ),
);
