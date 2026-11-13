// SPDX-License-Identifier: CC0-1.0

//! The foundation-model CATALOGUE of [`super`]: the models this emulator publishes, what
//! each supports, the access state of each and the renderings the model and inference
//! profile surfaces report them through.

use super::*;
mod published_routing;
pub use published_routing::*;

/// One foundation model Amazon Bedrock offers, as this control plane reports it.
///
/// Every field is `&'static` because the catalog is PUBLISHED data rather than anything a
/// request brings into being: this service has no operation that creates a foundation
/// model, so these rows are the same for every account and no handler writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundationModel {
    /// `BedrockModelId` — the id every operation that names a base model names it by.
    pub model_id: &'static str,
    /// `BrandedName` (len 1..20) — the model's own name.
    pub model_name: &'static str,
    /// `BrandedName` (len 1..20) — the provider that publishes the model.
    pub provider_name: &'static str,
    /// The model LINE this model belongs to: the branded family name its provider publishes
    /// the model under ("Amazon Bedrock foundation models" groups each provider's models by
    /// family, and the model catalogue reports that family beside the model), with the
    /// per-model variant left off — the family of a model is what it shares with its
    /// siblings, so two sizes or two dated revisions of one line carry the same family.
    ///
    /// Stored rather than derived: a family is branded data, so it is published for the
    /// model rather than computable from its id.
    pub model_family: &'static str,
    /// `ModelModalityList` — what the model takes, each value out of `ModelModality`
    /// (`crate::ops::types::MODEL_MODALITY_VALUES`).
    pub input_modalities: &'static [&'static str],
    /// `ModelModalityList` — what the model produces, from the same value set.
    pub output_modalities: &'static [&'static str],
    /// Whether the model streams its response.
    pub response_streaming_supported: bool,
    /// `ModelCustomizationList` — the customizations the model supports, each value out of
    /// `ModelCustomization` (`crate::ops::types::MODEL_CUSTOMIZATION_VALUES`); empty for a
    /// model that supports none.
    pub customizations_supported: &'static [&'static str],
    /// `InferenceTypeList` — the inference types the model is offered under, each value out
    /// of `InferenceType` (`crate::ops::types::INFERENCE_TYPE_VALUES`).
    pub inference_types_supported: &'static [&'static str],
    /// The Amazon Bedrock FEATURES published for this model, each out of
    /// [`PUBLISHED_FEATURE_VALUES`].
    ///
    /// Every one of them is a feature Amazon Bedrock layers ON a model — caching a prompt
    /// prefix, routing a prompt between models, optimizing inference latency, running the
    /// model over a dataset — and each publishes the models it supports in its own
    /// "Supported models" table in the Amazon Bedrock User Guide. So it is a published
    /// per-model fact and NOT derivable from what the model does: two models with the same
    /// modalities, streaming and inference types differ here when the feature's table lists
    /// one and not the other, and a model no table lists supports none of them — a freshly
    /// offered model is exactly that case.
    pub features_supported: &'static [&'static str],
    /// `FoundationModelLifecycleStatus` — `ACTIVE` for a model that is available, `LEGACY`
    /// for one that is deprecated
    /// (`crate::ops::types::FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES`).
    pub lifecycle_status: &'static str,
    /// `FoundationModelLifecycle$startOfLifeTime` — "Launch time when the model first
    /// becomes available" (`smithy_query(query="shape",shape="FoundationModelLifecycle")`),
    /// as an RFC 3339 instant: the day AWS made the model available on Amazon Bedrock, taken
    /// from that model's launch, at midnight UTC because what is published for it is a day
    /// rather than an instant. Every offered model has one — a model that is offered has
    /// launched — so this is not an `Option`.
    pub start_of_life_time: &'static str,
    /// `FoundationModelLifecycle$legacyTime` and `$endOfLifeTime` — the legacy window AWS
    /// publishes for a deprecated model ([`LegacyWindow`]), carried by a row whose
    /// [`FoundationModel::lifecycle_status`] is [`LIFECYCLE_LEGACY`] and by no other: a
    /// model that is still available has entered no legacy state and is scheduled for no
    /// end of life, and an instant for one is not something to invent.
    pub legacy_window: Option<LegacyWindow>,
}

/// The legacy window AWS publishes for a deprecated base model ("Model lifecycle" in the
/// Amazon Bedrock User Guide): a model enters legacy state on one date and stops being
/// available on another, and a caller is expected to move off it in between.
///
/// The two instants travel together because they are two ends of ONE window: a model that
/// has entered legacy state has an end of life, and one that has not has neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyWindow {
    /// `FoundationModelLifecycle$legacyTime` — "Time when the model enters legacy state"
    /// (`smithy_query(query="shape",shape="FoundationModelLifecycle")`), as RFC 3339.
    pub entered_at: &'static str,
    /// `FoundationModelLifecycle$endOfLifeTime` — "Time when the model is no longer
    /// available for use", the far end of the window, as RFC 3339.
    pub end_of_life_at: &'static str,
}

/// `ModelModality` — a model that takes text only.
pub(super) const IN_TEXT: &[&str] = &["TEXT"];
/// `ModelModality` — a model that takes text or an image.
pub(super) const IN_TEXT_AND_IMAGE: &[&str] = &["TEXT", "IMAGE"];
/// `ModelModality` — a model that generates text.
pub(super) const OUT_TEXT: &[&str] = &["TEXT"];
/// `ModelModality` — a model that generates an image.
pub(super) const OUT_IMAGE: &[&str] = &["IMAGE"];
/// `ModelModality` — a model that produces an embedding vector.
pub(super) const OUT_EMBEDDING: &[&str] = &["EMBEDDING"];
/// `InferenceType` — a model offered on demand only.
pub(super) const ON_DEMAND: &[&str] = &["ON_DEMAND"];
/// `InferenceType` — a model offered both on demand and with Provisioned Throughput.
pub(super) const ON_DEMAND_AND_PROVISIONED: &[&str] = &["ON_DEMAND", "PROVISIONED"];
/// `ModelCustomizationList` — a model no customization job accepts.
pub(super) const NO_CUSTOMIZATION: &[&str] = &[];
/// `ModelCustomization` — a model that can be fine-tuned on labelled data.
pub(super) const FINE_TUNING: &[&str] = &["FINE_TUNING"];
/// `ModelCustomization` — a model that accepts both fine-tuning and continued
/// pre-training.
pub(super) const FINE_TUNING_AND_PRE_TRAINING: &[&str] = &["FINE_TUNING", "CONTINUED_PRE_TRAINING"];
/// `ModelCustomization` — a model that accepts both fine-tuning and distillation.
pub(super) const FINE_TUNING_AND_DISTILLATION: &[&str] = &["FINE_TUNING", "DISTILLATION"];

/// Every Amazon Bedrock feature a catalog row can publish support for
/// ([`FoundationModel::features_supported`]), in the order a summary reports them.
///
/// Each one has its own "Supported models" table in the Amazon Bedrock User Guide, which is
/// what makes support for it a published per-model fact; the order here is that of the
/// feature names, so two rows list the features they share in the same order.
pub const PUBLISHED_FEATURE_VALUES: &[&str] = &[
    FEATURE_BATCH_INFERENCE,
    FEATURE_INTELLIGENT_PROMPT_ROUTING,
    FEATURE_LATENCY_OPTIMIZATION,
    FEATURE_PROMPT_CACHING,
];

/// `featuresSupported` — no feature's table lists the model, which is where a freshly
/// offered model starts.
pub(super) const NO_FEATURES: &[&str] = &[];
/// `featuresSupported` — a model batch inference runs and no other feature lists.
pub(super) const BATCH_ONLY: &[&str] = &[FEATURE_BATCH_INFERENCE];
/// `featuresSupported` — a model batch inference runs and a prompt router routes to.
pub(super) const BATCH_AND_ROUTING: &[&str] = &[FEATURE_BATCH_INFERENCE, FEATURE_INTELLIGENT_PROMPT_ROUTING];
/// `featuresSupported` — a model batch inference runs and a prompt prefix may be cached for.
pub(super) const BATCH_AND_CACHING: &[&str] = &[FEATURE_BATCH_INFERENCE, FEATURE_PROMPT_CACHING];
/// `featuresSupported` — a model only prompt caching's table lists.
pub(super) const CACHING_ONLY: &[&str] = &[FEATURE_PROMPT_CACHING];
/// `featuresSupported` — a model every one of these four features lists.
pub(super) const EVERY_PUBLISHED_FEATURE: &[&str] = PUBLISHED_FEATURE_VALUES;
/// `FoundationModelLifecycleStatus` — the model is available.
pub(super) const LIFECYCLE_ACTIVE: &str = "ACTIVE";
/// `FoundationModelLifecycleStatus` — the model is deprecated: it is still offered and
/// still invocable, and callers are expected to move off it before its end of life. The
/// two values this enum carries are the two states an OFFERED model can be in; a model
/// past its end of life is no longer offered, so it is absent from this catalog rather
/// than carried in it under a status (see [`FOUNDATION_MODELS`]).
pub(super) const LIFECYCLE_LEGACY: &str = "LEGACY";

/// A catalog row for a model that GENERATES TEXT: it answers with a turn of a conversation
/// and streams that answer, it is available, no customization job accepts it, and it is
/// offered on demand — which is what the catalog publishes for most of the models a client
/// converses with. A row AWS also sells reserved capacity for is wrapped in
/// [`also_provisioned`].
pub(super) const fn text_model(
    model_id: &'static str,
    model_name: &'static str,
    provider_name: &'static str,
    model_family: &'static str,
    input_modalities: &'static [&'static str],
    features_supported: &'static [&'static str],
    start_of_life_time: &'static str,
) -> FoundationModel {
    FoundationModel {
        model_id,
        model_name,
        provider_name,
        model_family,
        input_modalities,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time,
        legacy_window: None,
    }
}

/// A catalog row for a model that produces an EMBEDDING vector: a vector is no turn of a
/// conversation, so nothing it answers is streamed.
pub(super) const fn embedding_model(
    model_id: &'static str,
    model_name: &'static str,
    provider_name: &'static str,
    model_family: &'static str,
    input_modalities: &'static [&'static str],
    features_supported: &'static [&'static str],
    start_of_life_time: &'static str,
) -> FoundationModel {
    FoundationModel {
        model_id,
        model_name,
        provider_name,
        model_family,
        input_modalities,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time,
        legacy_window: None,
    }
}

/// A catalog row for a model that produces an IMAGE: an image is answered in one piece, so
/// nothing it answers is streamed.
pub(super) const fn image_model(
    model_id: &'static str,
    model_name: &'static str,
    provider_name: &'static str,
    model_family: &'static str,
    input_modalities: &'static [&'static str],
    features_supported: &'static [&'static str],
    start_of_life_time: &'static str,
) -> FoundationModel {
    FoundationModel {
        model_id,
        model_name,
        provider_name,
        model_family,
        input_modalities,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time,
        legacy_window: None,
    }
}

/// The same row, offered WITH PROVISIONED THROUGHPUT beside on demand: a model AWS also
/// sells reserved capacity for ("Supported Regions and models for Provisioned Throughput"),
/// which is what `byInferenceType=PROVISIONED` narrows a listing to.
pub(super) const fn also_provisioned(model: FoundationModel) -> FoundationModel {
    FoundationModel {
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        ..model
    }
}

/// The same row, publishing the CUSTOMIZATIONS the model's training variant accepts.
///
/// Amazon Bedrock publishes a customizable model under its TRAINING-QUALIFIED id — the
/// model id with its version and context-length qualifiers — and it is that id a
/// customization job names, so the customizations belong to the variant's row and the
/// on-demand row publishes none. One published model, two ids a client may address it by,
/// each publishing what it is offered for.
pub(super) const fn customizable(
    model: FoundationModel,
    customizations_supported: &'static [&'static str],
) -> FoundationModel {
    FoundationModel {
        customizations_supported,
        ..model
    }
}

/// Every foundation model this emulator's Amazon Bedrock offers, in `modelId` order.
///
/// Amazon Bedrock publishes its base models rather than letting a caller create one —
/// there is no `CreateFoundationModel` — so this is the published catalog itself: the set
/// `ListFoundationModels` reports and `GetFoundationModel` resolves one row of, not
/// anything a request can add to or remove from.
///
/// It is the set of models the service currently OFFERS, which is what makes it the set
/// both of those operations answer out of: a model is here while it is available
/// ([`LIFECYCLE_ACTIVE`]) and while it is deprecated ([`LIFECYCLE_LEGACY`]), and a model
/// past its end of life is not offered at all, so it is in no row — it is listed by
/// nothing and resolves for nobody. The inference plane keeps such an id in its own
/// inventory to refuse it, which is where the end-of-life refusal belongs; the control
/// plane simply no longer carries the model.
///
/// The ids are the base model ids AWS publishes in "Amazon Bedrock base model IDs
/// (on-demand throughput)" and "Supported foundation models in Amazon Bedrock", and the
/// rows AWS marks deprecated carry the status "Model lifecycle" lists them under; this
/// emulator's bedrock-runtime plane serves the same ids, so the two planes agree about
/// which models exist. Each row's provider, family, modalities, streaming support,
/// customizations, inference types and published features are as those pages publish them
/// for that model — the features from each feature's own "Supported models" table, so a
/// model a table does not list does not support that feature — and a per-model attribute no
/// page publishes is left out of the row rather than invented (`publicExtendedAccessTime`
/// is such an instant, so no row carries one).
///
/// Every value of each member a caller can filter on is carried by a row, so a filter on
/// any of them narrows the catalog rather than emptying it.
///
/// A row whose published attributes differ from the three kinds below — a model a
/// customization job accepts, or a deprecated one — is written out member by member; every
/// other row is built by [`text_model`], [`embedding_model`] or [`image_model`], which are
/// those three kinds, wrapped in [`also_provisioned`] where AWS sells reserved capacity for
/// the model too.
pub const FOUNDATION_MODELS: &[FoundationModel] = &[
    FoundationModel {
        model_id: "ai21.jamba-1-5-mini-v1:0",
        model_name: "Jamba 1.5 Mini",
        provider_name: "AI21 Labs",
        model_family: "Jamba 1.5",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-08-22T00:00:00Z",
        legacy_window: None,
    },
    // Amazon Nova — the Nova 2 generation, offered on demand.
    text_model(
        "amazon.nova-2-5-sonic",
        "Nova 2.5 Sonic",
        "Amazon",
        "Nova 2",
        IN_TEXT,
        CACHING_ONLY,
        "2026-03-10T00:00:00Z",
    ),
    text_model(
        "amazon.nova-2-lite-v1:0",
        "Nova 2 Lite",
        "Amazon",
        "Nova 2",
        IN_TEXT_AND_IMAGE,
        BATCH_AND_CACHING,
        "2025-12-03T00:00:00Z",
    ),
    text_model(
        "amazon.nova-2-sonic-v1:0",
        "Nova 2 Sonic",
        "Amazon",
        "Nova 2",
        IN_TEXT,
        CACHING_ONLY,
        "2025-12-03T00:00:00Z",
    ),
    FoundationModel {
        model_id: "amazon.nova-canvas-v1:0",
        model_name: "Nova Canvas",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-lite-v1:0",
        model_name: "Nova Lite",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: EVERY_PUBLISHED_FEATURE,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-micro-v1:0",
        model_name: "Nova Micro",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: EVERY_PUBLISHED_FEATURE,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.nova-pro-v1:0",
        model_name: "Nova Pro",
        provider_name: "Amazon",
        model_family: "Nova",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_DISTILLATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: EVERY_PUBLISHED_FEATURE,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-12-03T00:00:00Z",
        legacy_window: None,
    },
    // A reranking model scores passages against a query, so what it answers is no turn of a
    // conversation either.
    embedding_model(
        "amazon.rerank-v1:0",
        "Rerank",
        "Amazon",
        "Rerank",
        IN_TEXT,
        NO_FEATURES,
        "2024-12-01T00:00:00Z",
    ),
    also_provisioned(embedding_model(
        "amazon.titan-embed-g1-text-02",
        "Titan Embed G1 Text",
        "Amazon",
        "Titan",
        IN_TEXT,
        BATCH_ONLY,
        "2023-09-28T00:00:00Z",
    )),
    FoundationModel {
        model_id: "amazon.titan-embed-image-v1",
        model_name: "Titan Embed Image",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    // The same two Titan embedding models under the VERSION-QUALIFIED ids the catalog also
    // publishes them under: one published model, two ids a client may address it by. The
    // qualified id is the one a customization names, so it is the row that publishes the
    // customizations the model accepts ([`customizable`]).
    customizable(
        also_provisioned(embedding_model(
            "amazon.titan-embed-image-v1:0",
            "Titan Embed Image",
            "Amazon",
            "Titan",
            IN_TEXT_AND_IMAGE,
            BATCH_ONLY,
            "2023-11-29T00:00:00Z",
        )),
        FINE_TUNING,
    ),
    also_provisioned(embedding_model(
        "amazon.titan-embed-text-v1",
        "Titan Embed Text",
        "Amazon",
        "Titan",
        IN_TEXT,
        BATCH_ONLY,
        "2023-09-28T00:00:00Z",
    )),
    also_provisioned(embedding_model(
        "amazon.titan-embed-text-v1:2:8k",
        "Titan Embed Text",
        "Amazon",
        "Titan",
        IN_TEXT,
        BATCH_ONLY,
        "2023-09-28T00:00:00Z",
    )),
    FoundationModel {
        model_id: "amazon.titan-embed-text-v2:0",
        model_name: "Titan Embed Text V2",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-30T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "amazon.titan-image-generator-v1",
        model_name: "Titan Image Gen G1",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    // The same model under the version-qualified id a customization names.
    customizable(
        also_provisioned(image_model(
            "amazon.titan-image-generator-v1:0",
            "Titan Image Gen G1",
            "Amazon",
            "Titan",
            IN_TEXT_AND_IMAGE,
            NO_FEATURES,
            "2023-11-29T00:00:00Z",
        )),
        FINE_TUNING,
    ),
    FoundationModel {
        model_id: "amazon.titan-text-lite-v1",
        model_name: "Titan Text Lite",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-04-30T00:00:00Z",
        }),
    },
    // The same model under the training-qualified id a customization names: the version and
    // the context length, which is how Amazon Bedrock publishes a customizable Titan text
    // variant.
    FoundationModel {
        model_id: "amazon.titan-text-lite-v1:0:4k",
        model_name: "Titan Text Lite",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING_AND_PRE_TRAINING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-04-30T00:00:00Z",
        }),
    },
    FoundationModel {
        model_id: "amazon.titan-text-premier-v1:0",
        model_name: "Titan Text Premier",
        provider_name: "Amazon",
        model_family: "Titan",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-30T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-5-sonnet-20240620-v1:0",
        model_name: "Claude 3.5 Sonnet",
        provider_name: "Anthropic",
        model_family: "Claude 3.5",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_AND_ROUTING,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-06-20T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        model_name: "Claude 3.5 Sonnet",
        provider_name: "Anthropic",
        model_family: "Claude 3.5",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_AND_ROUTING,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-10-22T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        model_name: "Claude 3 Haiku",
        provider_name: "Anthropic",
        model_family: "Claude 3",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_AND_ROUTING,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-03-13T00:00:00Z",
        legacy_window: None,
    },
    FoundationModel {
        model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
        model_name: "Claude 3 Sonnet",
        provider_name: "Anthropic",
        model_family: "Claude 3",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-03-04T00:00:00Z",
        legacy_window: None,
    },
    // Anthropic Claude — the Claude 4 and Claude 5 generations, each row under the line
    // ("family") its provider publishes it in.
    text_model(
        "anthropic.claude-fable-5",
        "Claude Fable 5",
        "Anthropic",
        "Claude Fable 5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-05-05T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-fable-5-1",
        "Claude Fable 5.1",
        "Anthropic",
        "Claude Fable 5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-08-11T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-haiku-4-5-20251001-v1:0",
        "Claude Haiku 4.5",
        "Anthropic",
        "Claude Haiku 4.5",
        IN_TEXT_AND_IMAGE,
        EVERY_PUBLISHED_FEATURE,
        "2025-10-01T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-4-1-20250805-v1:0",
        "Claude Opus 4.1",
        "Anthropic",
        "Claude Opus 4",
        IN_TEXT_AND_IMAGE,
        BATCH_AND_CACHING,
        "2025-08-05T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-4-5-20251101-v1:0",
        "Claude Opus 4.5",
        "Anthropic",
        "Claude Opus 4",
        IN_TEXT_AND_IMAGE,
        BATCH_AND_CACHING,
        "2025-11-01T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-4-6-v1",
        "Claude Opus 4.6",
        "Anthropic",
        "Claude Opus 4",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-02-05T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-4-7",
        "Claude Opus 4.7",
        "Anthropic",
        "Claude Opus 4",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-04-14T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-4-8",
        "Claude Opus 4.8",
        "Anthropic",
        "Claude Opus 4",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-06-16T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-5",
        "Claude Opus 5",
        "Anthropic",
        "Claude Opus 5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-08-25T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-opus-5-5",
        "Claude Opus 5.5",
        "Anthropic",
        "Claude Opus 5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-10-20T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-sonnet-4-20250514-v1:0",
        "Claude Sonnet 4",
        "Anthropic",
        "Claude Sonnet 4",
        IN_TEXT_AND_IMAGE,
        BATCH_AND_CACHING,
        "2025-05-14T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-sonnet-4-5-20250929-v1:0",
        "Claude Sonnet 4.5",
        "Anthropic",
        "Claude Sonnet 4",
        IN_TEXT_AND_IMAGE,
        BATCH_AND_CACHING,
        "2025-09-29T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        "Anthropic",
        "Claude Sonnet 4",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-02-24T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-sonnet-5",
        "Claude Sonnet 5",
        "Anthropic",
        "Claude Sonnet 5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-07-14T00:00:00Z",
    ),
    text_model(
        "anthropic.claude-sonnet-5-5",
        "Claude Sonnet 5.5",
        "Anthropic",
        "Claude Sonnet 5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-09-29T00:00:00Z",
    ),
    FoundationModel {
        model_id: "cohere.command-light-text-v14",
        model_name: "Command Light",
        provider_name: "Cohere",
        model_family: "Command",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-06-30T00:00:00Z",
        }),
    },
    // The same model under the training-qualified id a customization names.
    FoundationModel {
        model_id: "cohere.command-light-text-v14:7:4k",
        model_name: "Command Light",
        provider_name: "Cohere",
        model_family: "Command",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-06-30T00:00:00Z",
        }),
    },
    FoundationModel {
        model_id: "cohere.command-text-v14",
        model_name: "Command",
        provider_name: "Cohere",
        model_family: "Command",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-06-30T00:00:00Z",
        }),
    },
    // The same model under the training-qualified id a customization names.
    FoundationModel {
        model_id: "cohere.command-text-v14:7:4k",
        model_name: "Command",
        provider_name: "Cohere",
        model_family: "Command",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_LEGACY,
        start_of_life_time: "2023-09-28T00:00:00Z",
        legacy_window: Some(LegacyWindow {
            entered_at: "2025-04-30T00:00:00Z",
            end_of_life_at: "2027-06-30T00:00:00Z",
        }),
    },
    FoundationModel {
        model_id: "cohere.embed-english-v3",
        model_name: "Embed English",
        provider_name: "Cohere",
        model_family: "Embed",
        input_modalities: IN_TEXT,
        output_modalities: OUT_EMBEDDING,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    // Cohere — the Embed and Rerank lines, each embedding model also under the
    // version-qualified id the catalog publishes it under.
    also_provisioned(embedding_model(
        "cohere.embed-english-v3:0",
        "Embed English",
        "Cohere",
        "Embed",
        IN_TEXT,
        BATCH_ONLY,
        "2023-11-29T00:00:00Z",
    )),
    also_provisioned(embedding_model(
        "cohere.embed-multilingual-v3",
        "Embed Multilingual",
        "Cohere",
        "Embed",
        IN_TEXT,
        BATCH_ONLY,
        "2023-11-29T00:00:00Z",
    )),
    also_provisioned(embedding_model(
        "cohere.embed-multilingual-v3:0",
        "Embed Multilingual",
        "Cohere",
        "Embed",
        IN_TEXT,
        BATCH_ONLY,
        "2023-11-29T00:00:00Z",
    )),
    embedding_model(
        "cohere.embed-v4:0",
        "Embed 4",
        "Cohere",
        "Embed",
        IN_TEXT_AND_IMAGE,
        BATCH_ONLY,
        "2025-04-15T00:00:00Z",
    ),
    embedding_model(
        "cohere.rerank-v3-5:0",
        "Rerank 3.5",
        "Cohere",
        "Rerank",
        IN_TEXT,
        NO_FEATURES,
        "2024-12-02T00:00:00Z",
    ),
    // DeepSeek.
    text_model(
        "deepseek.r1-v1:0",
        "DeepSeek-R1",
        "DeepSeek",
        "DeepSeek R1",
        IN_TEXT,
        NO_FEATURES,
        "2025-03-10T00:00:00Z",
    ),
    text_model(
        "deepseek.v3-v1:0",
        "DeepSeek-V3",
        "DeepSeek",
        "DeepSeek V3",
        IN_TEXT,
        NO_FEATURES,
        "2025-09-30T00:00:00Z",
    ),
    text_model(
        "deepseek.v3.2",
        "DeepSeek-V3.2",
        "DeepSeek",
        "DeepSeek V3",
        IN_TEXT,
        NO_FEATURES,
        "2025-12-01T00:00:00Z",
    ),
    // Google Gemma.
    text_model(
        "google.gemma-3-12b-it",
        "Gemma 3 12B",
        "Google",
        "Gemma 3",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-01-20T00:00:00Z",
    ),
    text_model(
        "google.gemma-3-27b-it",
        "Gemma 3 27B",
        "Google",
        "Gemma 3",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-01-20T00:00:00Z",
    ),
    text_model(
        "google.gemma-3-4b-it",
        "Gemma 3 4B",
        "Google",
        "Gemma 3",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-01-20T00:00:00Z",
    ),
    // Luma AI generates a clip rather than a conversation, so what it answers is an IMAGE
    // sequence and nothing it answers is streamed.
    image_model(
        "luma.ray-v2:0",
        "Ray 2",
        "Luma AI",
        "Ray",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-02-20T00:00:00Z",
    ),
    // Meta Llama.
    also_provisioned(text_model(
        "meta.llama3-1-70b-instruct-v1:0",
        "Llama 3.1 70B",
        "Meta",
        "Llama 3.1",
        IN_TEXT,
        BATCH_ONLY,
        "2024-07-23T00:00:00Z",
    )),
    FoundationModel {
        model_id: "meta.llama3-1-8b-instruct-v1:0",
        model_name: "Llama 3.1 8B",
        provider_name: "Meta",
        model_family: "Llama 3.1",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: FINE_TUNING,
        inference_types_supported: ON_DEMAND,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-07-23T00:00:00Z",
        legacy_window: None,
    },
    text_model(
        "meta.llama3-3-70b-instruct-v1:0",
        "Llama 3.3 70B",
        "Meta",
        "Llama 3.3",
        IN_TEXT,
        BATCH_ONLY,
        "2024-12-06T00:00:00Z",
    ),
    FoundationModel {
        model_id: "meta.llama3-70b-instruct-v1:0",
        model_name: "Llama 3 70B Instruct",
        provider_name: "Meta",
        model_family: "Llama 3",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-18T00:00:00Z",
        legacy_window: None,
    },
    also_provisioned(text_model(
        "meta.llama3-8b-instruct-v1:0",
        "Llama 3 8B Instruct",
        "Meta",
        "Llama 3",
        IN_TEXT,
        BATCH_ONLY,
        "2024-04-18T00:00:00Z",
    )),
    text_model(
        "meta.llama4-maverick-17b-instruct-v1:0",
        "Llama 4 Maverick",
        "Meta",
        "Llama 4",
        IN_TEXT_AND_IMAGE,
        BATCH_ONLY,
        "2025-04-29T00:00:00Z",
    ),
    text_model(
        "meta.llama4-scout-17b-instruct-v1:0",
        "Llama 4 Scout",
        "Meta",
        "Llama 4",
        IN_TEXT_AND_IMAGE,
        BATCH_ONLY,
        "2025-04-29T00:00:00Z",
    ),
    // MiniMax.
    text_model(
        "minimax.minimax-m2",
        "MiniMax M2",
        "MiniMax",
        "MiniMax M2",
        IN_TEXT,
        NO_FEATURES,
        "2025-11-11T00:00:00Z",
    ),
    text_model(
        "minimax.minimax-m2.1",
        "MiniMax M2.1",
        "MiniMax",
        "MiniMax M2",
        IN_TEXT,
        NO_FEATURES,
        "2026-01-13T00:00:00Z",
    ),
    text_model(
        "minimax.minimax-m2.5",
        "MiniMax M2.5",
        "MiniMax",
        "MiniMax M2",
        IN_TEXT,
        NO_FEATURES,
        "2026-04-07T00:00:00Z",
    ),
    // Mistral AI.
    text_model(
        "mistral.devstral-2-123b",
        "Devstral 2 123B",
        "Mistral AI",
        "Devstral 2",
        IN_TEXT,
        NO_FEATURES,
        "2026-01-27T00:00:00Z",
    ),
    text_model(
        "mistral.magistral-small-2509",
        "Magistral Small",
        "Mistral AI",
        "Magistral",
        IN_TEXT,
        NO_FEATURES,
        "2025-09-18T00:00:00Z",
    ),
    text_model(
        "mistral.ministral-3-14b-instruct",
        "Ministral 3 14B",
        "Mistral AI",
        "Ministral 3",
        IN_TEXT,
        NO_FEATURES,
        "2025-10-16T00:00:00Z",
    ),
    text_model(
        "mistral.ministral-3-3b-instruct",
        "Ministral 3 3B",
        "Mistral AI",
        "Ministral 3",
        IN_TEXT,
        NO_FEATURES,
        "2025-10-16T00:00:00Z",
    ),
    text_model(
        "mistral.ministral-3-8b-instruct",
        "Ministral 3 8B",
        "Mistral AI",
        "Ministral 3",
        IN_TEXT,
        NO_FEATURES,
        "2025-10-16T00:00:00Z",
    ),
    also_provisioned(text_model(
        "mistral.mistral-7b-instruct-v0:2",
        "Mistral 7B Instruct",
        "Mistral AI",
        "Mistral",
        IN_TEXT,
        BATCH_ONLY,
        "2023-11-29T00:00:00Z",
    )),
    FoundationModel {
        model_id: "mistral.mistral-large-2402-v1:0",
        model_name: "Mistral Large",
        provider_name: "Mistral AI",
        model_family: "Mistral Large",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-04-02T00:00:00Z",
        legacy_window: None,
    },
    text_model(
        "mistral.mistral-large-2407-v1:0",
        "Mistral Large 2",
        "Mistral AI",
        "Mistral Large",
        IN_TEXT,
        BATCH_ONLY,
        "2024-07-24T00:00:00Z",
    ),
    text_model(
        "mistral.mistral-large-3-675b-instruct",
        "Mistral Large 3",
        "Mistral AI",
        "Mistral Large",
        IN_TEXT,
        NO_FEATURES,
        "2025-12-02T00:00:00Z",
    ),
    FoundationModel {
        model_id: "mistral.mixtral-8x7b-instruct-v0:1",
        model_name: "Mixtral 8x7B",
        provider_name: "Mistral AI",
        model_family: "Mixtral",
        input_modalities: IN_TEXT,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: BATCH_ONLY,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2024-03-01T00:00:00Z",
        legacy_window: None,
    },
    text_model(
        "mistral.pixtral-large-2502-v1:0",
        "Pixtral Large",
        "Mistral AI",
        "Pixtral",
        IN_TEXT_AND_IMAGE,
        BATCH_ONLY,
        "2025-02-24T00:00:00Z",
    ),
    text_model(
        "mistral.voxtral-mini-3b-2507",
        "Voxtral Mini 3B",
        "Mistral AI",
        "Voxtral",
        IN_TEXT,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    text_model(
        "mistral.voxtral-small-24b-2507",
        "Voxtral Small 24B",
        "Mistral AI",
        "Voxtral",
        IN_TEXT,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    // Moonshot AI.
    text_model(
        "moonshot.kimi-k2-thinking",
        "Kimi K2 Thinking",
        "Moonshot AI",
        "Kimi K2",
        IN_TEXT,
        NO_FEATURES,
        "2025-11-06T00:00:00Z",
    ),
    FoundationModel {
        model_id: "moonshotai.kimi-k2.5",
        model_name: "Kimi K2.5",
        provider_name: "Moonshot AI",
        model_family: "Kimi K2.5",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_TEXT,
        response_streaming_supported: true,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2025-12-09T17:00:00Z",
        legacy_window: None,
    },
    text_model(
        "moonshotai.kimi-k3",
        "Kimi K3",
        "Moonshot AI",
        "Kimi K3",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-04-21T00:00:00Z",
    ),
    // NVIDIA Nemotron.
    text_model(
        "nvidia.nemotron-nano-12b-v2",
        "Nemotron Nano 12B",
        "NVIDIA",
        "Nemotron Nano",
        IN_TEXT,
        NO_FEATURES,
        "2025-09-15T00:00:00Z",
    ),
    text_model(
        "nvidia.nemotron-nano-3-30b",
        "Nemotron Nano 3 30B",
        "NVIDIA",
        "Nemotron Nano 3",
        IN_TEXT,
        NO_FEATURES,
        "2026-02-09T00:00:00Z",
    ),
    text_model(
        "nvidia.nemotron-nano-9b-v2",
        "Nemotron Nano 9B",
        "NVIDIA",
        "Nemotron Nano",
        IN_TEXT,
        NO_FEATURES,
        "2025-08-18T00:00:00Z",
    ),
    text_model(
        "nvidia.nemotron-super-3-120b",
        "Nemotron Super 3",
        "NVIDIA",
        "Nemotron Super 3",
        IN_TEXT,
        NO_FEATURES,
        "2026-03-16T00:00:00Z",
    ),
    // OpenAI — the models whose own OpenAI-compatible endpoints Amazon Bedrock serves
    // beside its own invocation APIs.
    text_model(
        "openai.gpt-5.4",
        "GPT-5.4",
        "OpenAI",
        "GPT-5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-01-15T00:00:00Z",
    ),
    text_model(
        "openai.gpt-5.5",
        "GPT-5.5",
        "OpenAI",
        "GPT-5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-03-03T00:00:00Z",
    ),
    text_model(
        "openai.gpt-5.6-luna",
        "GPT-5.6 Luna",
        "OpenAI",
        "GPT-5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-05-19T00:00:00Z",
    ),
    text_model(
        "openai.gpt-5.6-sol",
        "GPT-5.6 Sol",
        "OpenAI",
        "GPT-5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-05-19T00:00:00Z",
    ),
    text_model(
        "openai.gpt-5.6-terra",
        "GPT-5.6 Terra",
        "OpenAI",
        "GPT-5",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-05-19T00:00:00Z",
    ),
    text_model(
        "openai.gpt-6-astra",
        "GPT-6 Astra",
        "OpenAI",
        "GPT-6",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-09-08T17:00:00Z",
    ),
    text_model(
        "openai.gpt-6-luna",
        "GPT-6 Luna",
        "OpenAI",
        "GPT-6",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-09-08T17:00:00Z",
    ),
    text_model(
        "openai.gpt-6-sol",
        "GPT-6 Sol",
        "OpenAI",
        "GPT-6",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-09-08T17:00:00Z",
    ),
    text_model(
        "openai.gpt-6.1-sol",
        "GPT-6.1 Sol",
        "OpenAI",
        "GPT-6",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-11-10T00:00:00Z",
    ),
    text_model(
        "openai.gpt-oss-120b-1:0",
        "GPT-OSS 120B",
        "OpenAI",
        "GPT-OSS",
        IN_TEXT,
        NO_FEATURES,
        "2025-08-05T00:00:00Z",
    ),
    text_model(
        "openai.gpt-oss-20b-1:0",
        "GPT-OSS 20B",
        "OpenAI",
        "GPT-OSS",
        IN_TEXT,
        NO_FEATURES,
        "2025-08-05T00:00:00Z",
    ),
    text_model(
        "openai.gpt-oss-safeguard-120b",
        "Safeguard 120B",
        "OpenAI",
        "GPT-OSS",
        IN_TEXT,
        NO_FEATURES,
        "2025-10-29T00:00:00Z",
    ),
    text_model(
        "openai.gpt-oss-safeguard-20b",
        "Safeguard 20B",
        "OpenAI",
        "GPT-OSS",
        IN_TEXT,
        NO_FEATURES,
        "2025-10-29T00:00:00Z",
    ),
    // Qwen.
    text_model(
        "qwen.qwen3-235b-a22b-2507-v1:0",
        "Qwen3 235B A22B",
        "Qwen",
        "Qwen3",
        IN_TEXT,
        NO_FEATURES,
        "2025-07-29T00:00:00Z",
    ),
    text_model(
        "qwen.qwen3-32b-v1:0",
        "Qwen3 32B",
        "Qwen",
        "Qwen3",
        IN_TEXT,
        NO_FEATURES,
        "2025-07-29T00:00:00Z",
    ),
    text_model(
        "qwen.qwen3-coder-30b-a3b-v1:0",
        "Qwen3 Coder 30B",
        "Qwen",
        "Qwen3 Coder",
        IN_TEXT,
        NO_FEATURES,
        "2025-07-29T00:00:00Z",
    ),
    text_model(
        "qwen.qwen3-coder-480b-a35b-v1:0",
        "Qwen3 Coder 480B",
        "Qwen",
        "Qwen3 Coder",
        IN_TEXT,
        NO_FEATURES,
        "2025-07-29T00:00:00Z",
    ),
    text_model(
        "qwen.qwen3-next-80b-a3b",
        "Qwen3 Next 80B",
        "Qwen",
        "Qwen3 Next",
        IN_TEXT,
        NO_FEATURES,
        "2025-10-14T00:00:00Z",
    ),
    text_model(
        "qwen.qwen3-vl-235b-a22b",
        "Qwen3 VL 235B",
        "Qwen",
        "Qwen3 VL",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-11-18T00:00:00Z",
    ),
    // Stability AI — image generation and the image-editing models beside it, each of which
    // takes an image and a prompt and answers with an image.
    image_model(
        "stability.sd3-5-large-v1:0",
        "SD3.5 Large",
        "Stability AI",
        "Stable Diffusion 3",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2024-12-03T00:00:00Z",
    ),
    image_model(
        "stability.stable-conservative-upscale-v1:0",
        "Conservative Upscale",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-creative-upscale-v1:0",
        "Creative Upscale",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    FoundationModel {
        model_id: "stability.stable-diffusion-xl-v1",
        model_name: "SDXL 1.0",
        provider_name: "Stability AI",
        model_family: "SDXL",
        input_modalities: IN_TEXT_AND_IMAGE,
        output_modalities: OUT_IMAGE,
        response_streaming_supported: false,
        customizations_supported: NO_CUSTOMIZATION,
        inference_types_supported: ON_DEMAND_AND_PROVISIONED,
        features_supported: NO_FEATURES,
        lifecycle_status: LIFECYCLE_ACTIVE,
        start_of_life_time: "2023-11-29T00:00:00Z",
        legacy_window: None,
    },
    image_model(
        "stability.stable-fast-upscale-v1:0",
        "Fast Upscale",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-control-sketch-v1:0",
        "Control Sketch",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-control-structure-v1:0",
        "Control Structure",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-core-v1:1",
        "Stable Image Core",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2024-09-05T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-erase-object-v1:0",
        "Erase Object",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-inpaint-v1:0",
        "Inpaint",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-remove-background-v1:0",
        "Remove Background",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-search-recolor-v1:0",
        "Search and Recolor",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-search-replace-v1:0",
        "Search and Replace",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-style-guide-v1:0",
        "Style Guide",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-image-ultra-v1:1",
        "Stable Image Ultra",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2024-09-05T00:00:00Z",
    ),
    image_model(
        "stability.stable-outpaint-v1:0",
        "Outpaint",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    image_model(
        "stability.stable-style-transfer-v1:0",
        "Style Transfer",
        "Stability AI",
        "Stable Image",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-07-15T00:00:00Z",
    ),
    // TwelveLabs reads a clip and answers about it in text.
    text_model(
        "twelvelabs.pegasus-1-2-v1:0",
        "Pegasus 1.2",
        "TwelveLabs",
        "Pegasus",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2025-06-17T00:00:00Z",
    ),
    // Writer.
    text_model(
        "writer.palmyra-vision-7b",
        "Palmyra Vision 7B",
        "Writer",
        "Palmyra Vision",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-02-17T00:00:00Z",
    ),
    text_model(
        "writer.palmyra-x4-v1:0",
        "Palmyra X4",
        "Writer",
        "Palmyra X",
        IN_TEXT,
        NO_FEATURES,
        "2025-04-30T00:00:00Z",
    ),
    text_model(
        "writer.palmyra-x5-v1:0",
        "Palmyra X5",
        "Writer",
        "Palmyra X",
        IN_TEXT,
        NO_FEATURES,
        "2025-04-30T00:00:00Z",
    ),
    // xAI.
    text_model(
        "xai.grok-4.6",
        "Grok 4.6",
        "xAI",
        "Grok 4",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-01-08T00:00:00Z",
    ),
    text_model(
        "xai.grok-4.7",
        "Grok 4.7",
        "xAI",
        "Grok 4",
        IN_TEXT_AND_IMAGE,
        NO_FEATURES,
        "2026-03-24T00:00:00Z",
    ),
    // Z AI.
    text_model(
        "zai.glm-4.7",
        "GLM-4.7",
        "Z AI",
        "GLM-4",
        IN_TEXT,
        NO_FEATURES,
        "2026-01-06T00:00:00Z",
    ),
    text_model(
        "zai.glm-4.7-flash",
        "GLM-4.7 Flash",
        "Z AI",
        "GLM-4",
        IN_TEXT,
        NO_FEATURES,
        "2026-01-06T00:00:00Z",
    ),
    text_model(
        "zai.glm-5",
        "GLM-5",
        "Z AI",
        "GLM-5",
        IN_TEXT,
        NO_FEATURES,
        "2026-05-12T00:00:00Z",
    ),
    text_model(
        "zai.glm-5.3",
        "GLM-5.3",
        "Z AI",
        "GLM-5",
        IN_TEXT,
        NO_FEATURES,
        "2026-09-22T00:00:00Z",
    ),
];

/// The catalog as the per-service store holds it: one entry per `modelId`, keyed by it, so
/// iteration is in `modelId` order and a lookup by id is one map probe.
///
/// A newtype so it SEEDS ITSELF ([`Default`]) out of [`FOUNDATION_MODELS`]: the store
/// around it keeps its derived `Default`, and every handler that reports or resolves a
/// base model reads the catalog through the engine like any other part of this service's
/// state rather than reaching for the constant.
#[derive(Debug, Clone)]
pub struct FoundationModelCatalog(pub(super) BTreeMap<&'static str, FoundationModel>);

impl Default for FoundationModelCatalog {
    fn default() -> Self {
        Self(
            FOUNDATION_MODELS
                .iter()
                .map(|model| (model.model_id, *model))
                .collect(),
        )
    }
}

impl FoundationModelCatalog {
    /// Every model in the catalog, in `modelId` order.
    pub fn models(&self) -> impl Iterator<Item = &FoundationModel> {
        self.0.values()
    }

    /// The model a `modelId` names, or `None` when the catalog carries no such model.
    #[must_use]
    pub fn model(&self, model_id: &str) -> Option<&FoundationModel> {
        self.0.get(model_id)
    }

    /// The model a QUALIFIED base model id names: a `modelId` the catalog carries, or one
    /// of its ids followed by the `:`-separated version qualifiers AWS appends when it
    /// writes a base model id into a resource ARN (`…:0:8k`).
    ///
    /// Those qualifiers are part of the id a `CustomModelArn` embeds — its pattern admits
    /// `([:][a-z0-9-]{1,63}){0,2}` after the model id
    /// (`smithy_query(query="operation",operation="CreateCustomModelDeployment")`) — and
    /// they name a variant of the SAME published model, so the qualified id resolves to
    /// the catalog row its unqualified prefix names. Matching is at a `:` boundary only,
    /// so one model id is never read as a prefix of another.
    #[must_use]
    pub fn qualified_model(&self, qualified_model_id: &str) -> Option<&FoundationModel> {
        let mut candidate = qualified_model_id;
        loop {
            if let Some(model) = self.model(candidate) {
                return Some(model);
            }
            candidate = candidate.rsplit_once(':')?.0;
        }
    }

    /// What a `ModelIdentifier` naming a BASE model names: a model this account's catalog
    /// offers, a model this service still recognises and offers no longer
    /// ([`RETIRED_FOUNDATION_MODELS`]), or no model at all.
    ///
    /// Both spellings the member admits are read — the bare model id and the model's
    /// `foundation-model` ARN ([`base_model_id_of`]) — and the id is resolved the way a base
    /// model id embedded in a resource is ([`Self::qualified_model`]), so a `:`-qualified
    /// variant names the published model it is a variant of.
    ///
    /// The three answers are what let an operation refuse a model it cannot act on for the
    /// right REASON: a model that exists but may not back this request is an eligibility or
    /// authorization decision of that operation, while only an identifier naming no model at
    /// all is a missing resource.
    #[must_use]
    pub fn resolve_base_model(&self, model_identifier: &str) -> BaseModelIdentity<'_> {
        let base_model_id = base_model_id_of(model_identifier);
        if let Some(model) = self.qualified_model(base_model_id) {
            return BaseModelIdentity::Offered(model);
        }
        if RETIRED_FOUNDATION_MODELS.contains(&base_model_id) {
            return BaseModelIdentity::Retired;
        }
        BaseModelIdentity::Unknown
    }
}

/// Whether this account reaches every model the catalog offers without asking for access to
/// one model at a time.
///
/// Reported beside the listing itself, because it is what tells a client whether a model it
/// found in the catalog needs an access request before it can be invoked, or is simply
/// there to be used. Account-level like [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::use_case_for_model_access`],
/// and PUBLISHED rather than created: no operation of this service turns it off, so it
/// seeds itself the way the catalog does ([`FoundationModelCatalog::default`]) and a read
/// of it changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundationModelAccessDefault(pub(super) bool);

impl Default for FoundationModelAccessDefault {
    fn default() -> Self {
        Self(ACCESS_TO_ALL_MODELS_BY_DEFAULT)
    }
}

impl FoundationModelAccessDefault {
    /// Whether access to all models is on for this account.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.0
    }
}

/// An account reaches the models this control plane offers without requesting access to
/// each one: the models that need a provider agreement accepted first say so through their
/// own offer ([`model_agreement_outstanding`]) rather than through this flag, so the flag
/// is the account-level default and not a per-model one.
pub const ACCESS_TO_ALL_MODELS_BY_DEFAULT: bool = true;

/// What a `modelId` naming a base model resolves to
/// ([`FoundationModelCatalog::resolve_base_model`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseModelIdentity<'a> {
    /// A model the published catalog offers: the row itself.
    Offered(&'a FoundationModel),
    /// A model this service recognises and no longer offers, so there is no row to read and
    /// nothing it is offered under.
    Retired,
    /// No model of this service: the identifier resolves to nothing at all.
    Unknown,
}

/// The base model ids this service still RECOGNISES although it offers the model no longer:
/// the ids AWS publishes as past their end of life ("Model lifecycle" in the Amazon Bedrock
/// User Guide).
///
/// A model past its end of life is OFFERED by nobody, which is why [`FOUNDATION_MODELS`]
/// carries no row for one — no listing reports it and no read of the catalog resolves it.
/// Its id goes on naming a model this service knows, though, so a request that names one is
/// not a request whose identifier cannot be read: an operation deciding whether a model may
/// back what it is being asked for refuses it on THAT ground — the model is offered under
/// nothing — rather than reporting that the identifier names no model
/// ([`FoundationModelCatalog::resolve_base_model`]). This emulator's bedrock-runtime plane
/// keeps the same ids in its own inventory for the same reason, where they are answered with
/// the end-of-life refusal rather than an unreadable-identifier one.
pub const RETIRED_FOUNDATION_MODELS: &[&str] = &[
    "ai21.j2-grande-instruct",
    "ai21.j2-jumbo-instruct",
    "ai21.j2-mid-v1",
    "ai21.j2-ultra-v1",
    "amazon.titan-text-express-v1",
    "anthropic.claude-instant-v1",
    "anthropic.claude-v1",
    "anthropic.claude-v2",
    "anthropic.claude-v2:1",
    "meta.llama2-13b-chat-v1",
    "meta.llama2-70b-chat-v1",
    "stability.stable-diffusion-xl-v0",
];

/// The `foundation-model` ARN of `model_id` in `region`:
/// `arn:aws:bedrock:<region>::foundation-model/<modelId>`.
///
/// Built here rather than through `::engine::emulator_arn` because `FoundationModelArn` is
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/…$` — an EMPTY account
/// field, since a foundation model is published by AWS rather than owned by the account
/// reading it, so the canonical account id that helper interpolates would put this ARN
/// outside the pattern the model declares for it.
#[must_use]
pub fn foundation_model_arn(region: &str, model_id: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::foundation-model/{model_id}")
}

/// Whether `by_provider` — a `ListFoundationModels` `byProvider` filter — names `model`'s
/// provider.
///
/// Accepted spellings: the provider's own `providerName` as the catalog reports it, and the
/// provider segment of the `modelId` (everything before its first `.`), which is the
/// lowercase spelling the AWS CLI's own examples pass. Compared case-insensitively,
/// because `Provider` declares a pattern (`^[A-Za-z0-9- ]{1,63}$`) and no case.
#[must_use]
pub fn provider_matches(model: &FoundationModel, by_provider: &str) -> bool {
    let id_provider = model
        .model_id
        .split_once('.')
        .map_or(model.model_id, |(provider, _)| provider);
    by_provider.eq_ignore_ascii_case(model.provider_name)
        || by_provider.eq_ignore_ascii_case(id_provider)
}

// =============================================================================
// The capability projection of a published model
// =============================================================================
//
// Amazon Bedrock reports, beside the ten members `FoundationModelSummary` declares, the
// CAPABILITIES of the model: the family it belongs to, the invocation APIs it is reachable
// through, and whether guardrails, prompt caching, intelligent prompt routing, latency
// optimization and batch inference may be used with it. None of them is declared on the
// on-disk shape (`smithy_query(query="shape", shape="FoundationModelSummary")` lists
// modelArn/modelId/modelName/providerName/inputModalities/outputModalities/
// responseStreamingSupported/customizationsSupported/inferenceTypesSupported/
// modelLifecycle), so they are appended after it — a client decides from them whether it
// may stream, converse, guard, cache, route, batch or customise a model it picked out of
// the listing, which is what makes a listing that omits them undecidable.
//
// Each one is read off the catalog row: the family and the four features of
// [`PUBLISHED_FEATURE_VALUES`] are PUBLISHED per model — each feature has its own
// "Supported models" table, so support for it is a fact about the model and not a
// consequence of what the model does — while the rest follow from attributes the row
// already carries, so they are computed rather than stored a second time. Nothing here is
// invented for a model: a feature no table lists the model in is simply not supported for
// it, which is where a freshly offered model starts.
//
// A capability is reported as a RECORD naming what is supported
// ([`CapabilitySupport`], [`BatchSupport`], [`FeaturesSupport`], [`ConverseSupport`])
// rather than as a bare boolean, because what one of them says about a model is more than
// one bit: batch inference, for instance, runs the base model, runs it from another region
// and runs a customization of it, and each of those is its own answer.
//
// And the capability metadata is published for a model that TAKES PART IN A CONVERSATION
// and for no other: a model that answers with a vector or an image carries the ten members
// the shape declares plus `guardrailsSupported`, and the catalog publishes it no family, no
// invocation APIs, no feature record and no Converse record, so the summary reports none of
// them for it rather than reporting one it made up.

/// `ModelModality` — the modality a model that takes part in a conversation produces
/// (`crate::ops::types::MODEL_MODALITY_VALUES`).
pub(super) const MODALITY_TEXT: &str = "TEXT";

/// The `providerName` whose own OpenAI-compatible endpoints Amazon Bedrock serves its
/// models on beside its own invocation APIs, which is what puts those two APIs in their
/// `inferenceAPIsSupported`.
pub(super) const OPENAI_MODEL_PROVIDER: &str = "OpenAI";

/// The Converse API: Amazon Bedrock's own conversation API.
pub(super) const INFERENCE_API_CONVERSE: &str = "converse";
/// The InvokeModel API: every offered model is reachable through it, whatever it produces.
pub(super) const INFERENCE_API_INVOKE_MODEL: &str = "invokeModel";
/// The OpenAI-compatible chat completions endpoint.
pub(super) const INFERENCE_API_OPENAI_CHAT_COMPLETIONS: &str = "openAiChatCompletions";
/// The OpenAI-compatible responses endpoint.
pub(super) const INFERENCE_API_OPENAI_RESPONSES: &str = "openAiResponses";

/// `featuresSupported` — a model invocation job may run the model over a dataset ("Supported
/// models and Regions for batch inference").
pub(super) const FEATURE_BATCH_INFERENCE: &str = "BATCH_INFERENCE";
/// `featuresSupported` — a prompt router may route a prompt to the model ("Supported models
/// and Regions for intelligent prompt routing").
pub(super) const FEATURE_INTELLIGENT_PROMPT_ROUTING: &str = "INTELLIGENT_PROMPT_ROUTING";
/// `featuresSupported` — the model is offered with latency-optimized inference ("Supported
/// models and Regions for latency-optimized inference").
pub(super) const FEATURE_LATENCY_OPTIMIZATION: &str = "LATENCY_OPTIMIZATION";
/// `featuresSupported` — a prompt prefix may be cached explicitly for the model ("Supported
/// models and Regions for prompt caching").
pub(super) const FEATURE_PROMPT_CACHING: &str = "PROMPT_CACHING";

/// Whether `model` takes part in a conversation: it produces TEXT, which is the one
/// modality a turn of a conversation is made of.
///
/// The precondition of every capability that acts on what a model SAYS — Converse and
/// guardrails — so an embedding or image model, which says nothing a further turn can be
/// built from, has neither.
#[must_use]
pub fn model_is_conversational(model: &FoundationModel) -> bool {
    model.output_modalities.contains(&MODALITY_TEXT)
}

/// Whether a guardrail may be applied to `model`.
///
/// A guardrail evaluates the text a model is given and the text it produces, so it applies
/// to a model that produces text and to no other: there is nothing in a vector or an image
/// for a topic, word or contextual-grounding policy to decide about. Unlike the four
/// features of [`PUBLISHED_FEATURE_VALUES`], this needs nothing of the model beyond that —
/// a guardrail is applied BESIDE the invocation rather than built into the model's
/// integration — so it is decided here rather than published per model.
#[must_use]
pub fn model_guardrails_supported(model: &FoundationModel) -> bool {
    model_is_conversational(model)
}

/// Whether `feature` — one of [`PUBLISHED_FEATURE_VALUES`] — is published for `model`.
pub(super) fn model_publishes_feature(model: &FoundationModel, feature: &str) -> bool {
    model.features_supported.contains(&feature)
}

/// Whether a prompt prefix may be cached explicitly for `model`.
#[must_use]
pub fn model_explicit_prompt_caching(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_PROMPT_CACHING)
}

/// One capability of a model as a summary reports it: the RECORD naming whether that
/// capability may be used with the model, which is the form each capability member of a
/// summary carries rather than a bare boolean.
#[must_use]
pub fn capability_support(is_supported: bool) -> CapabilitySupport {
    CapabilitySupport {
        is_supported: Some(is_supported),
    }
}

/// Whether a prompt router may route a prompt to `model`.
#[must_use]
pub fn model_intelligent_prompt_routing(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_INTELLIGENT_PROMPT_ROUTING)
}

/// Whether `model` is offered with latency-optimized inference.
#[must_use]
pub fn model_latency_optimization_supported(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_LATENCY_OPTIMIZATION)
}

/// Whether a model invocation job may run `model` over a dataset, as the record a summary
/// reports it: a job runs the base model itself, runs it from another region, and runs a
/// CUSTOMIZATION of it — the last of which needs the model to accept a customization at
/// all, so it holds of a model whose `customizationsSupported` is not empty and of no
/// other.
#[must_use]
pub fn model_batch_support(model: &FoundationModel) -> BatchSupport {
    let supported = model_batch_supported(model);
    BatchSupport {
        base_model_supported: Some(supported),
        cross_region_supported: Some(supported),
        custom_model_supported: Some(supported && !model.customizations_supported.is_empty()),
    }
}

/// Whether a model invocation job may run `model` over a dataset.
#[must_use]
pub fn model_batch_supported(model: &FoundationModel) -> bool {
    model_publishes_feature(model, FEATURE_BATCH_INFERENCE)
}

/// The invocation APIs `model` is reachable through, in the order this control plane
/// reports them.
///
/// `invokeModel` reaches every offered model, whatever it produces; `converse` carries a
/// CONVERSATION, so it reaches a conversational model ([`model_is_conversational`]) and no
/// other; and the two OpenAI-compatible endpoints reach a model published by that provider
/// ([`OPENAI_MODEL_PROVIDER`]), whose own API they are the shape of.
#[must_use]
pub fn model_inference_apis_supported(model: &FoundationModel) -> Vec<String> {
    let mut apis: Vec<String> = Vec::new();
    if model_is_conversational(model) {
        apis.push(INFERENCE_API_CONVERSE.to_string());
    }
    apis.push(INFERENCE_API_INVOKE_MODEL.to_string());
    if model.provider_name == OPENAI_MODEL_PROVIDER {
        apis.push(INFERENCE_API_OPENAI_CHAT_COMPLETIONS.to_string());
        apis.push(INFERENCE_API_OPENAI_RESPONSES.to_string());
    }
    apis
}

/// The features published for `model` ([`FoundationModel::features_supported`]), as the
/// record a summary reports them in: one member per feature of
/// [`PUBLISHED_FEATURE_VALUES`], naming whether the catalog publishes that feature for the
/// model, beside the two features that follow from what the model IS rather than from a
/// supported-models table — a token count ([`model_count_tokens_supported`]) and prompt
/// optimization ([`model_prompt_optimization_supported`]) — so a client tests one feature by
/// name.
///
/// The same set the capability members beside it report one by one, so the record and those
/// members cannot disagree: a feature is named supported here exactly while the row
/// publishes it — and a model no feature's supported-models table lists, where a freshly
/// offered model starts, has every member of the record answer no.
#[must_use]
pub fn model_features_supported(model: &FoundationModel) -> FeaturesSupport {
    let published = |feature: &str| Some(model_publishes_feature(model, feature));
    FeaturesSupport {
        batch_inference: published(FEATURE_BATCH_INFERENCE),
        count_tokens: Some(model_count_tokens_supported(model)),
        intelligent_prompt_routing: published(FEATURE_INTELLIGENT_PROMPT_ROUTING),
        latency_optimization: published(FEATURE_LATENCY_OPTIMIZATION),
        prompt_caching: published(FEATURE_PROMPT_CACHING),
        prompt_optimization: Some(model_prompt_optimization_supported(model)),
    }
}

/// Whether a prompt may be OPTIMIZED for `model`.
///
/// An advanced prompt optimization job rewrites a prompt TEMPLATE for the models it is told
/// to target (`modelConfigurations` of
/// `smithy_query(query="operation",operation="CreateAdvancedPromptOptimizationJob")`), so
/// what it needs of a model is that a prompt is what the model is given — it takes part in
/// a conversation — and nothing else.
#[must_use]
pub fn model_prompt_optimization_supported(model: &FoundationModel) -> bool {
    model_is_conversational(model)
}

/// Whether the tokens of a conversation may be COUNTED for `model`.
///
/// A count is the MODEL's own tokenizer's, so the operation that answers one is published
/// per model family rather than per model, and this emulator's inference plane counts for
/// the one family it carries a tokenizer for — the family published by
/// [`COUNT_TOKENS_MODEL_PROVIDER`] — and refuses a model outside it. Reported here so a
/// client reads the same answer out of the catalog that the count itself gives.
#[must_use]
pub fn model_count_tokens_supported(model: &FoundationModel) -> bool {
    model.provider_name == COUNT_TOKENS_MODEL_PROVIDER
}

/// The `providerName` whose models a token count is served for.
pub(super) const COUNT_TOKENS_MODEL_PROVIDER: &str = "Anthropic";

/// What the Converse API supports for `model`, or `None` for a model Converse does not
/// reach at all ([`model_is_conversational`]) — nothing it answers is a turn of a
/// conversation, so the catalog publishes no Converse record for it.
///
/// Reported beside `inferenceAPIsSupported` because Converse is the API a conversational
/// client drives the model through, and every member of the record is something that client
/// has to know before it starts a conversation. Each one is read off the row or off what
/// this emulator's inference plane serves, and a member nothing publishes for a model is
/// left absent rather than filled with a value invented for it:
///
/// * `streamingSupported` is the model's own `responseStreamingSupported`.
/// * `maxTokensMaximum` is the ceiling the provider publishes for the model
///   ([`CONVERSE_MAX_OUTPUT_TOKENS`]), which is the ceiling the inference plane caps a
///   generated answer at; `maxTokensDefault` is absent, because a default for it is
///   published nowhere.
/// * `systemRoleSupported` holds of a model Converse reaches: a system role is part of the
///   conversation the API carries rather than of any one model's integration.
/// * `reasoningSupported` is published per model ([`REASONING_MODELS`]).
/// * `stopSequencesDefault` is empty: a conversation halts at the stop sequences the
///   REQUEST carries, and no model publishes one of its own.
/// * `userDocumentTypesSupported`, `userImageTypesSupported` and
///   `userVideoTypesSupported` are the formats a turn may carry TO the model, so each one
///   is the format set of a modality the model takes ([`FoundationModel::input_modalities`])
///   and empty for a modality it does not.
/// * `additionalRequestFieldsSchema` is absent: the schema of a model's provider-specific
///   request fields is published per model, and this catalog publishes one for no row.
#[must_use]
pub fn model_converse_support(model: &FoundationModel) -> Option<ConverseSupport> {
    if !model_is_conversational(model) {
        return None;
    }
    let takes = |modality: &str| model.input_modalities.contains(&modality);
    let formats = |carried: bool, values: &[&str]| {
        if carried {
            values.iter().map(|value| (*value).to_string()).collect()
        } else {
            Vec::new()
        }
    };
    Some(ConverseSupport {
        supported: Some(true),
        streaming_supported: Some(model.response_streaming_supported),
        additional_request_fields_schema: None,
        max_tokens_default: None,
        max_tokens_maximum: model_max_output_tokens(model),
        reasoning_supported: Some(capability_support(model_reasoning_supported(model))),
        stop_sequences_default: Some(Vec::new()),
        system_role_supported: Some(true),
        user_document_types_supported: Some(formats(takes(MODALITY_TEXT), CONVERSE_DOCUMENT_FORMATS)),
        user_image_types_supported: Some(formats(takes(MODALITY_IMAGE), CONVERSE_IMAGE_FORMATS)),
        user_video_types_supported: Some(formats(takes(MODALITY_VIDEO), CONVERSE_VIDEO_FORMATS)),
    })
}

/// The ceiling on generated tokens the provider publishes for `model` ("Inference parameters
/// for foundation models" in the Amazon Bedrock User Guide), or `None` for a model it
/// publishes none for.
#[must_use]
pub fn model_max_output_tokens(model: &FoundationModel) -> Option<i32> {
    CONVERSE_MAX_OUTPUT_TOKENS
        .iter()
        .find(|(model_id, _)| *model_id == model.model_id)
        .map(|(_, ceiling)| *ceiling)
}

/// Whether the provider publishes EXTENDED REASONING for `model`: the model answers with a
/// reasoning block beside its text, which a client has to ask for and account tokens for.
#[must_use]
pub fn model_reasoning_supported(model: &FoundationModel) -> bool {
    REASONING_MODELS.contains(&model.model_id)
}

/// The ceiling on generated tokens each model's provider publishes for it, by `modelId`:
/// the largest `maxTokens` a Converse request may ask of the model.
///
/// The same ceiling this emulator's inference plane caps a generated answer at (the
/// `max_output_tokens` of its own model inventory), so a client that reads the ceiling here
/// and then asks for it is served rather than refused. A model whose provider publishes no
/// ceiling is in no row, and its Converse record reports none.
pub(super) const CONVERSE_MAX_OUTPUT_TOKENS: &[(&str, i32)] = &[
    ("amazon.nova-lite-v1:0", 5120),
    ("amazon.nova-micro-v1:0", 5120),
    ("amazon.nova-pro-v1:0", 5120),
    ("amazon.titan-text-lite-v1", 4096),
    ("amazon.titan-text-premier-v1:0", 3072),
    ("anthropic.claude-3-5-sonnet-20240620-v1:0", 8192),
    ("anthropic.claude-3-5-sonnet-20241022-v2:0", 8192),
    ("anthropic.claude-3-haiku-20240307-v1:0", 4096),
    ("anthropic.claude-3-sonnet-20240229-v1:0", 4096),
    ("cohere.command-light-text-v14", 4096),
    ("cohere.command-text-v14", 4096),
    ("meta.llama3-1-70b-instruct-v1:0", 2048),
    ("meta.llama3-1-8b-instruct-v1:0", 2048),
    ("meta.llama3-3-70b-instruct-v1:0", 2048),
    ("meta.llama3-70b-instruct-v1:0", 2048),
    ("meta.llama3-8b-instruct-v1:0", 2048),
    ("mistral.mistral-7b-instruct-v0:2", 8192),
    ("mistral.mistral-large-2402-v1:0", 8192),
    ("mistral.mistral-large-2407-v1:0", 8192),
    ("mistral.mixtral-8x7b-instruct-v0:1", 4096),
];

/// The models whose provider publishes EXTENDED REASONING for them, by `modelId`: the
/// reasoning lines of each provider — the Anthropic Claude lines from Claude 4 on, which
/// publish extended thinking, and the lines each other provider publishes as its reasoning
/// model.
pub(super) const REASONING_MODELS: &[&str] = &[
    "anthropic.claude-fable-5",
    "anthropic.claude-fable-5-1",
    "anthropic.claude-haiku-4-5-20251001-v1:0",
    "anthropic.claude-opus-4-1-20250805-v1:0",
    "anthropic.claude-opus-4-5-20251101-v1:0",
    "anthropic.claude-opus-4-6-v1",
    "anthropic.claude-opus-4-7",
    "anthropic.claude-opus-4-8",
    "anthropic.claude-opus-5",
    "anthropic.claude-opus-5-5",
    "anthropic.claude-sonnet-4-20250514-v1:0",
    "anthropic.claude-sonnet-4-5-20250929-v1:0",
    "anthropic.claude-sonnet-4-6",
    "anthropic.claude-sonnet-5",
    "anthropic.claude-sonnet-5-5",
    "deepseek.r1-v1:0",
    "mistral.magistral-small-2509",
    "moonshot.kimi-k2-thinking",
    "nvidia.nemotron-super-3-120b",
    "qwen.qwen3-235b-a22b-2507-v1:0",
    "qwen.qwen3-32b-v1:0",
    "qwen.qwen3-next-80b-a3b",
];

/// The document formats a turn of a conversation may carry to a model that takes TEXT
/// (`DocumentFormat` in the Amazon Bedrock Runtime model, which is what `Converse` accepts
/// a document block in).
pub(super) const CONVERSE_DOCUMENT_FORMATS: &[&str] = &[
    "pdf", "csv", "doc", "docx", "xls", "xlsx", "html", "txt", "md",
];

/// The image formats a turn may carry to a model that takes IMAGE (`ImageFormat`).
pub(super) const CONVERSE_IMAGE_FORMATS: &[&str] = &["png", "jpeg", "gif", "webp"];

/// The video formats a turn may carry to a model that takes video (`VideoFormat`). The
/// modality is written as the rule the other two are — the formats of a modality the model
/// takes — and no value `ModelModality` declares
/// (`crate::ops::types::MODEL_MODALITY_VALUES`) is video, so no row of this catalog takes
/// one and the member is empty for every model today.
pub(super) const CONVERSE_VIDEO_FORMATS: &[&str] = &[
    "mkv", "mov", "mp4", "webm", "flv", "mpeg", "mpg", "wmv", "three_gp",
];

/// `ModelModality` — the modality of a model that takes an image.
pub(super) const MODALITY_IMAGE: &str = "IMAGE";

/// The modality of a model that takes a video clip. `ModelModality` declares no such value
/// in this model copy, so the catalog carries it on no row; it is named so the Converse
/// record states the same RULE for all three format sets.
pub(super) const MODALITY_VIDEO: &str = "VIDEO";

/// One catalog row as the `FoundationModelSummary` a `ListFoundationModels` response for
/// `region` carries.
///
/// Lives here rather than in `ops/list_foundation_models.rs` because every member it
/// carries is also a member `GetFoundationModel` reports
/// ([`foundation_model_details`], which is built out of it), and the two must agree about
/// them: a listed summary is the same row the read of that one model finds.
///
/// `modelLifecycle` reports the row's `status`, the row's launch instant
/// ([`FoundationModel::start_of_life_time`], which every offered model has) and, for a
/// deprecated model, both ends of the legacy window AWS publishes for it
/// ([`FoundationModel::legacy_window`]): when it entered that state and when it stops being
/// available. `publicExtendedAccessTime` is left out, because nothing publishes it per
/// model: it is part of a legacy window AWS states no instant for.
///
/// The members after `modelLifecycle` are the model's CAPABILITIES, each computed from this
/// row — see the capability section above for what each one takes of a model, and why only
/// a model that takes part in a conversation carries them.
#[must_use]
pub fn foundation_model_summary(model: &FoundationModel, region: &str) -> FoundationModelSummary {
    let list = |values: &[&str]| values.iter().map(|value| (*value).to_string()).collect();
    let conversational = model_is_conversational(model);
    FoundationModelSummary {
        model_arn: foundation_model_arn(region, model.model_id),
        model_id: model.model_id.to_string(),
        model_name: Some(model.model_name.to_string()),
        provider_name: Some(model.provider_name.to_string()),
        input_modalities: Some(list(model.input_modalities)),
        output_modalities: Some(list(model.output_modalities)),
        response_streaming_supported: Some(model.response_streaming_supported),
        customizations_supported: Some(list(model.customizations_supported)),
        inference_types_supported: Some(list(model.inference_types_supported)),
        model_lifecycle: Some(FoundationModelLifecycle {
            status: model.lifecycle_status.to_string(),
            start_of_life_time: published_instant(model.start_of_life_time),
            end_of_life_time: model
                .legacy_window
                .and_then(|window| published_instant(window.end_of_life_at)),
            legacy_time: model
                .legacy_window
                .and_then(|window| published_instant(window.entered_at)),
            public_extended_access_time: None,
        }),
        model_family: conversational.then(|| model.model_family.to_string()),
        inference_apis_supported: conversational.then(|| model_inference_apis_supported(model)),
        features_supported: conversational.then(|| model_features_supported(model)),
        guardrails_supported: Some(model_guardrails_supported(model)),
        explicit_prompt_caching: conversational
            .then(|| capability_support(model_explicit_prompt_caching(model))),
        intelligent_prompt_routing: conversational
            .then(|| capability_support(model_intelligent_prompt_routing(model))),
        latency_optimization_supported: conversational
            .then(|| model_latency_optimization_supported(model)),
        batch_supported: conversational.then(|| model_batch_support(model)),
        converse: model_converse_support(model),
    }
}

/// A PUBLISHED instant of the catalog — one written in this file as the RFC 3339 day AWS
/// publishes for it — as the wire timestamp type the response shapes carry.
///
/// `None` only for a value that is not an instant at all, which no row of
/// [`FOUNDATION_MODELS`] carries: a row's instants are pinned by this crate's own suites, so
/// a misspelled one fails there rather than being reported as an absent date.
///
/// Reached by the catalogues built OVER the models too (`state/foundation_models/
/// published_routing.rs`), whose own instants are the launch instants of the models they
/// are published over.
pub(super) fn published_instant(published: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(published, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The published row `model_id` names, read off [`FOUNDATION_MODELS`] itself.
///
/// The catalog is published data, so a row is the same for every account and reachable
/// without a store: this is what the catalogues built OVER the models — the cross-region
/// inference profiles and the default prompt routers — read a target model's published
/// attributes from, rather than carrying a second copy of them. `None` for an id the
/// catalog offers no model under, which is what makes a target model nothing can be
/// published for visible instead of invented.
#[must_use]
pub fn catalogued_model(model_id: &str) -> Option<&'static FoundationModel> {
    FOUNDATION_MODELS
        .iter()
        .find(|model| model.model_id == model_id)
}

/// The same catalog row as the `FoundationModelDetails` a `GetFoundationModel` response
/// for `region` carries.
///
/// `FoundationModelDetails` and `FoundationModelSummary` carry the same members, and
/// a read of a model must report what a listing reports for it, so this moves the ONE
/// mapping [`foundation_model_summary`] computes into the shape this response declares
/// rather than computing the members a second time.
#[must_use]
pub fn foundation_model_details(model: &FoundationModel, region: &str) -> FoundationModelDetails {
    let summary = foundation_model_summary(model, region);
    FoundationModelDetails {
        model_arn: summary.model_arn,
        model_id: summary.model_id,
        model_name: summary.model_name,
        provider_name: summary.provider_name,
        input_modalities: summary.input_modalities,
        output_modalities: summary.output_modalities,
        response_streaming_supported: summary.response_streaming_supported,
        customizations_supported: summary.customizations_supported,
        inference_types_supported: summary.inference_types_supported,
        model_lifecycle: summary.model_lifecycle,
        model_family: summary.model_family,
        inference_apis_supported: summary.inference_apis_supported,
        features_supported: summary.features_supported,
        guardrails_supported: summary.guardrails_supported,
        explicit_prompt_caching: summary.explicit_prompt_caching,
        intelligent_prompt_routing: summary.intelligent_prompt_routing,
        latency_optimization_supported: summary.latency_optimization_supported,
        batch_supported: summary.batch_supported,
        converse: summary.converse,
    }
}

// =============================================================================
// The agreement offers of a published model
// =============================================================================

/// `OfferType` — every offer associated with the model.
pub const OFFER_TYPE_ALL: &str = "ALL";

/// `OfferType` — only the offers that are publicly available, as against an offer agreed
/// privately with a provider.
pub const OFFER_TYPE_PUBLIC: &str = "PUBLIC";

/// The `providerName` of the models AWS itself publishes.
///
/// It is the one provider whose models an account reaches without accepting another
/// party's terms, which is what [`model_agreement_offers`] reads it for.
pub const AWS_MODEL_PROVIDER: &str = "Amazon";

/// Whether `model` is offered under an AGREEMENT at all: a set of terms the account accepts
/// before it may use the model.
///
/// A model from a THIRD-PARTY provider is published under that provider's own end-user
/// license agreement, which is the agreement an offer is an offer OF; the models AWS
/// publishes itself ([`AWS_MODEL_PROVIDER`]) are reached without accepting another party's
/// terms, so there is no agreement for an offer of them to be about.
///
/// Stated once here because it decides two things that must not drift apart: which models
/// expose an offer ([`model_agreement_offers`]), and which `modelId` the offers listing can
/// read as the identifier of an agreement-bearing model at all.
#[must_use]
pub fn model_offered_under_agreement(model: &FoundationModel) -> bool {
    model.provider_name != AWS_MODEL_PROVIDER
}

/// The offers associated with `model`, narrowed to `offer_type` when the request names one.
///
/// A model from a THIRD-PARTY provider is offered under that provider's own end-user
/// license agreement, which the account accepts — by the `offerToken` of one of these
/// offers — before it may use the model; the models AWS publishes itself
/// ([`AWS_MODEL_PROVIDER`]) carry no such agreement, so they expose no offer
/// ([`model_offered_under_agreement`]).
///
/// Every offer here is PUBLIC: this emulator holds no privately agreed offer, so
/// [`OFFER_TYPE_ALL`] and [`OFFER_TYPE_PUBLIC`] admit the same one offer per model, and a
/// request naming no `offerType` gets every offer the model has.
///
/// The TERMS a model is offered on are the provider's published document, price list and
/// support policy, none of which this emulator carries — and every member of `LegalTerm`,
/// `SupportTerm`, `ValidityTerm` and `DimensionalPriceRate` is optional
/// (`smithy_query(query="shape",shape="TermDetails")`) — so the term structures an offer
/// carries are present and name no document, price or duration rather than inventing one.
#[must_use]
pub fn model_agreement_offers(model: &FoundationModel, offer_type: Option<&str>) -> Vec<Offer> {
    if !model_offered_under_agreement(model) {
        return Vec::new();
    }
    // The one offer a third-party model is published under is PUBLIC, so it is admitted by
    // either value the filter can carry, and by a request that carries no filter at all.
    let admitted = offer_type
        .is_none_or(|wanted| wanted == OFFER_TYPE_ALL || wanted == OFFER_TYPE_PUBLIC);
    if !admitted {
        return Vec::new();
    }
    let offer_id = model_offer_id(model.model_id);
    vec![Offer {
        offer_token: model_offer_token(model.model_id, &offer_id),
        offer_id: Some(offer_id),
        term_details: TermDetails {
            usage_based_pricing_term: PricingTerm {
                rate_card: Vec::new(),
            },
            legal_term: LegalTerm { url: None },
            support_term: SupportTerm {
                refund_policy_description: None,
            },
            validity_term: None,
        },
    }]
}

/// Whether this account has still to accept an agreement before it may use `model`.
///
/// An OUTSTANDING offer is exactly that statement: a model published under its provider's
/// own end-user license agreement is offered to the account until the account accepts those
/// terms, while a model AWS publishes itself is offered under none and so has no offer to
/// accept ([`model_agreement_offers`]). Read off the same offers a listing reports, so the
/// agreement a read of the model's availability describes is the one a caller can act on.
#[must_use]
pub fn model_agreement_outstanding(model: &FoundationModel) -> bool {
    !model_agreement_offers(model, None).is_empty()
}

/// `AgreementStatus` — the agreement this account needs for the model is in place, which
/// includes a model that needs none.
pub const AGREEMENT_STATUS_AVAILABLE: &str = "AVAILABLE";

/// `AgreementStatus` — the account holds no agreement for the model, so an offer of the
/// model's terms is still outstanding ([`model_agreement_outstanding`]).
pub const AGREEMENT_STATUS_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// `AuthorizationStatus` — the account may use the model.
pub const AUTHORIZATION_STATUS_AUTHORIZED: &str = "AUTHORIZED";

/// `AuthorizationStatus` — the account may not use the model, because the terms it is
/// offered under have not been accepted.
pub const AUTHORIZATION_STATUS_NOT_AUTHORIZED: &str = "NOT_AUTHORIZED";

/// `EntitlementAvailability` and `RegionAvailability` both spell their positive value this
/// way: the entitlement the account would use the model under exists, and the model is
/// offered in the region the request addressed.
pub const AVAILABILITY_AVAILABLE: &str = "AVAILABLE";

/// The same value set's negative: no entitlement, or no offering in the region.
pub const AVAILABILITY_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// The `offerId` of the public offer of `model_id`.
///
/// Derived from the model the offer is FOR, so the id a listing reports for a model is the
/// same id every later listing reports for it: an offer is not a resource a request brings
/// into being, so nothing about it is per-run.
#[must_use]
pub fn model_offer_id(model_id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "offer-{}",
        &hex::encode(Sha256::digest(model_id.as_bytes()))[..12]
    )
}

/// The `offerToken` of the offer `offer_id` of `model_id`: the opaque token
/// `CreateFoundationModelAgreement` takes to accept exactly this offer.
///
/// Derived from what the offer IS — the model and the offer id — for the reason
/// [`model_offer_id`] is: the token a listing hands out has to be the token a later
/// agreement call can be checked against, so it is computed from the offer rather than
/// minted per run.
#[must_use]
pub fn model_offer_token(model_id: &str, offer_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(model_id.as_bytes());
    digest.update(b":");
    digest.update(offer_id.as_bytes());
    hex::encode(digest.finalize())
}

/// What this control plane answers when a `modelId` names no model in the catalog: the
/// identifier could not be resolved to a model.
///
/// Stated once here because every operation that resolves a `BedrockModelId` against the
/// published catalog makes the same refusal for the same reason, and two spellings of it
/// could drift.
#[must_use]
pub fn foundation_model_not_found(identifier: &str) -> String {
    format!("Could not resolve the model from the provided model identifier: {identifier}")
}

/// Why a `modelIdentifier` names no model the catalog offers, and so WHICH refusal the read
/// of it answers.
///
/// Three different faults hide behind "the catalog has no row for it", and Amazon Bedrock
/// tells them apart rather than reporting one of them for all three:
///
/// * a model the service recognises and no longer offers is a resource that EXISTED and is
///   gone: `ResourceNotFoundException` (404), carrying the end-of-life reason;
/// * an ARN naming no model is an ARN to report missing: `ResourceNotFoundException` (404),
///   which is what that error documents itself as ("The specified resource Amazon Resource
///   Name (ARN) was not found");
/// * a bare model id naming no model is a value this service cannot read as a model at all,
///   which is an INPUT fault: `ValidationException` (400) carrying the fixed
///   [`INVALID_MODEL_IDENTIFIER_MESSAGE`].
///
/// Keeping them apart is what lets a caller act on the answer: 400 says "fix the request",
/// where 404 is routinely retried as eventual consistency. It is the same split the
/// inference plane makes for the same member, read here off the published catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnofferedFoundationModel {
    /// A model past its end of life ([`RETIRED_FOUNDATION_MODELS`]).
    PastEndOfLife,
    /// A `foundation-model` ARN of this service's namespace naming no model.
    MissingArn,
    /// A bare model id naming no model.
    UnreadableIdentifier,
}

impl UnofferedFoundationModel {
    /// The sentence this refusal carries. Fixed text for the two readings whose fault is
    /// the CLASS of the identifier rather than the value in it; the missing ARN echoes the
    /// ARN the request addressed, because that is the resource it reports absent.
    #[must_use]
    pub fn message(self, identifier: &str) -> String {
        match self {
            Self::PastEndOfLife => FOUNDATION_MODEL_END_OF_LIFE_MESSAGE.to_string(),
            Self::MissingArn => foundation_model_not_found(identifier),
            Self::UnreadableIdentifier => INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
        }
    }
}

/// How `identifier` — a `modelIdentifier` the catalog resolved to no offered model
/// ([`FoundationModelCatalog::resolve_base_model`]) — is refused.
///
/// Stated once because the reading belongs to the published CATALOG rather than to one
/// operation: every read that resolves a model identifier against it refuses an identifier
/// it cannot resolve the same way. The two readings that are not about the model's LIFECYCLE
/// are the spelling split every model-identifying member shares
/// ([`unresolved_model`]), read here against the one family this catalogue resolves, so the
/// split exists once.
#[must_use]
pub fn unoffered_foundation_model(identifier: &str) -> UnofferedFoundationModel {
    if RETIRED_FOUNDATION_MODELS.contains(&base_model_id_of(identifier)) {
        return UnofferedFoundationModel::PastEndOfLife;
    }
    match unresolved_model(identifier, &[FOUNDATION_MODEL_RESOURCE_TYPE]) {
        UnresolvedModel::MissingResource(_) => UnofferedFoundationModel::MissingArn,
        UnresolvedModel::UnreadableIdentifier => UnofferedFoundationModel::UnreadableIdentifier,
    }
}

/// The resource type a `foundation-model` ARN names, as `FoundationModelArn`'s pattern
/// spells it (`…::foundation-model/<modelId>`) — what tells that spelling of a
/// `modelIdentifier` from the bare model id the same member admits ([`names_arn_of`]).
pub const FOUNDATION_MODEL_RESOURCE_TYPE: &str = "foundation-model";

/// What this control plane answers for a model id whose model has reached end of life.
///
/// Taken from the Bedrock Runtime emulator, which states the same refusal for the same
/// class on the inference surface, rather than spelled a second time here: one wording for
/// one sentence, exactly as [`INVALID_MODEL_IDENTIFIER_MESSAGE`] is shared.
pub const FOUNDATION_MODEL_END_OF_LIFE_MESSAGE: &str =
    ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::END_OF_LIFE_MESSAGE;

// =============================================================================
// The published default prompt router catalog
// =============================================================================
//
// Amazon Bedrock publishes the `default` prompt routers itself — `PromptRouterType` is
// `[custom, default]` and `ListPromptRouters$type` DEFAULTS to `default`
// (`smithy_query(query="operation",operation="ListPromptRouters")`) — and there is no
// operation that creates one, so these rows are published data the same way
// [`FOUNDATION_MODELS`] is: the same for every account, written by no handler. A caller
// discovers the routable model sets from them before building a router of its own.
