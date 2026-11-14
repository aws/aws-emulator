// SPDX-License-Identifier: CC0-1.0

//! How WAF's request validator STATES a member-constraint failure: the order it lists a
//! member's clauses in, and the form it reports a `@pattern` in.
//!
//! The clauses themselves are the framework's — [`Violation::message`] writes each one
//! and `::aws_smithy_schema_server::envelope` wraps them in the
//! `<n> validation errors detected: …` envelope — so nothing about constraint CHECKING
//! or clause WORDING is re-implemented here. What this module owns is the two choices
//! the framework leaves open and WAF makes differently:
//!
//! * a member that breaks both its `@pattern` and a `@length` bound has its PATTERN
//!   clause stated first, and
//! * a `@pattern` is reported as the FULL-MATCH expression equivalent to the Smithy
//!   partial match, so an unanchored pattern is reported wrapped in `.*`
//!   ([`full_match_pattern`]).
//!
//! Both are reached from the one reporter this service words a constraint failure
//! through ([`constraint_envelope`]) — the boundary walk's refusal goes through it from
//! `crate::service`'s refusal hook, and a handler's self-applied check goes through it
//! directly, so a member refused at either point reads identically.

use super::*;

use std::collections::HashMap;

/// The way WAF reports a Smithy `@pattern`: the full-match expression equivalent to
/// Smithy's partial match.
///
/// A Smithy `@pattern` matches ANYWHERE in the value ("the pattern is not anchored"), so
/// the expression a caller would have to satisfy as a whole match is the pattern with
/// `.*` on either side — and that is the expression WAF's validator advertises, rather
/// than the model's bare text. A side that already carries `.*`, or that is anchored
/// there (`^` / `$`), is left as it is: `.*` before `^` would state an expression no
/// value can satisfy.
pub fn full_match_pattern(pattern: &str) -> String {
    let mut stated = String::with_capacity(pattern.len() + 4);
    if !(pattern.starts_with(".*") || pattern.starts_with('^')) {
        stated.push_str(".*");
    }
    stated.push_str(pattern);
    if !(pattern.ends_with(".*") || pattern.ends_with('$')) {
        stated.push_str(".*");
    }
    stated
}

/// The envelope this service answers for `violations`, in WAF's clause order and with
/// every `@pattern` stated as [`full_match_pattern`] does.
///
/// The envelope and each clause are the framework's own, so a member refused by a
/// handler reads exactly like one refused at the request boundary.
pub fn constraint_envelope(violations: &[Violation]) -> String {
    ::aws_smithy_schema_server::envelope(&stated_clauses(violations))
        .unwrap_or_else(|| "invalid request".to_string())
}

/// `violations` as WAF states them: each one's `@pattern` rendered full-match, and the
/// clauses about one member ordered pattern-first.
///
/// The members keep the order the walk found them in — a request is reported member by
/// member, so each member's clauses are grouped at the position its FIRST clause was
/// found at — and only the clauses WITHIN one member are reordered.
fn stated_clauses(violations: &[Violation]) -> Vec<Violation> {
    // The position each member's first clause was found at, so the sort below is one
    // lookup per clause rather than a scan.
    let mut member_order: HashMap<&str, usize> = HashMap::new();
    for (position, violation) in violations.iter().enumerate() {
        member_order.entry(violation.path.as_str()).or_insert(position);
    }
    let mut stated: Vec<(usize, Violation)> = violations
        .iter()
        .map(|violation| {
            (
                member_order
                    .get(violation.path.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
                as_stated(violation),
            )
        })
        .collect();
    stated.sort_by_key(|(member, violation)| (*member, clause_rank(&violation.kind)));
    stated.into_iter().map(|(_, violation)| violation).collect()
}

/// One violation as WAF states it: a `@pattern` failure reports the full-match
/// expression, and every other kind is the framework's unchanged.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            kind: ViolationKind::Pattern(full_match_pattern(pattern)),
            ..violation.clone()
        },
        _ => violation.clone(),
    }
}

/// Where a clause about one member falls in WAF's order: its `@pattern` first, then the
/// bounds, in the order the walk found them.
fn clause_rank(kind: &ViolationKind) -> u8 {
    match kind {
        ViolationKind::Pattern(_) => 0,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unanchored pattern is wrapped on both sides; a side that is anchored, or that
    /// already carries `.*`, is left alone. The rows are this service's own patterns
    /// (`smithy_query(query="shape",shape="NextMarker,EntityName,LockToken")`) plus the
    /// two degenerate forms.
    #[test]
    fn a_pattern_is_stated_as_its_full_match_expression() {
        let rows: &[(&str, &str)] = &[
            (r"\S", r".*\S.*"),
            (r"^[\w\-]+$", r"^[\w\-]+$"),
            (r"^[0-9a-f]{8}$", r"^[0-9a-f]{8}$"),
            ("[^0-9]", ".*[^0-9].*"),
            (".*", ".*"),
            ("arn:[^:]*:.*", ".*arn:[^:]*:.*"),
            ("(^a$|b+)", ".*(^a$|b+).*"),
        ];
        for (pattern, expected) in rows {
            let stated = full_match_pattern(pattern);
            assert_eq!(
                &stated, expected,
                "case {pattern:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// A member that breaks its pattern AND a length bound is reported pattern-first,
    /// however the walk found them.
    #[test]
    fn a_members_pattern_clause_is_stated_before_its_bounds() {
        let length = Violation {
            path: "nextMarker".to_string(),
            kind: ViolationKind::MinLength(1),
            value: Some(String::new()),
            sensitive: false,
        };
        let pattern = Violation {
            path: "nextMarker".to_string(),
            kind: ViolationKind::Pattern(r"\S".to_string()),
            value: Some(String::new()),
            sensitive: false,
        };
        let envelope = constraint_envelope(&[length, pattern]);
        assert_eq!(
            envelope,
            "2 validation errors detected: Value '' at 'nextMarker' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: .*\\S.*; \
             Value '' at 'nextMarker' failed to satisfy constraint: Member must have \
             length greater than or equal to 1",
            "the pattern clause comes first and states the full-match expression"
        );
    }
}
