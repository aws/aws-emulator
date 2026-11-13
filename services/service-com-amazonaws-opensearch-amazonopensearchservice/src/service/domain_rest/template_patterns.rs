// SPDX-License-Identifier: CC0-1.0

//! The PATTERNS an index template applies to: the names a write may carry, and whether two
//! templates of the same priority can match one index name.
//!
//! "index_patterns: array of wildcard expressions matching the names of indexes" — the list
//! is the CALLER's, and so is the composable form's `composed_of`, so this module is where
//! both are read under bounds and where the overlap between two templates is decided.
//!
//! # What bounds the reading
//!
//! Deciding an overlap compares every pattern the write carries against every pattern each
//! stored template of the same priority carries, so the comparisons are the PRODUCT of two
//! caller-chosen counts. Three bounds, each this emulator's own, make that product a product
//! of constants:
//!
//! * [`MAX_TEMPLATE_PATTERNS`] — how many names one list may carry, the same bound a request
//!   naming index patterns is held to ([`super::MAX_NAMED_INDEX_PATTERNS`]);
//! * [`MAX_PATTERN_BYTES`] — how long one of them may be, which is the engine's own ceiling
//!   on an index name and so on a pattern over index names;
//! * [`MAX_OVERLAP_WORK`] — how much COMPARING may cost over one write. A literal pattern
//!   (one carrying no wildcard) is compared by equality and costs nothing against it; only a
//!   pattern carrying `*` or `?` is matched, and such a comparison is charged the product of
//!   the two lengths, which is what the matcher costs (it is single-pass with one backtrack
//!   point per `*`, so its cost is bounded by that product). A write whose overlap cannot be
//!   decided inside that budget is refused rather than compared further.

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::Value;

use crate::state::{ingest, Domain, IndexTemplate};

use super::error;

/// How many patterns one template's list may carry.
pub(super) const MAX_TEMPLATE_PATTERNS: usize = super::MAX_NAMED_INDEX_PATTERNS;

/// How long one pattern may be, in bytes: the engine's own ceiling on an index name, which
/// is what a pattern over index names matches.
pub(super) const MAX_PATTERN_BYTES: usize = 255;

/// How much work deciding one write's overlaps may cost; see this module's docs.
const MAX_OVERLAP_WORK: usize = 16_000_000;

/// The names the array member `member` of `definition` carries, held to the bounds above.
pub(super) fn bounded_names(
    definition: &Value,
    member: &str,
) -> Result<Vec<String>, Box<RtResponse>> {
    let named: Vec<String> = definition
        .get(member)
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if named.len() > MAX_TEMPLATE_PATTERNS {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "[{member}] may name at most {MAX_TEMPLATE_PATTERNS} entries, and this \
                 request names {}",
                named.len()
            ),
        )));
    }
    if let Some(oversized) = named.iter().find(|name| name.len() > MAX_PATTERN_BYTES) {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "an entry of [{member}] may be at most {MAX_PATTERN_BYTES} bytes, and this \
                 request names one of {} bytes",
                oversized.len()
            ),
        )));
    }
    Ok(named)
}

/// The refusal a composable template earns when another template of the SAME priority
/// already matches an index name it would match, or `None` when none does.
///
/// Two such templates leave index creation with no template to prefer — the priority is
/// what decides between matching templates — so the engine refuses the second one, naming
/// the templates it overlaps and the patterns each carries.
pub(super) fn equal_priority_overlap(
    domain: &Domain,
    name: &str,
    index_patterns: &[String],
    priority: i64,
) -> Option<RtResponse> {
    // What comparing may still cost over this write; see [`MAX_OVERLAP_WORK`].
    let mut budget = MAX_OVERLAP_WORK;
    let mut overlaps: Vec<&IndexTemplate> = Vec::new();
    for held in domain
        .index_templates
        .values()
        .filter(|held| held.name != name && held.priority == priority)
    {
        for theirs in &held.index_patterns {
            let mut matched = false;
            for ours in index_patterns {
                match overlapping(ours, theirs, &mut budget) {
                    Some(true) => {
                        matched = true;
                        break;
                    }
                    Some(false) => {}
                    // The budget is spent, so this write's overlaps cannot be decided: it is
                    // refused rather than compared further or admitted undecided.
                    None => return Some(undecidable(name)),
                }
            }
            if matched {
                overlaps.push(held);
                break;
            }
        }
    }
    if overlaps.is_empty() {
        return None;
    }
    let names: Vec<&str> = overlaps.iter().map(|held| held.name.as_str()).collect();
    let patterns: Vec<String> = overlaps
        .iter()
        .map(|held| format!("{} => [{}]", held.name, held.index_patterns.join(", ")))
        .collect();
    Some(error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!(
            "index template [{name}] has index patterns [{}] matching patterns from existing \
             templates [{}] with patterns ({}) that have the same priority [{priority}], \
             multiple index templates may not match during index creation, please use a \
             different priority",
            index_patterns.join(", "),
            names.join(","),
            patterns.join(",")
        ),
    ))
}

/// The refusal for a write whose overlaps this cluster would not finish comparing.
fn undecidable(name: &str) -> RtResponse {
    error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!(
            "index template [{name}] names patterns this cluster cannot decide the overlap of \
             within the {MAX_OVERLAP_WORK} comparisons it spends on one write, please name \
             fewer or less general patterns"
        ),
    )
}

/// Whether two index patterns can match ONE index name: the same pattern, or one whose
/// wildcards cover the other — and `None` when `budget` had nothing left for the comparison.
///
/// A pattern carrying no wildcard matches one index name, so two such patterns overlap only
/// where they are the SAME name: that case is decided by equality and costs the budget
/// nothing. Where one carries a wildcard the question is a match, read both ways round since
/// either may be the one carrying them, and through the same wildcard matching every other
/// pattern of this crate is read by — so an overlap is decided by the matching rule itself
/// rather than by a second reading of `*` and `?`. Each such match is charged the product of
/// the two lengths.
fn overlapping(left: &str, right: &str, budget: &mut usize) -> Option<bool> {
    let wildcarded = |pattern: &str| pattern.contains(['*', '?']);
    if !wildcarded(left) && !wildcarded(right) {
        return Some(left == right);
    }
    let charged = left.len().saturating_mul(right.len()).max(1);
    *budget = budget.checked_sub(charged)?;
    Some(
        ::service_common::iam_authorization::wildcard_match(left, right, true)
            || ::service_common::iam_authorization::wildcard_match(right, left, true),
    )
}
