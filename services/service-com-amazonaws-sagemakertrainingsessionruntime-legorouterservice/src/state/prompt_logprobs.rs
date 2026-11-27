// SPDX-License-Identifier: CC0-1.0

//! The per-token log-probabilities a completed `computeLogprobs` operation serves.
//!
//! `ComputeLogprobsConfig` asks for the scores of "the input prompt whose token
//! log-probabilities will be computed", and `ComputeLogprobsResult$logprobs` — "the
//! per-token log-probabilities for the input prompt", `@required` — is what carries them:
//! one score per prompt token, each the log-probability of that token GIVEN the tokens
//! before it.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, so there is no learned conditional
//! distribution to read a score off, and nothing here invents one. What is reported is the
//! log-probability of a token under the MAXIMUM-ENTROPY conditional distribution over the
//! vocabulary this service speaks — the distribution of a model that has learned nothing,
//! and the only one that can be stated without one. It is `ln(1 / V)` over the
//! [`VOCABULARY_TOKENS`] that vocabulary holds: a real number computed from the real
//! vocabulary, never a value drawn from a digest, a counter, a word list or the prompt's
//! own ids, so no score here is dressed up as inference a forward pass performed.
//!
//! The FIRST token of a prompt has no preceding token to be conditioned on, so no
//! conditional probability is defined for it and none is reported: `DoubleList` is a list
//! of plain `Double` and carries no absence of its own, so the missing score travels as
//! the non-finite double that JSON renders as `null` ([`NO_CONTEXT_LOGPROB`]).
//!
//! Everything about the LIST is real and computed from the request: there is exactly one
//! entry per prompt token counted across every chunk, in prompt order, so the same token
//! sequence split into different chunks is scored identically — how a prompt is chunked is
//! framing, not content ("the ordered list of encoded text chunks forming the input
//! sequence", `ModelInput$chunks`).

use crate::ops::types::ModelInput;

/// How many tokens the vocabulary this service speaks holds.
///
/// Every token id this service serves is drawn from the `cl100k_base` encoding the
/// tokenizer crate ships (see [`super::sampled_tokens`]), whose published vocabulary is
/// 100,256 byte-pair tokens — the ids `0..=100_255`, before the special markers an
/// ordinary encoding never emits. It is the support of the distribution a score below is
/// taken under, so it is stated once here rather than at each use.
pub const VOCABULARY_TOKENS: u32 = 100_256;

/// The score a prompt's FIRST token carries: a non-finite double, which is how a list of
/// plain `Double` says "no value here" and what a JSON body renders as `null`.
///
/// A log-probability is conditional on the tokens before it, and the first token of a
/// prompt has none, so there is no number to report for it — not a zero, which would claim
/// certainty, and not a score invented to fill the slot.
pub const NO_CONTEXT_LOGPROB: f64 = f64::NAN;

/// The log-probability of one token under the maximum-entropy (uniform) distribution over
/// the vocabulary: `ln(1 / V)`, a finite negative double.
///
/// Computed rather than written down, from [`VOCABULARY_TOKENS`].
#[must_use]
pub fn uniform_vocabulary_logprob() -> f64 {
    -f64::from(VOCABULARY_TOKENS).ln()
}

/// How many tokens `prompt` carries, as a count of scores to report: the token ids of
/// every chunk, in order.
#[must_use]
pub fn prompt_logprob_count(prompt: &ModelInput) -> usize {
    prompt.chunks.iter().map(|chunk| chunk.tokens.len()).sum()
}

/// The per-token log-probabilities of `prompt`: one entry per token across all of its
/// chunks, the first without a score ([`NO_CONTEXT_LOGPROB`]) and every later one the
/// score this emulator can state ([`uniform_vocabulary_logprob`]).
///
/// The list is as long as the prompt that was accepted — a count of the chunks' own token
/// lists, never a number the request named — so the answer is bounded by the request.
#[must_use]
pub fn prompt_logprobs(prompt: &ModelInput) -> Vec<f64> {
    let scored = uniform_vocabulary_logprob();
    (0..prompt_logprob_count(prompt))
        .map(|index| {
            if index == 0 {
                NO_CONTEXT_LOGPROB
            } else {
                scored
            }
        })
        .collect()
}
