// SPDX-License-Identifier: CC0-1.0

//! The declaration of this service's HAND-AUTHORED type modules.
//!
//! WHY THIS MODULE EXISTS. Two of this service's type modules are written by hand:
//!
//! * `types/start_speech_synthesis_stream_types.rs` — the op-stub generator declines an
//!   operation carrying a `@streaming` event-stream member, so neither
//!   `StartSpeechSynthesisStream`'s shapes nor its handler were ever emitted
//!   (`src/ops/SKIPPED_STUBS.md` records it); and
//! * `types/started_task_wire.rs` — the envelope `StartSpeechSynthesisTask` answers
//!   with, which carries one member beyond the ones the model declares and so cannot be
//!   expressed through the derive at all.
//!
//! What cannot live in `types/mod.rs` is their DECLARATION. That file carries
//! `@generated`: a model update rewrites it from the model and lists exactly the modules
//! the generator rendered, so a refresh drops every hand-authored module from it and the
//! crate stops building — which is what the update this module was added for did. The
//! `mod` lines live here instead, in a module the generator does not write, declared
//! from `src/service.rs`: the one file this crate's scaffold emits once and never
//! regenerates.
//!
//! Each module keeps its file where it is (a file at a generated path without the
//! `@generated` marker is hand-written, and an update neither overwrites nor deletes
//! one), so the `#[path]`s below point back into `types/`. Every shape is declared
//! exactly once across these modules and the rendered ones — a shape the model now
//! declares for another operation is reached from `crate::ops::types` rather than
//! re-declared here — and both modules are re-exported, so one
//! `use crate::service::types_local::*;` in a handler resolves the shapes of its
//! operation that the model's own rendering does not carry.

/// The header-bound half of `StartSpeechSynthesisStream`'s request and the events of
/// its two streams.
#[path = "../types/start_speech_synthesis_stream_types.rs"]
pub mod start_speech_synthesis_stream_types;

/// The task envelope `StartSpeechSynthesisTask` answers with, and the one member of it
/// the on-disk model does not declare.
#[path = "../types/started_task_wire.rs"]
pub mod started_task_wire;

#[allow(unused_imports)]
pub use start_speech_synthesis_stream_types::*;
#[allow(unused_imports)]
pub use started_task_wire::*;
