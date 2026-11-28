// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_medical_scribe_listening_session`
//! operation: the header-bound halves of its input and output, and the event
//! structures its `@streaming` input union carries.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules. The shapes the implemented `GetMedicalScribeListeningSession` already declares
//! (`MedicalScribeChannelDefinition`, the `*Response` projections of the post-stream
//! settings, every exception) are reused from `get_medical_scribe_listening_session_types`
//! and `shared_types` rather than re-declared here.
//!
//! HAND-WRITTEN: the scaffold generator skips this operation (its input is a `@streaming`
//! union it emits no framing for), so `types/mod.rs` declares no module for it. The
//! declaration lives in the operation's own file instead of in that generated, rewritten-on
//! -every-update one, which is why the shapes this module reaches are named through
//! `crate::ops::types` rather than through its parent.
//!
//! ## Why the `Settings` / `Headers` pair exists
//!
//! `StartMedicalScribeListeningSession` binds every input member but one to an
//! `@httpHeader`; the remaining member is the `@streaming` event-stream
//! `@httpPayload`, whose framing the generated codec does not speak. Declaring the
//! request shape with that one member in `unsupported = [..]` gives a struct the
//! framework's own HTTP-binding deserializer fills from the request headers and
//! `constraints::validate` checks against the model's `@range` / `@pattern` /
//! `@length` / enum-membership traits — so the header half of the request is
//! validated by the MODEL, not by hand — while the payload half is decoded from real
//! `vnd.amazon.eventstream` frames in the operation itself. The response is the same
//! split: the `Headers` struct serializes the modelled response headers and the event
//! stream is the body.
//!
//! No OUTPUT-stream event structure is declared here. That is deliberate: this
//! emulator runs no speech recogniser, so no `MedicalScribeTranscriptEvent` is ever
//! constructed (see the operation's own module documentation); the one in-band member
//! it does frame, `InternalServerException`, is already declared in `shared_types`.
#![allow(unused_imports)]

use crate::ops::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-connecthealth-connecthealth.json");

/// The members of `MedicalScribeInputStream`, as the `:event-type` header of an
/// input frame spells them
/// (`smithy_query(query="shape", shape="MedicalScribeInputStream")`).
pub const MEMBER_CONFIGURATION_EVENT: &str = "configurationEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_AUDIO_EVENT: &str = "audioEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_BINARY_AUDIO_EVENT: &str = "binaryAudioEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_SESSION_CONTROL_EVENT: &str = "sessionControlEvent";

/// The member of `MedicalScribeOutputStream` this service frames into a response
/// stream (`smithy_query(query="shape", shape="MedicalScribeOutputStream")`).
pub const MEMBER_INTERNAL_FAILURE_EXCEPTION: &str = "internalFailureException";

/// The one value `MedicalScribeSessionControlEventType` accepts: the event that ends
/// a listening session
/// (`smithy_query(query="shape", shape="MedicalScribeSessionControlEventType")`).
pub const END_OF_SESSION: &str = "END_OF_SESSION";

/// The header-bound half of `StartMedicalScribeListeningSessionInput`; `inputStream`
/// is the event-stream payload and is decoded frame by frame instead.
#[smithy_macros::smithy(
    shape = "com.amazonaws.connecthealth#StartMedicalScribeListeningSessionInput",
    unsupported = ["inputStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartMedicalScribeListeningSessionSettings {
    pub session_id: String,
    pub domain_id: String,
    pub subscription_id: String,
    pub language_code: String,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
}

/// The header-bound half of `StartMedicalScribeListeningSessionOutput`;
/// `responseStream` is the event-stream payload.
#[smithy_macros::smithy(
    shape = "com.amazonaws.connecthealth#StartMedicalScribeListeningSessionOutput",
    unsupported = ["responseStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartMedicalScribeListeningSessionHeaders {
    pub session_id: Option<String>,
    pub domain_id: Option<String>,
    pub subscription_id: Option<String>,
    pub request_id: Option<String>,
    pub language_code: Option<String>,
    pub media_sample_rate_hertz: Option<i32>,
    pub media_encoding: Option<String>,
}

/// The event that configures a listening session: everything
/// `GetMedicalScribeListeningSession` reports about how the session was set up comes
/// from here.
#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#MedicalScribeConfigurationEvent")]
#[derive(Debug, Clone)]
pub struct MedicalScribeConfigurationEvent {
    pub post_stream_action_settings: MedicalScribePostStreamActionSettings,
    pub channel_definitions: Option<Vec<MedicalScribeChannelDefinition>>,
    pub encounter_context: Option<EncounterContext>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#MedicalScribePostStreamActionSettings")]
#[derive(Debug, Clone)]
pub struct MedicalScribePostStreamActionSettings {
    pub output_s3_uri: String,
    pub clinical_note_generation_settings: ClinicalNoteGenerationSettings,
    pub medical_code_generation_settings: Option<MedicalCodeGenerationSettings>,
}

/// Whether the session generates medical codes from its clinical note after the stream
/// ends. The one member is `@required`, so it is a bare `bool`
/// (`smithy_query(query="shape", shape="MedicalCodeGenerationSettings")`).
#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#MedicalCodeGenerationSettings")]
#[derive(Debug, Clone, Default)]
pub struct MedicalCodeGenerationSettings {
    pub enabled: bool,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#ClinicalNoteGenerationSettings")]
#[derive(Debug, Clone)]
pub struct ClinicalNoteGenerationSettings {
    pub note_template_settings: NoteTemplateSettings,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.connecthealth#NoteTemplateSettings")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum NoteTemplateSettings {
    managedTemplate(ManagedTemplate),
    customTemplate(CustomTemplate),
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#ManagedTemplate")]
#[derive(Debug, Clone, Default)]
pub struct ManagedTemplate {
    pub template_type: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#CustomTemplate")]
#[derive(Debug, Clone, Default)]
pub struct CustomTemplate {
    pub template_type: String,
    pub template_instructions: Vec<TemplateSectionInstruction>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#TemplateSectionInstruction")]
#[derive(Debug, Clone, Default)]
pub struct TemplateSectionInstruction {
    pub section_header: String,
    pub section_instruction: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#EncounterContext")]
#[derive(Debug, Clone, Default)]
pub struct EncounterContext {
    pub unstructured_context: Option<String>,
    pub medical_coding_context: Option<MedicalCodingContext>,
}

/// The structured patient and encounter context a configuration event may carry for
/// medical code generation — every member optional
/// (`smithy_query(query="shape", shape="MedicalCodingContext")`). Whether a session was
/// given one is what `GetMedicalScribeListeningSession` reports
/// (`medicalCodingContextProvided`); the context itself is sensitive and is reported by no
/// reader.
#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#MedicalCodingContext")]
#[derive(Debug, Clone, Default)]
pub struct MedicalCodingContext {
    pub sex: Option<String>,
    pub status: Option<String>,
    pub date_of_birth: Option<String>,
    pub encounter_type: Option<String>,
    pub encounter_reason: Option<String>,
    pub after_hours: Option<bool>,
    pub encounter_date: Option<String>,
}

/// The audio event whose chunk travels as a base64 value inside the frame's JSON
/// payload. Its sibling `MedicalScribeBinaryAudioEvent` carries the chunk as the
/// frame's `@eventPayload` — raw bytes, so it needs no struct to be read into.
#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#MedicalScribeAudioEvent")]
#[derive(Debug, Clone, Default)]
pub struct MedicalScribeAudioEvent {
    pub audio_chunk: Vec<u8>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connecthealth#MedicalScribeSessionControlEvent")]
#[derive(Debug, Clone, Default)]
pub struct MedicalScribeSessionControlEvent {
    pub r#type: Option<String>,
}
