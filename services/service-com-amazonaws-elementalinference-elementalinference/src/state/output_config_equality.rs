// SPDX-License-Identifier: CC0-1.0

//! Value equality for the `OutputConfig` tree.
//!
//! An output of a feed is compared member by member in several places of this service:
//! it is what tells an output a request would really append from one the feed already
//! holds exactly as asked ([`crate::state::Feed::outputs_to_append`]), and what tells a
//! create this Region has already served from a new one
//! ([`crate::state::ComAmazonawsElementalinferenceElementalinferenceState::feed_created_from`]).
//! The configuration is part of that record, so two configurations have to be
//! comparable.
//!
//! The shapes themselves are rendered from the Smithy model (`crate::ops::types`), which
//! derives only `Debug`, `Clone` and `Default` on them, and is re-rendered by the next
//! model update — so the comparison lives HERE, beside the record it serves, rather than
//! as an edit to a generated file that the next update would drop. Each impl below
//! compares exactly the shape's own members, which is structural equality: two configs
//! are equal when they describe the same configuration.

use crate::ops::types::*;

/// Structural `PartialEq`/`Eq` for a generated structure shape, over the members named.
///
/// Every member of these shapes is itself comparable (a string, an integer, or another
/// shape this module gives an impl to), so comparing them all is value equality for the
/// shape.
macro_rules! structural_eq {
    ($shape:ty { $($member:ident),* $(,)? }) => {
        impl PartialEq for $shape {
            fn eq(&self, _other: &Self) -> bool {
                true $(&& self.$member == _other.$member)*
            }
        }
        impl Eq for $shape {}
    };
}

/// Structural `PartialEq`/`Eq` for a generated union shape: equal only when both sides
/// carry the SAME variant and the two payloads are equal, so a config of one feature is
/// never equal to a config of another.
macro_rules! structural_union_eq {
    ($shape:ty { $($variant:ident),* $(,)? }) => {
        impl PartialEq for $shape {
            fn eq(&self, other: &Self) -> bool {
                match (self, other) {
                    $((Self::$variant(ours), Self::$variant(theirs)) => ours == theirs,)*
                    _ => false,
                }
            }
        }
        impl Eq for $shape {}
    };
}

structural_union_eq!(OutputConfig {
    Cropping,
    Clipping,
    Subtitling,
    contextualMetadata,
    Function,
});

structural_eq!(CroppingConfig { template_groups });
structural_eq!(TemplateGroup { name, template_uris });
structural_eq!(ClippingConfig {
    callback_metadata,
    data_source_configuration,
});
structural_eq!(DataSourceConfiguration { fixture_id });
structural_eq!(SubtitlingConfig {
    language,
    aspect_ratio,
    dictionary,
    profanity_filter,
});
structural_eq!(AspectRatio { width, height });
structural_eq!(ContextualMetadataConfig {
    summary_generation,
    extended_analysis,
    custom_questions,
});
structural_eq!(CustomQuestionsConfig { shot, scene });
structural_eq!(QuestionSetReference {
    question_set_id,
    version,
});
structural_eq!(FunctionConfig {
    function_arn,
    trigger,
    frame_settings,
    subtitling_settings,
    contextual_metadata_settings,
    clipping_settings,
});
structural_union_eq!(Trigger {
    fixedInterval,
    perShot,
    perScene,
    perClip,
});
structural_eq!(FixedIntervalTrigger { interval_seconds });
structural_eq!(ShotTrigger {});
structural_eq!(SceneTrigger {});
structural_eq!(ClipTrigger {});
structural_eq!(FrameSettings {
    frame_sampling,
    frame_size,
});
structural_eq!(FrameSamplingStrategy { mode, count });
structural_eq!(FrameSize {
    width_pixels,
    height_pixels,
});
structural_eq!(SubtitlingSettings { output_name });
structural_eq!(ContextualMetadataSettings { output_name });
structural_eq!(ClippingSettings { output_name });
