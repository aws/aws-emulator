// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_speech_synthesis_stream` operation.
//!
//! Hand-authored rather than generated: the op-stub generator declines an operation
//! carrying a `@streaming` event-stream member (`src/ops/SKIPPED_STUBS.md` records
//! it), so neither its shapes nor its handler were emitted. Its DECLARATION therefore
//! lives in `src/ops/types_local/mod.rs` rather than in the `@generated`
//! `types/mod.rs`, which a model update rewrites.
//!
//! Each structure is declared exactly once across this service's type modules: every
//! shape this operation shares with another — its error shapes
//! (`ValidationException` and the two it declares but never constructs) and the
//! value set of `ValidationExceptionReason` — is RENDERED from the model for those
//! operations and reached from `crate::ops::types` here, so only the shapes the
//! model's own rendering does not carry are written out below.
//!
//! ## Why the input is split in two
//!
//! `StartSpeechSynthesisStreamInput` binds every member but one to an
//! `@httpHeader`; the remaining member, `ActionStream`, is the `@streaming`
//! event-stream `@httpPayload`, whose framing the generated codec does not speak.
//! Declaring the request shape with that member in `unsupported = [..]` gives a
//! struct the framework's own HTTP binding deserializer fills from the request
//! headers and `constraints::validate` checks against the model's enum, length and
//! pattern traits — so the header half of the request is validated by the MODEL —
//! while the payload half is decoded from real `vnd.amazon.eventstream` frames in
//! the operation. The output needs no such split: its only member is the
//! `@streaming` payload, so the stream IS the response body.
//!
//! ## The events declared here, and the one that is not
//!
//! `TextEvent` and `CloseStreamEvent` are the two members of the input union
//! `StartSpeechSynthesisStreamActionStream`, decoded from the frames a caller
//! sends; `StreamClosedEvent` is the output union member that ends the stream.
//! `AudioEvent` is deliberately NOT declared: its `AudioChunk` member is the
//! event's `@eventPayload`, so the frame payload IS the raw chunk rather than a
//! JSON object with a member in it, and the operation writes that frame from the
//! chunk bytes directly (see the framing note on the operation).

#![allow(unused_imports)]

use crate::ops::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-polly-parrot-v1.json");

/// The header-bound half of `StartSpeechSynthesisStreamInput`; `ActionStream` is the
/// event-stream payload and is decoded frame by frame instead.
#[smithy_macros::smithy(
    shape = "com.amazonaws.polly#StartSpeechSynthesisStreamInput",
    unsupported = ["ActionStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartSpeechSynthesisStreamSettings {
    pub engine: String,
    pub language_code: Option<String>,
    pub lexicon_names: Option<Vec<String>>,
    pub output_format: String,
    pub sample_rate: Option<String>,
    pub voice_id: String,
}

/// One `TextEvent` of the input stream: text to synthesize, how to read it, and
/// whether the audio buffered so far has to be written out at once.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#TextEvent")]
#[derive(Debug, Clone, Default)]
pub struct TextEvent {
    pub text: String,
    pub text_type: Option<String>,
    pub flush_stream_configuration: Option<FlushStreamConfiguration>,
}

/// "Configuration for controlling when synthesized audio flushes to the output
/// stream."
#[smithy_macros::smithy(shape = "com.amazonaws.polly#FlushStreamConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct FlushStreamConfiguration {
    pub force: bool,
}

/// The input event that ends the input stream, after which "all audio will be
/// returned". It carries no member.
#[smithy_macros::smithy(shape = "com.amazonaws.polly#CloseStreamEvent")]
#[derive(Debug, Clone, Default)]
pub struct CloseStreamEvent {}

/// The output event that closes the stream, carrying "the total number of
/// characters synthesized during the streaming session".
#[smithy_macros::smithy(shape = "com.amazonaws.polly#StreamClosedEvent")]
#[derive(Debug, Clone, Default)]
pub struct StreamClosedEvent {
    pub request_characters: i32,
}

