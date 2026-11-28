// SPDX-License-Identifier: CC0-1.0

//! The size a routing rule's CONDITIONS may reach: how many values one condition matches on,
//! and how long one of those values may be.
//!
//! `CreateRoutingRule$Conditions` caps how many conditions of each kind a rule carries
//! ("conditions can contain up to two matchHeaders conditions and one matchBasePaths
//! conditions", [`super::MAX_MATCH_HEADERS_CONDITIONS`] and
//! [`super::MAX_MATCH_BASE_PATHS_CONDITIONS`]) but says nothing about the size of the lists
//! inside them, and neither does the model: `RoutingRuleMatchBasePaths$AnyOf` and
//! `RoutingRuleMatchHeaders$AnyOf` target lists with no length trait, and the values they
//! hold target strings with none either (`smithy_query(query="shape",
//! shape="RoutingRuleMatchBasePaths,RoutingRuleMatchHeaders,SelectionKey,
//! SelectionExpression")`).
//!
//! The bounds here are therefore EMULATOR growth bounds, the same kind as
//! [`super::MAX_ROUTING_RULES_PER_DOMAIN`]: a stored rule is read on EVERY request the
//! domain name's front door serves (`crate::service::domain_name_socket`), where each value
//! is compared against the request's path and each header glob is translated into a matcher,
//! so what one write stores decides what every later request costs. Capping the list and the
//! value here is what makes that cost a constant rather than something a single write chooses.
//!
//! `super` re-exports every public item; `validate_routing_rule` applies them, so both
//! writers of the resource (`CreateRoutingRule` and `PutRoutingRule`) are held to them.

use super::wire;

/// The most values one condition's `AnyOf` may name, after which the write is refused.
pub const MAX_CONDITION_VALUES: usize = 25;

/// The longest one of those values may be, in bytes — a base path to match on, or a header
/// value glob.
pub const MAX_CONDITION_VALUE_BYTES: usize = 1024;

/// Refuse a condition whose `AnyOf` names more values than [`MAX_CONDITION_VALUES`], or one
/// value longer than [`MAX_CONDITION_VALUE_BYTES`].
///
/// `Err` carries the message for the operation's `BadRequestException`, which states the
/// constraint and names nothing from the request.
pub fn validate_condition_size(condition: &wire::RoutingRuleCondition) -> Result<(), String> {
    if let Some(base_paths) = &condition.match_base_paths {
        within_bounds(base_paths.any_of.len(), base_paths.any_of.iter().map(String::as_str))?;
    }
    if let Some(headers) = &condition.match_headers {
        within_bounds(
            headers.any_of.len(),
            headers
                .any_of
                .iter()
                .flat_map(|value| [value.header.as_str(), value.value_glob.as_str()]),
        )?;
    }
    Ok(())
}

/// The two readings above, over a condition's value count and its values.
fn within_bounds<'v>(count: usize, values: impl Iterator<Item = &'v str>) -> Result<(), String> {
    if count > MAX_CONDITION_VALUES {
        return Err(condition_value_count_message());
    }
    for value in values {
        if value.len() > MAX_CONDITION_VALUE_BYTES {
            return Err(condition_value_length_message());
        }
    }
    Ok(())
}

/// The refusal a condition naming more values than the bound takes, built from the bound so
/// the sentence cannot drift from the constraint it explains.
pub fn condition_value_count_message() -> String {
    format!("A routing rule condition can match on at most {MAX_CONDITION_VALUES} values.")
}

/// The refusal a condition carrying a value longer than the bound takes, built the same way.
pub fn condition_value_length_message() -> String {
    format!(
        "A routing rule condition value can be at most {MAX_CONDITION_VALUE_BYTES} characters."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `matchBasePaths` condition over `count` values, each `length` bytes long.
    fn base_paths(count: usize, length: usize) -> wire::RoutingRuleCondition {
        wire::RoutingRuleCondition {
            match_base_paths: Some(wire::RoutingRuleMatchBasePaths {
                any_of: (0..count).map(|_| "b".repeat(length)).collect(),
            }),
            match_headers: None,
        }
    }

    /// A `matchHeaders` condition over `count` entries, each glob `length` bytes long.
    fn match_headers(count: usize, length: usize) -> wire::RoutingRuleCondition {
        wire::RoutingRuleCondition {
            match_base_paths: None,
            match_headers: Some(wire::RoutingRuleMatchHeaders {
                any_of: (0..count)
                    .map(|_| wire::RoutingRuleMatchHeaderValue {
                        header: "x-tier".to_string(),
                        value_glob: "g".repeat(length),
                    })
                    .collect(),
            }),
        }
    }

    /// RULE: a condition is admitted up to the bounds and refused past either of them, for
    /// both kinds of condition.
    #[test]
    fn a_condition_is_admitted_up_to_the_bounds_and_refused_past_them() {
        // The rows walk each bound: one value, the count at the bound, one past it, a value
        // at the length bound, and one byte past it.
        let cases: [(&str, usize, usize, Option<String>); 5] = [
            ("one short value", 1, 8, None),
            ("the value count at the bound", MAX_CONDITION_VALUES, 8, None),
            (
                "one value past the count bound",
                MAX_CONDITION_VALUES + 1,
                8,
                Some(condition_value_count_message()),
            ),
            ("a value at the length bound", 1, MAX_CONDITION_VALUE_BYTES, None),
            (
                "a value one byte past the length bound",
                1,
                MAX_CONDITION_VALUE_BYTES + 1,
                Some(condition_value_length_message()),
            ),
        ];
        for (case, count, length, expected) in cases {
            for (kind, condition) in [
                ("matchBasePaths", base_paths(count, length)),
                ("matchHeaders", match_headers(count, length)),
            ] {
                let actual = validate_condition_size(&condition);
                assert_eq!(
                    actual.clone().err(),
                    expected,
                    "case {case:?} as a {kind} condition: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
