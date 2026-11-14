// SPDX-License-Identifier: CC0-1.0

//! The base models this data plane resolves a `modelId` against: the catalogue itself.
//!
//! IDENTIFIER RESOLUTION IS NOT WHERE ENVELOPE COVERAGE IS DECIDED. Amazon Bedrock
//! publishes its base models rather than letting a caller create one, and every model it
//! publishes for on-demand inference is one an account may invoke — so a `modelId` naming
//! one resolves and is served, and only an identifier naming no published model is answered
//! as an identifier the service cannot interpret
//! ([`super::INVALID_MODEL_IDENTIFIER_MESSAGE`]). The control plane of this same emulator
//! publishes that catalogue through `ListFoundationModels` and `GetFoundationModel`, and
//! the two planes carry the same model ids, so a model a client finds in the listing is one
//! it can invoke.
//!
//! What a model is served WITH is decided per row, by how it is [`Served`]:
//!
//! * a TEXT model answers in its provider's own `InvokeModel` body envelope
//!   ([`super::invoke_model_response_body`]) and through the Converse API, whose
//!   request and response shapes are the SERVICE's own and therefore the same for every
//!   model it serves;
//! * a model of another modality (an embedding vector, an image) is recognised and refused
//!   with the modelled model-processing error rather than answered with a fabricated
//!   payload;
//! * a video generation model is served by `StartAsyncInvoke` alone, which is the surface
//!   AWS publishes asynchronous invocation on;
//! * a model past its end of life keeps a recognised identifier and serves no inference
//!   ("Model lifecycle" in the Amazon Bedrock User Guide), which is a 404 on a model that
//!   existed rather than a 400 on an unreadable identifier. Such a model is no longer
//!   OFFERED, so the control plane's catalogue no longer carries it while this inventory
//!   does — the refusal belongs to the plane that would serve the inference.
//!
//! Sources: the ids and modalities AWS publishes in "Amazon Bedrock base model IDs
//! (on-demand throughput)" and "Supported foundation models in Amazon Bedrock", the retired
//! ones in "Model lifecycle", and each provider's own request/response format in "Inference
//! request parameters and response fields for foundation models" for the family a row names.

use super::{ModelEntry, ModelFamily, ModelLifecycle, ModelModality};

use ModelFamily::{AnthropicMessages, CohereCommand, Llama, Mistral, Nova, OpenAiChat, TitanText};
use ModelModality::{Embedding, Image, Text};

/// How one catalogued model is served.
#[derive(Debug, Clone, Copy)]
pub(super) enum Served {
    /// A text model, answered in `family`'s own `InvokeModel` envelope. `converse` is
    /// whether the Converse API serves it too — not every base model is served by it
    /// ("Supported models and model features" lists the Cohere Command text models as
    /// `InvokeModel`-only) — and `max_output_tokens` is the model's own ceiling on
    /// generated tokens, which caps the generation budget a request asks for.
    Text {
        family: ModelFamily,
        max_output_tokens: u32,
        converse: bool,
    },
    /// A recognised model whose modality this emulator computes no inference for.
    Unserved(ModelModality),
    /// A video generation model: served by `StartAsyncInvoke` alone, since the result of an
    /// asynchronous invocation is an artifact written to Amazon S3 rather than a response
    /// body. Every id carried this way needs a `modelInput` schema in
    /// [`super::async_model_input_rules`], which this crate's tests pin against the
    /// inventory.
    AsyncVideo,
    /// A model past its end of life: the identifier resolves, the model serves nothing.
    Retired(ModelModality),
}

/// A text model served in its family's envelope and by the Converse API.
const fn text(family: ModelFamily, max_output_tokens: u32) -> Served {
    Served::Text {
        family,
        max_output_tokens,
        converse: true,
    }
}

/// A text model the Converse API does not serve, so only the invoke surfaces answer for it.
const fn invoke_only(family: ModelFamily, max_output_tokens: u32) -> Served {
    Served::Text {
        family,
        max_output_tokens,
        converse: false,
    }
}

/// The generation ceiling each family's provider publishes for the line this catalogue
/// carries ("Inference request parameters and response fields for foundation models", per
/// provider): the cap a request's own generation budget is clamped to, since a model cannot
/// generate past its own ceiling however large a budget a caller allows.
const ANTHROPIC_CLAUDE_3_MAX_TOKENS: u32 = 4096;
const ANTHROPIC_MAX_TOKENS: u32 = 8192;
const NOVA_MAX_TOKENS: u32 = 5120;
const TITAN_TEXT_MAX_TOKENS: u32 = 4096;
const TITAN_TEXT_PREMIER_MAX_TOKENS: u32 = 3072;
const LLAMA_MAX_TOKENS: u32 = 2048;
const MISTRAL_MAX_TOKENS: u32 = 8192;
const MIXTRAL_MAX_TOKENS: u32 = 4096;
const COHERE_MAX_TOKENS: u32 = 4096;
const OPENAI_CHAT_MAX_TOKENS: u32 = 4096;

/// The CONTEXT WINDOW each family's provider publishes for the line this catalogue
/// carries: the most input tokens one request may send the model ("Supported foundation
/// models in Amazon Bedrock", the per-model context-window column). It bounds the INPUT a
/// request carries, where [`ANTHROPIC_MAX_TOKENS`] and the constants beside it bound the
/// OUTPUT one generation may produce, so an input past it is refused rather than served or
/// shortened.
const ANTHROPIC_CONTEXT_WINDOW: u32 = 200_000;
const NOVA_CONTEXT_WINDOW: u32 = 300_000;
const NOVA_MICRO_CONTEXT_WINDOW: u32 = 128_000;
const NOVA_PREMIER_CONTEXT_WINDOW: u32 = 1_000_000;
const TITAN_TEXT_PREMIER_CONTEXT_WINDOW: u32 = 32_000;
const TITAN_TEXT_LITE_CONTEXT_WINDOW: u32 = 4_000;
const LLAMA_CONTEXT_WINDOW: u32 = 128_000;
const LLAMA_3_CONTEXT_WINDOW: u32 = 8_000;
const MISTRAL_CONTEXT_WINDOW: u32 = 128_000;
const MISTRAL_7B_CONTEXT_WINDOW: u32 = 32_000;
const COHERE_COMMAND_R_CONTEXT_WINDOW: u32 = 128_000;
const COHERE_COMMAND_TEXT_CONTEXT_WINDOW: u32 = 4_000;
const OPENAI_CHAT_CONTEXT_WINDOW: u32 = 128_000;

/// The lines whose published context window is NOT their family's current one, by the
/// prefix of the model ids that line is published under — the earlier generation of a
/// family whose newer models read a longer prompt, and the small member of a family whose
/// larger models read a longer one. The longest window of a family is its default
/// ([`family_context_window`]), so a row this table does not name reads what its family
/// publishes.
const CONTEXT_WINDOW_OVERRIDES: &[(&str, u32)] = &[
    ("amazon.nova-micro", NOVA_MICRO_CONTEXT_WINDOW),
    ("amazon.nova-premier", NOVA_PREMIER_CONTEXT_WINDOW),
    ("amazon.titan-text-lite", TITAN_TEXT_LITE_CONTEXT_WINDOW),
    ("meta.llama3-8b", LLAMA_3_CONTEXT_WINDOW),
    ("meta.llama3-70b", LLAMA_3_CONTEXT_WINDOW),
    ("mistral.mistral-7b", MISTRAL_7B_CONTEXT_WINDOW),
    ("mistral.mixtral-8x7b", MISTRAL_7B_CONTEXT_WINDOW),
    ("cohere.command-text", COHERE_COMMAND_TEXT_CONTEXT_WINDOW),
    ("cohere.command-light-text", COHERE_COMMAND_TEXT_CONTEXT_WINDOW),
];

/// The context window of the line a family's current models belong to.
const fn family_context_window(family: ModelFamily) -> u32 {
    match family {
        AnthropicMessages => ANTHROPIC_CONTEXT_WINDOW,
        Nova => NOVA_CONTEXT_WINDOW,
        TitanText => TITAN_TEXT_PREMIER_CONTEXT_WINDOW,
        Llama => LLAMA_CONTEXT_WINDOW,
        Mistral => MISTRAL_CONTEXT_WINDOW,
        CohereCommand => COHERE_COMMAND_R_CONTEXT_WINDOW,
        OpenAiChat => OPENAI_CHAT_CONTEXT_WINDOW,
    }
}

/// The context window of one catalogued model: the window published for its own line where
/// [`CONTEXT_WINDOW_OVERRIDES`] names it, and its family's otherwise.
fn context_window(model_id: &str, family: ModelFamily) -> u32 {
    CONTEXT_WINDOW_OVERRIDES
        .iter()
        .find(|(prefix, _)| model_id.starts_with(prefix))
        .map_or_else(|| family_context_window(family), |(_, window)| *window)
}

/// Every base model id this data plane recognises, with how each one is served.
///
/// Grouped by provider, as the published model tables group them. A provider whose own
/// `InvokeModel` body format this emulator renders carries that family; a provider Amazon
/// Bedrock serves through the OpenAI-compatible chat completions schema — the shape those
/// providers' own APIs publish — carries [`ModelFamily::OpenAiChat`].
pub(super) const CATALOGUE: &[(&str, Served)] = &[
    // AI21 Labs.
    ("ai21.jamba-1-5-mini-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Amazon Nova understanding models, including the Nova 2 generation.
    ("amazon.nova-2-5-sonic", text(Nova, NOVA_MAX_TOKENS)),
    ("amazon.nova-2-lite-v1:0", text(Nova, NOVA_MAX_TOKENS)),
    ("amazon.nova-2-sonic-v1:0", text(Nova, NOVA_MAX_TOKENS)),
    ("amazon.nova-micro-v1:0", text(Nova, NOVA_MAX_TOKENS)),
    ("amazon.nova-lite-v1:0", text(Nova, NOVA_MAX_TOKENS)),
    ("amazon.nova-pro-v1:0", text(Nova, NOVA_MAX_TOKENS)),
    ("amazon.nova-premier-v1:0", text(Nova, NOVA_MAX_TOKENS)),
    // Amazon Titan Text. Titan Text Express is past its end of life (below).
    ("amazon.titan-text-lite-v1", text(TitanText, TITAN_TEXT_MAX_TOKENS)),
    ("amazon.titan-text-lite-v1:0:4k", text(TitanText, TITAN_TEXT_MAX_TOKENS)),
    ("amazon.titan-text-premier-v1:0", text(TitanText, TITAN_TEXT_PREMIER_MAX_TOKENS)),
    // Anthropic Claude: the dated Claude 3 models at that line's ceiling, every later
    // line at the current one.
    ("anthropic.claude-3-haiku-20240307-v1:0", text(AnthropicMessages, ANTHROPIC_CLAUDE_3_MAX_TOKENS)),
    ("anthropic.claude-3-sonnet-20240229-v1:0", text(AnthropicMessages, ANTHROPIC_CLAUDE_3_MAX_TOKENS)),
    ("anthropic.claude-3-opus-20240229-v1:0", text(AnthropicMessages, ANTHROPIC_CLAUDE_3_MAX_TOKENS)),
    ("anthropic.claude-3-5-haiku-20241022-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-3-5-sonnet-20240620-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-3-5-sonnet-20241022-v2:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-3-7-sonnet-20250219-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-fable-5", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-fable-5-1", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-haiku-4-5-20251001-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-4-1-20250805-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-4-5-20251101-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-4-6-v1", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-4-7", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-4-8", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-5", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-opus-5-5", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-sonnet-4-20250514-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-sonnet-4-5-20250929-v1:0", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-sonnet-4-6", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-sonnet-5", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    ("anthropic.claude-sonnet-5-5", text(AnthropicMessages, ANTHROPIC_MAX_TOKENS)),
    // Cohere Command: the text models are InvokeModel-only, the R models are served by
    // Converse too.
    ("cohere.command-text-v14", invoke_only(CohereCommand, COHERE_MAX_TOKENS)),
    ("cohere.command-text-v14:7:4k", invoke_only(CohereCommand, COHERE_MAX_TOKENS)),
    ("cohere.command-light-text-v14", invoke_only(CohereCommand, COHERE_MAX_TOKENS)),
    ("cohere.command-light-text-v14:7:4k", invoke_only(CohereCommand, COHERE_MAX_TOKENS)),
    ("cohere.command-r-v1:0", text(CohereCommand, COHERE_MAX_TOKENS)),
    ("cohere.command-r-plus-v1:0", text(CohereCommand, COHERE_MAX_TOKENS)),
    // DeepSeek.
    ("deepseek.r1-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("deepseek.v3-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("deepseek.v3.2", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Google Gemma.
    ("google.gemma-3-12b-it", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("google.gemma-3-27b-it", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("google.gemma-3-4b-it", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Meta Llama 3 and later.
    ("meta.llama3-8b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-70b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-1-8b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-1-70b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-1-405b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-2-1b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-2-3b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-2-11b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-2-90b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama3-3-70b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama4-maverick-17b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    ("meta.llama4-scout-17b-instruct-v1:0", text(Llama, LLAMA_MAX_TOKENS)),
    // MiniMax.
    ("minimax.minimax-m2", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("minimax.minimax-m2.1", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("minimax.minimax-m2.5", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Mistral AI.
    ("mistral.mistral-7b-instruct-v0:2", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.mixtral-8x7b-instruct-v0:1", text(Mistral, MIXTRAL_MAX_TOKENS)),
    ("mistral.mistral-small-2402-v1:0", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.mistral-large-2402-v1:0", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.mistral-large-2407-v1:0", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.mistral-large-3-675b-instruct", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.devstral-2-123b", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.magistral-small-2509", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.ministral-3-3b-instruct", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.ministral-3-8b-instruct", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.ministral-3-14b-instruct", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.pixtral-large-2502-v1:0", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.voxtral-mini-3b-2507", text(Mistral, MISTRAL_MAX_TOKENS)),
    ("mistral.voxtral-small-24b-2507", text(Mistral, MISTRAL_MAX_TOKENS)),
    // Moonshot AI.
    ("moonshot.kimi-k2-thinking", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("moonshotai.kimi-k2.5", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("moonshotai.kimi-k3", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // NVIDIA Nemotron.
    ("nvidia.nemotron-nano-9b-v2", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("nvidia.nemotron-nano-12b-v2", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("nvidia.nemotron-nano-3-30b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("nvidia.nemotron-super-3-120b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // OpenAI, whose own OpenAI-compatible endpoints Amazon Bedrock serves its models on.
    ("openai.gpt-5.4", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-5.5", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-5.6-luna", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-5.6-sol", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-5.6-terra", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-6-astra", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-6-luna", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-6-sol", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-6.1-sol", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-oss-20b-1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-oss-120b-1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-oss-safeguard-20b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("openai.gpt-oss-safeguard-120b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Qwen.
    ("qwen.qwen3-32b-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("qwen.qwen3-235b-a22b-2507-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("qwen.qwen3-coder-30b-a3b-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("qwen.qwen3-coder-480b-a35b-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("qwen.qwen3-next-80b-a3b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("qwen.qwen3-vl-235b-a22b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // TwelveLabs reads a clip and answers about it in text.
    ("twelvelabs.pegasus-1-2-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Writer.
    ("writer.palmyra-vision-7b", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("writer.palmyra-x4-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("writer.palmyra-x5-v1:0", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // xAI.
    ("xai.grok-4.6", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("xai.grok-4.7", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Z AI.
    ("zai.glm-4.7", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("zai.glm-4.7-flash", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("zai.glm-5", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    ("zai.glm-5.3", text(OpenAiChat, OPENAI_CHAT_MAX_TOKENS)),
    // Recognised ids whose modality this emulator serves no inference for: the embedding
    // and reranking models, which answer a vector or a score rather than a turn, and the
    // image generation and image editing models.
    ("amazon.rerank-v1:0", Served::Unserved(Embedding)),
    ("amazon.titan-embed-g1-text-02", Served::Unserved(Embedding)),
    ("amazon.titan-embed-text-v1", Served::Unserved(Embedding)),
    ("amazon.titan-embed-text-v1:2:8k", Served::Unserved(Embedding)),
    ("amazon.titan-embed-text-v2:0", Served::Unserved(Embedding)),
    ("amazon.titan-embed-image-v1", Served::Unserved(Embedding)),
    ("amazon.titan-embed-image-v1:0", Served::Unserved(Embedding)),
    ("cohere.embed-english-v3", Served::Unserved(Embedding)),
    ("cohere.embed-english-v3:0", Served::Unserved(Embedding)),
    ("cohere.embed-multilingual-v3", Served::Unserved(Embedding)),
    ("cohere.embed-multilingual-v3:0", Served::Unserved(Embedding)),
    ("cohere.embed-v4:0", Served::Unserved(Embedding)),
    ("cohere.rerank-v3-5:0", Served::Unserved(Embedding)),
    ("amazon.nova-canvas-v1:0", Served::Unserved(Image)),
    ("amazon.titan-image-generator-v1", Served::Unserved(Image)),
    ("amazon.titan-image-generator-v1:0", Served::Unserved(Image)),
    ("amazon.titan-image-generator-v2:0", Served::Unserved(Image)),
    ("stability.sd3-5-large-v1:0", Served::Unserved(Image)),
    ("stability.stable-diffusion-xl-v1", Served::Unserved(Image)),
    ("stability.stable-conservative-upscale-v1:0", Served::Unserved(Image)),
    ("stability.stable-creative-upscale-v1:0", Served::Unserved(Image)),
    ("stability.stable-fast-upscale-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-control-sketch-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-control-structure-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-core-v1:1", Served::Unserved(Image)),
    ("stability.stable-image-erase-object-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-inpaint-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-remove-background-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-search-recolor-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-search-replace-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-style-guide-v1:0", Served::Unserved(Image)),
    ("stability.stable-image-ultra-v1:1", Served::Unserved(Image)),
    ("stability.stable-outpaint-v1:0", Served::Unserved(Image)),
    ("stability.stable-style-transfer-v1:0", Served::Unserved(Image)),
    // The video generation models, which are the models `StartAsyncInvoke` serves.
    ("amazon.nova-reel-v1:0", Served::AsyncVideo),
    ("amazon.nova-reel-v1:1", Served::AsyncVideo),
    ("luma.ray-v2:0", Served::AsyncVideo),
    // Past end of life ("Model lifecycle"): the identifier resolves, the model is gone, so
    // an invocation is answered 404 with the end-of-life reason.
    ("amazon.titan-text-express-v1", Served::Retired(Text)),
    ("anthropic.claude-v1", Served::Retired(Text)),
    ("anthropic.claude-v2", Served::Retired(Text)),
    ("anthropic.claude-v2:1", Served::Retired(Text)),
    ("anthropic.claude-instant-v1", Served::Retired(Text)),
    ("ai21.j2-mid-v1", Served::Retired(Text)),
    ("ai21.j2-ultra-v1", Served::Retired(Text)),
    ("ai21.j2-grande-instruct", Served::Retired(Text)),
    ("ai21.j2-jumbo-instruct", Served::Retired(Text)),
    ("meta.llama2-13b-chat-v1", Served::Retired(Text)),
    ("meta.llama2-70b-chat-v1", Served::Retired(Text)),
    ("stability.stable-diffusion-xl-v0", Served::Retired(Image)),
];

/// The inventory [`super::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState`]
/// seeds itself with: one [`ModelEntry`] per row of [`CATALOGUE`].
pub(super) fn seed_models() -> Vec<ModelEntry> {
    CATALOGUE
        .iter()
        .map(|(model_id, served)| entry(model_id, *served))
        .collect()
}

/// One inventory entry, as its catalogue row says the model is served.
fn entry(model_id: &str, served: Served) -> ModelEntry {
    let model_id = model_id.to_string();
    match served {
        Served::Text {
            family,
            max_output_tokens,
            converse,
        } => ModelEntry {
            context_window: context_window(&model_id, family),
            model_id,
            family: Some(family),
            modality: Text,
            lifecycle: ModelLifecycle::Active,
            supports_converse: converse,
            // `CountTokens` counts with the MODEL's own tokenizer, and AWS publishes it
            // for the Anthropic Claude family (`CountTokens` "Supported models"); the
            // recorded corpus shows the service refusing the count for a model outside
            // that set rather than answering one.
            supports_count_tokens: matches!(family, AnthropicMessages),
            // No text model is invocable asynchronously: `StartAsyncInvoke`'s result is an
            // artifact written to Amazon S3, which is what the video models produce.
            supports_async_invoke: false,
            max_output_tokens,
        },
        Served::Unserved(modality) => ModelEntry {
            model_id,
            family: None,
            modality,
            lifecycle: ModelLifecycle::Active,
            supports_converse: false,
            supports_count_tokens: false,
            supports_async_invoke: false,
            max_output_tokens: 0,
            context_window: 0,
        },
        Served::AsyncVideo => ModelEntry {
            model_id,
            family: None,
            modality: ModelModality::Video,
            lifecycle: ModelLifecycle::Active,
            supports_converse: false,
            supports_count_tokens: false,
            supports_async_invoke: true,
            max_output_tokens: 0,
            context_window: 0,
        },
        Served::Retired(modality) => ModelEntry {
            model_id,
            family: None,
            modality,
            lifecycle: ModelLifecycle::EndOfLife,
            supports_converse: false,
            supports_count_tokens: false,
            supports_async_invoke: false,
            max_output_tokens: 0,
            context_window: 0,
        },
    }
}
