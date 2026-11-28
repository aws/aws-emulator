// SPDX-License-Identifier: CC0-1.0

//! The members this service refuses with the CONSTRAINT REPORT rather than with a sentence
//! of its own, and the checks their handlers need in order to state it.
//!
//! Most members of this service answer a broken `@length`, `@range` or `@pattern` with a
//! sentence AppSync words itself (`crate::state::free_text`,
//! `crate::service::member_refusal`). Some do not: their refusal is the generic report the
//! validation layer in front of the service builds — `N validation error(s) detected:
//! Value at '<member path>' failed to satisfy constraint: <requirement>` — naming the
//! member and the requirement it broke and quoting no submitted value.
//!
//! WHICH of the two a member takes is a property of the member AND of the operation
//! carrying it, not of the constraint class: `code` is `Code`, `@length` 1..32768, on both
//! the operations that STORE a definition and on the one that EVALUATES one, and only the
//! second states the report (the first names the ceiling in the service's own words,
//! `crate::service::code_length_refusal`). The same holds for `maxResults`, which every
//! list operation carries off the one `MaxResults` shape: the `/v2/apis…` surface states
//! the report where the v1 control plane states its own page-size sentence.
//!
//! That is why these checks run HERE and in the handler rather than at the request
//! boundary (`skip_validation` in `src/ops/types/`, the same split
//! [`crate::state::api_name_refusal`] is there for): the seam that words a boundary
//! refusal (`smithy_server_runtime::Service::override_refusal`) is handed the broken
//! clauses alone, with no operation to key on, so the operation's own handler is the one
//! place the choice can be made for the operation that makes it.
//!
//! The REPORT itself is nobody's invention here: it is the tree's one statement of the
//! framework's own (`service_common::validation_message`), with the value half dropped
//! from every clause — which is what the recorded refusals of these members carry, and
//! what keeps a caller's oversized submission out of the message it is refused with.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::smithy_server_runtime::Refusal;

/// `Code`'s modelled minimum length: a definition is at least one character long
/// (`smithy_query(query="shape", shape="Code")`).
pub const CODE_MIN_LEN: usize = 1;

/// `Code`'s modelled maximum length.
pub const CODE_MAX_LEN: usize = 32_768;

/// The member a request carries the code to evaluate on: `EvaluateCodeRequest$code`, "the
/// code definition to be evaluated", spelled as the framework's validating walk spells a
/// member of an operation's input, because that is the member path the clause reports.
pub const CODE_MEMBER: &str = "code";

/// `Context`'s modelled minimum length: the shortest value is two characters, the shortest
/// text that can be a map at all (`smithy_query(query="shape", shape="Context")`).
pub const CONTEXT_MIN_LEN: usize = 2;

/// `Context`'s modelled maximum length.
pub const CONTEXT_MAX_LEN: usize = 28_000;

/// The member a request carries the contextual information for the evaluation on:
/// `EvaluateCodeRequest$context`.
pub const CONTEXT_MEMBER: &str = "context";

/// `MaxResults`'s modelled bounds (`@range` 0..25,
/// `smithy_query(query="shape", shape="MaxResults")`): a page size of `0` is the member's
/// own default, i.e. "unspecified", and is a value the range admits.
pub const MAX_RESULTS_MIN: i64 = 0;

/// `MaxResults`'s modelled maximum ([`MAX_RESULTS_MIN`]).
pub const MAX_RESULTS_MAX: i64 = 25;

/// The member a list operation carries its page size on.
pub const MAX_RESULTS_MEMBER: &str = "maxResults";

/// The constraint report for `violations`, or `None` when there are none.
///
/// One rendering for every member in this module: the clauses and the envelope are the
/// framework's, stated through the tree's one copy of that rendering, with the value half
/// dropped from each clause.
fn constraint_report(violations: Vec<Violation>) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    ::service_common::validation_message::aws_validation_refusal_withholding_violations(
        &Refusal::Invalid(violations),
        |_| true,
    )
    .map(|refused| refused.message)
}

/// The `@length` bound `value` crosses, as the framework's own walk reports it: the one
/// bound that was crossed, with no value carried (the report states none for these
/// members).
///
/// The length is counted in CHARACTERS, which is what Smithy's `@length` bounds on a
/// string, so a multi-byte value is measured the way the model measures it.
fn length_violation(member: &str, value: &str, min: usize, max: usize) -> Vec<Violation> {
    let length = value.chars().count();
    let kind = if length < min {
        ViolationKind::MinLength(min as i64)
    } else if length > max {
        ViolationKind::MaxLength(max as i64)
    } else {
        return Vec::new();
    };
    vec![Violation {
        path: member.to_string(),
        kind,
        // No value: the report this is rendered through states none for these members, so
        // there is nothing to quote and nothing of a caller's bytes is copied to be
        // dropped again. The CHECK above was made against the whole submitted value.
        value: None,
        sensitive: false,
    }]
}

/// The constraint report for a `code` outside `Code`'s modelled `@length`, or `None` for one
/// inside it.
///
/// The refusal is carried by the evaluating operation's own modelled `BadRequestException`
/// (400) — the one client error it declares — with this message, which is the answer its
/// recordings show for this member.
pub fn evaluated_code_refusal(code: &str) -> Option<String> {
    constraint_report(length_violation(
        CODE_MEMBER,
        code,
        CODE_MIN_LEN,
        CODE_MAX_LEN,
    ))
}

/// The constraint report for a `context` outside `Context`'s modelled `@length`, or `None`
/// for one inside it.
///
/// Only the `@length`: `Context`'s `@pattern` is `^[\s\S]*$`, which every value it can hold
/// satisfies (`crate::state::free_text` states that reading), so there is no form left to
/// refuse a context for beyond its size.
pub fn evaluated_context_refusal(context: &str) -> Option<String> {
    constraint_report(length_violation(
        CONTEXT_MEMBER,
        context,
        CONTEXT_MIN_LEN,
        CONTEXT_MAX_LEN,
    ))
}

/// The constraint report for a `maxResults` outside `MaxResults`'s modelled `@range`, or
/// `None` for one inside it — each published bound included.
///
/// Stated by the `/v2/apis…` surface, whose recordings show the framework-level constraint
/// error for this member (400 `ValidationException`,
/// `crate::scaffolded::Scaffolded::validation`) where the v1 control plane states its own
/// page-size sentence (`crate::service::RANGED_MEMBERS`).
pub fn page_size_refusal(max_results: i64) -> Option<String> {
    let kind = if max_results < MAX_RESULTS_MIN {
        ViolationKind::MinValue(MAX_RESULTS_MIN.to_string())
    } else if max_results > MAX_RESULTS_MAX {
        ViolationKind::MaxValue(MAX_RESULTS_MAX.to_string())
    } else {
        return None;
    };
    constraint_report(vec![Violation {
        path: MAX_RESULTS_MEMBER.to_string(),
        kind,
        value: None,
        sensitive: false,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report a refusal of `member` states for `requirement`.
    fn expected(member: &str, requirement: &str) -> String {
        format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             {requirement}"
        )
    }

    /// RULE: a value inside a member's modelled bounds — each published bound included — is
    /// not refused.
    #[test]
    fn a_value_inside_the_modelled_bounds_is_not_refused() {
        for (case, refusal) in [
            (
                "the shortest code the bound admits",
                evaluated_code_refusal(&"c".repeat(CODE_MIN_LEN)),
            ),
            (
                "the longest code the bound admits",
                evaluated_code_refusal(&"c".repeat(CODE_MAX_LEN)),
            ),
            (
                "the shortest context the bound admits",
                evaluated_context_refusal(&"{}".repeat(CONTEXT_MIN_LEN / 2)),
            ),
            (
                "the longest context the bound admits",
                evaluated_context_refusal(&"c".repeat(CONTEXT_MAX_LEN)),
            ),
            (
                "a page size of the modelled minimum",
                page_size_refusal(MAX_RESULTS_MIN),
            ),
            (
                "a page size of the modelled maximum",
                page_size_refusal(MAX_RESULTS_MAX),
            ),
        ] {
            assert!(
                refusal.is_none(),
                "case {case:?}: expected no refusal, got {refusal:?}"
            );
        }
    }

    /// RULE: a value outside those bounds states the report, naming the member and the
    /// bound it crossed and quoting nothing of the submitted value.
    #[test]
    fn a_value_outside_the_modelled_bounds_states_the_report() {
        let over_long_code = "c".repeat(CODE_MAX_LEN + 1);
        let over_long_context = "c".repeat(CONTEXT_MAX_LEN + 1);
        let cases: Vec<(&str, Option<String>, String)> = vec![
            (
                "a code one character past the maximum",
                evaluated_code_refusal(&over_long_code),
                expected(
                    CODE_MEMBER,
                    &format!("Member must have length less than or equal to {CODE_MAX_LEN}"),
                ),
            ),
            (
                "an empty code",
                evaluated_code_refusal(""),
                expected(
                    CODE_MEMBER,
                    &format!("Member must have length greater than or equal to {CODE_MIN_LEN}"),
                ),
            ),
            (
                "a context one character short of the minimum",
                evaluated_context_refusal("{"),
                expected(
                    CONTEXT_MEMBER,
                    &format!("Member must have length greater than or equal to {CONTEXT_MIN_LEN}"),
                ),
            ),
            (
                "a context one character past the maximum",
                evaluated_context_refusal(&over_long_context),
                expected(
                    CONTEXT_MEMBER,
                    &format!("Member must have length less than or equal to {CONTEXT_MAX_LEN}"),
                ),
            ),
            (
                "a page size one past the maximum",
                page_size_refusal(MAX_RESULTS_MAX + 1),
                expected(
                    MAX_RESULTS_MEMBER,
                    &format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
                ),
            ),
            (
                "a page size one below the minimum",
                page_size_refusal(MAX_RESULTS_MIN - 1),
                expected(
                    MAX_RESULTS_MEMBER,
                    &format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
                ),
            ),
        ];
        for (case, refusal, expected) in cases {
            assert_eq!(
                refusal.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }

    /// RULE: no clause of these members quotes the submitted value, however short it is.
    #[test]
    fn no_clause_quotes_the_submitted_value() {
        for (case, value, refusal) in [
            ("an empty code", "", evaluated_code_refusal("")),
            ("a one-character context", "{", evaluated_context_refusal("{")),
        ] {
            let message = refusal.unwrap_or_default();
            assert!(
                !message.contains(&format!("Value '{value}'")) && !message.contains("Value null"),
                "case {case:?}: expected no value quoted back, got {message:?}"
            );
        }
    }
}
