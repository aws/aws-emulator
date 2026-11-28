// SPDX-License-Identifier: CC0-1.0

//! The task envelope `StartSpeechSynthesisTask` answers with, and the one member of it
//! the on-disk model does not declare.
//!
//! WHY THIS MODULE EXISTS. `#[smithy_macros::smithy(shape = ...)]` derives a structure's
//! `Schema`, its serializer and its deserializer from the on-disk model's member list,
//! and it admits EXACTLY that list: a Rust field with no member behind it is a compile
//! error, so a member the service really puts on the wire but the model omits cannot be
//! declared through the derive at all. The task-creation response reports, beside the
//! task's modelled members, the SDK feature toggles the service applied to the request —
//! a list, serialized whether or not anything is in it — and the model's `SynthesisTask`
//! declares no such member, which is why this envelope's wire form is written out here.
//!
//! WHAT IS NOT WRITTEN OUT: the modelled members. The envelope HOLDS the model's own
//! [`SynthesisTask`] and serializes it through the derived codec, so there is one
//! projection of a stored task ([`crate::state::synthesis_task`]) and one codec for it
//! behind every operation that reports a task, and the creation response cannot drift
//! from the task reads about any modelled member. Only the extra member is written here.
//!
//! This module is HAND-WRITTEN, so it is declared from `src/ops/types_local/mod.rs`
//! rather than from the `@generated` `types/mod.rs` a model update rewrites.
//!
//! The member the model does not declare is appended AFTER the model's own, with its
//! shape id in a namespace of its own ([`RECORDED_NAMESPACE`]), so it can neither claim
//! to be modelled nor collide with a member the model may declare later — a model
//! refresh only has to move it into place and put the derive back. The ids never leave
//! the process: the codec writes a member's NAME as the key.
//!
//! WHAT IS DELIBERATELY ABSENT, for the same reason the derive's own output would be
//! inert here: the constraint traits (the validating walk runs on operation INPUTS only,
//! and this envelope is output-only), the `@http` chain (baked on only for an
//! operation's input) and `@jsonName` (the member's wire name IS its member name).
//!
//! Nothing here decides a VALUE: the modelled members come from the stored task, and the
//! toggle list from [`StartedSynthesisTask::reporting`].

use ::aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::aws_smithy_schema_server::DeserializableStruct;

use crate::ops::types::SynthesisTask;

/// The namespace the member the on-disk model does not declare sits in: NOT this
/// service's own, so a hand-written member can never be mistaken for a modelled one.
const RECORDED_NAMESPACE: &str = "com.amazonaws.polly.recorded";

/// The `SynthesisTask` object a task-creation response carries: the task as the model
/// declares it, plus the SDK feature toggles the service applied to the request that
/// started it.
#[derive(Debug, Clone, Default)]
pub struct StartedSynthesisTask {
    /// The task's modelled members, exactly as every other reader of a task reports
    /// them.
    pub task: SynthesisTask,
    /// The toggles the service applied, reported whether or not there are any.
    pub sdk_feature_toggles: Vec<String>,
}

impl StartedSynthesisTask {
    /// The one member the model does not declare, appended after the model's own
    /// fifteen (the index is its position in the member list the wire carries) and in
    /// [`RECORDED_NAMESPACE`].
    pub const M_SDK_FEATURE_TOGGLES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.polly.recorded#SynthesisTask$SdkFeatureToggles",
            RECORDED_NAMESPACE,
            "SdkFeatureToggles",
        ),
        ShapeType::List,
        "SdkFeatureToggles",
        15,
    );

    /// The envelope that reports `task`, with the toggles the request it was started
    /// from turned on.
    ///
    /// `toggles` is what the request asked the service to apply: this service's input
    /// declares nothing that turns a toggle on, so a caller passes the toggles it
    /// resolved — an empty list — rather than this constructor inventing one.
    pub fn reporting(task: SynthesisTask, toggles: Vec<String>) -> Self {
        Self {
            task,
            sdk_feature_toggles: toggles,
        }
    }
}

impl SerializableStruct for StartedSynthesisTask {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        // The modelled members, through the derived codec: one definition of how a task
        // is written, shared with every operation that reports one.
        self.task.serialize_members(&mut *s)?;
        // Then the member the model does not declare. Written unconditionally — a list
        // with nothing in it is the empty list, not an absent member. A scalar element
        // is written against the LIST member's own schema, which is where the codec
        // reads the element layout from and what the derive emits for a list of
        // strings; only a structure element carries a schema of its own.
        s.write_list(&Self::M_SDK_FEATURE_TOGGLES, &|element| {
            for toggle in self.sdk_feature_toggles.iter() {
                element.write_string(&Self::M_SDK_FEATURE_TOGGLES, toggle.as_str())?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

impl DeserializableStruct for StartedSynthesisTask {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        // A reader dispatches a key by looking it up among the members of the shape it
        // reads against, and the shape of a task is the model's — which declares no
        // toggle member, so a model-driven reader (every generated SDK included) skips
        // that key and keeps the modelled envelope. This reads exactly that.
        Ok(Self {
            task: SynthesisTask::deserialize(d)?,
            sdk_feature_toggles: Vec::new(),
        })
    }
}
