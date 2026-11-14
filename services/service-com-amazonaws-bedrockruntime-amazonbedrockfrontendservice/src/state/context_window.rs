// SPDX-License-Identifier: CC0-1.0

//! The model's context window, and the refusal an input past it earns.
//!
//! # The rule this module implements
//!
//! A foundation model reads a BOUNDED prompt: its context window is how many
//! input tokens one request may send it, published per model ("Supported
//! foundation models in Amazon Bedrock" in the Amazon Bedrock User Guide) rather
//! than stated in the Smithy model, which bounds only the bytes of
//! `InvokeModel$body`. An input whose counted tokens are past that window is
//! therefore REFUSED — `ValidationException`, HTTP 400 — on every inference
//! surface, with the model-side sentence
//! [`INPUT_TOKENS_EXCEEDED_MESSAGE`]; the Converse API, which reports a
//! model-side error of a turn it was about to serve, carries that sentence
//! behind [`MODEL_ERRORS_PREFIX`].
//!
//! Refusing it is the whole point of the bound: a request served against a
//! shortened prompt looks exactly like one served whole, so a caller could not
//! tell the model read its prompt from the service having dropped most of it.
//! Nothing in this emulator shortens a prompt to fit — the only thing a request
//! past the window gets is the refusal.
//!
//! The window each model publishes is in [`super::model_catalog`], on
//! [`super::ModelEntry::context_window`]; the tokens an input is are counted
//! with the single tokenizer every surface of this service counts with
//! ([`super::count_text_tokens`]), so the count a refusal is decided on is the
//! count `CountTokens` reports for the same input.

use super::ModelEntry;

/// The sentence the inference surfaces refuse an input past the invoked model's
/// context window with. Entirely fixed text for this refusal class: it names no
/// count and no limit, so nothing of the request leaks into it.
pub const INPUT_TOKENS_EXCEEDED_MESSAGE: &str = "Input Tokens Exceeded: Number of input tokens \
                                                 exceeds maximum length. Please update the input \
                                                 to try again.";

/// What the Converse API puts in front of an error the MODEL reported for a turn
/// it was asked to serve, as opposed to one the service itself found in the
/// request.
pub const MODEL_ERRORS_PREFIX: &str = "The model returned the following errors: ";

/// A model-side error as the Converse API states it.
pub fn model_errors_message(detail: &str) -> String {
    format!("{MODEL_ERRORS_PREFIX}{detail}")
}

/// Whether an input of `input_tokens` tokens is past what `model` reads in one
/// request.
///
/// A model whose window is unknown to this inventory (zero — a model that takes
/// no text prompt at all) bounds nothing here: a bound that was never published
/// cannot refuse a request.
pub fn exceeds_context_window(model: &ModelEntry, input_tokens: u32) -> bool {
    model.context_window > 0 && input_tokens > model.context_window
}
