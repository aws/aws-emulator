// SPDX-License-Identifier: CC0-1.0

//! The context window of the base model a training session adapts, and the failure an
//! input that does not fit it settles in.
//!
//! A sampling operation is served by the model the session was created around — the
//! control plane records it as `TrainingSessionModelConfig$BaseModelArn` and publishes it
//! into the directory both planes read — and a model can only attend over so many tokens
//! at once. An operation whose prompt, plus whatever it asks the model to generate, needs
//! more than that window is not something the model can process at all: it is accepted at
//! submit and settles in the modelled failure status with the reason below, carrying no
//! result.
//!
//! The window is a property of the BASE MODEL, not of this emulator, so it is looked up
//! per base model family and a model of no family published here is given none: nothing is
//! then compared, rather than a limit being guessed for a model whose own is unknown.

/// The context window, in tokens, of each base model FAMILY this emulator publishes one
/// for, keyed by the family name a `BaseModelArn` carries.
///
/// A training session fine-tunes a model of the SageMaker public hub, whose ARN names the
/// model it adapts in its resource path (`.../Model/openai-reasoning-<family>/<version>`),
/// so the key is matched against the ARN rather than read off a field of its own. The
/// window is the number of tokens the training-session runtime serves that family's
/// sampling operations over; the `gpt-oss` family is served over 65,536, every size of it
/// alike. The entries are ordered longest key first, so the most specific family a model's
/// ARN names is the one that decides its window.
const BASE_MODEL_CONTEXT_WINDOWS: &[(&str, i32)] = &[("gpt-oss", 65_536)];

/// How a sampling operation whose input does not fit the base model's context window
/// reports it: the fixed sentence the service states for that condition, which quotes no
/// value of the request.
#[must_use]
pub fn context_length_exceeded() -> String {
    "The input exceeds the model's maximum context length. Please reduce the length of \
     your prompt or the requested max tokens."
        .to_string()
}

/// The context window of the base model `base_model_arn` names, and `None` for a base
/// model of no family [`BASE_MODEL_CONTEXT_WINDOWS`] publishes a window for.
///
/// Matched case-insensitively: a model name travels through an ARN as written, and the
/// family it names is the same family whatever case the writer used.
#[must_use]
pub fn base_model_context_window(base_model_arn: &str) -> Option<i32> {
    let arn = base_model_arn.to_ascii_lowercase();
    BASE_MODEL_CONTEXT_WINDOWS
        .iter()
        .find(|(family, _)| arn.contains(family))
        .map(|(_, window)| *window)
}
