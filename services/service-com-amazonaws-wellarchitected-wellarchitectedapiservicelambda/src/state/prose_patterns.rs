// SPDX-License-Identifier: CC0-1.0

//! The modelled `@pattern` of a prose member, held to where the request boundary cannot
//! apply it.
//!
//! The boundary validator compiles each member's `@pattern` with the `regex` crate, which
//! has no look-around, and an expression it cannot compile is treated there as satisfied —
//! so a member whose pattern forbids a fragment with `(?!…)` reaches the handler unchecked.
//! The requirement still stands, so the handler that stores such a value holds it to the
//! expression, the way every handler of this crate restates an enum's value set.
//!
//! The CHECK is not written here: `service_common::unapplied_patterns` is the tree's home
//! for it, and it evaluates the MODEL'S OWN expression text with a look-around-capable
//! engine under its own budget, so nothing in this crate can drift from what the model
//! spells. This module only names the expression each member carries and the sentence a
//! refusal states.

/// `PutAgentRecommendationFeedbackRequest$comments`: "optional comments providing
/// additional context about the feedback" (`smithy_query(query="operation",
/// operation="PutAgentRecommendationFeedback")`), which admits every character outside the
/// Unicode `Other` category plus the two line breaks, and no `${name:` variable reference.
///
/// The expression AS THE MODEL WRITES IT, which is what the check evaluates.
pub const COMMENTS_PATTERN: &str = r"^(?:(?!\$\{[a-zA-Z]+:)[\P{C}\r\n])+$";

/// The constraint sentence a member-constraint refusal states for a value outside its
/// member's `@pattern`: the expression the model declares, with the value WITHHELD — the
/// requirement is the pattern, and the members that carry one of these patterns are
/// `@sensitive`.
pub fn pattern_constraint(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// Whether `comments` is a value [`COMMENTS_PATTERN`] admits.
pub fn comments_admitted(comments: &str) -> bool {
    service_common::unapplied_patterns::modelled_pattern_admits(COMMENTS_PATTERN, comments)
}
