// SPDX-License-Identifier: CC0-1.0

//! The WORDS of the refusal the framework's boundary walk produces for this service: how a
//! set of constraint violations becomes the `message` a 400 `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAM — which requests are checked, and how the violations and
//! the request's own values are collected — and this module owns the sentence, so the clause
//! rules live in one place instead of in each operation that trips them. What is ENFORCED is
//! always the model's: every violation handed to [`refusal`] comes from the framework's own
//! walk, and nothing here accepts or refuses anything.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! one report happened to cite:
//!
//! 1. the envelope is AWS's, `<N> validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; ` ([`envelope`]);
//! 2. a clause states the value as SUBMITTED, the member's dotted path and the requirement it
//!    failed, in the framework's own words ([`requirement`]);
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes to
//!    demand a whole-value match (`service_common::validation::unanchored`, the tree's one
//!    renderer of that rule);
//! 4. the echoed value is entity-escaped TWICE ([`entity_escaped_twice`]);
//! 5. a constraint broken by an ELEMENT of a collection is stated at the COLLECTION member,
//!    with no position and no `member` step: the value echoed is the whole collection the
//!    member held — the list the caller sent ([`collection_value`]) — and the element's
//!    requirement is nested inside `Member must satisfy constraint: [ … ]`. Two elements of
//!    one list that broke the same rule are therefore ONE clause, while a member that broke
//!    two rules still states both. A constraint broken by the list ITSELF (its own
//!    `@length`) is not nested, and neither is one broken by a member INSIDE a structure
//!    element, which keeps its own full path;
//! 6. an enumerated type's value SET is listed as the SERVICE states that enum, which is
//!    neither the order nor always the membership the model declares
//!    ([`stated_value_set`]); applied inside [`requirement`], so one enum reads alike
//!    whether its clause is stated at the enumerated member itself or nested inside one
//!    about the collection that holds it.
//!
//! The status (400) and the error code ([`VALIDATION_EXCEPTION`]) are what the framework
//! already answers a constraint failure with on this restJson1 service and are restated, not
//! changed: a chosen [`Refused`] replaces the code and the message together.

use std::collections::{HashMap, HashSet};

use ::service_common::validation::{list_of_element, rendered_list, unanchored};
use ::smithy_server_runtime::{Refused, Violation, ViolationKind};

/// The error code this service answers an input-constraint violation with: the modeled
/// `ValidationException`, which every operation of this service declares with
/// `@httpError 400` (the framework's own default code for a refusal, restated here because
/// a chosen [`Refused`] replaces code and message together).
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// How a clause nests the requirement an element of a collection broke (rule 5).
const ELEMENT_CONSTRAINT_OPEN: &str = "Member must satisfy constraint: [";

/// The deepest collection nesting one clause walks out of.
///
/// A step out of a collection is spelled `<position>.member`, and the step a MAP entry
/// carries is the key the CALLER chose, so a key spelled `<digits>.member` is
/// indistinguishable from a list position by the path alone: without a ceiling, the nesting
/// a request could ask for would grow with the length of such a key. The deepest collection
/// an input of this service declares is a list of scalars inside a structure inside a list
/// (`filters.<i>.member.filterValues.<j>.member`), which is ONE element step out of its own
/// collection; the ceiling is one past that, enough to state a list of lists were the model
/// to grow one.
const MAX_ELEMENT_DEPTH: usize = 2;

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations) or a violation class this module has no wording for
/// ([`ViolationKind`] is `#[non_exhaustive]`), which keeps the framework's own.
///
/// `lists` holds every collection of scalar elements the request carried, keyed by the
/// member path the walk reports that collection at and already rendered
/// (`service_common::validation::submitted_lists`) — the value a clause stated at a
/// collection member needs and no violation carries.
pub(crate) fn refusal(
    violations: &[Violation],
    lists: &HashMap<String, String>,
) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: message(violations, lists)?,
    })
}

/// The `message` for a set of violations: the count, then one clause per failing (member,
/// requirement), in the order the walk found them.
fn message(violations: &[Violation], lists: &HashMap<String, String>) -> Option<String> {
    let mut stated: HashSet<(String, String)> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        let mut requirement = requirement(&violation.kind)?;
        for _ in 0..depth {
            requirement = format!("{ELEMENT_CONSTRAINT_OPEN}{requirement}]");
        }
        if !stated.insert((path.to_string(), requirement.clone())) {
            continue;
        }
        let held = (depth > 0)
            .then(|| collection_value(violations, path, lists))
            .flatten();
        clauses.push(match (&violation.value, held) {
            // An absent member has no value to echo: that is a fact of the request's shape,
            // not a value the caller sent, so there is nothing to escape.
            (None, _) => {
                format!("Value null at '{path}' failed to satisfy constraint: {requirement}")
            }
            (Some(submitted), held) => {
                let shown = held.as_deref().unwrap_or(submitted.as_str());
                format!(
                    "Value '{}' at '{path}' failed to satisfy constraint: {requirement}",
                    entity_escaped_twice(shown)
                )
            }
        });
    }
    envelope(&clauses)
}

/// AWS's envelope over the clauses of one refusal: `1 validation error detected: <clause>`
/// for one, `<N> validation errors detected: <clause>; <clause>` for several.
fn envelope(clauses: &[String]) -> Option<String> {
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The member path a clause names, and how many collection steps it walked out of to get
/// there: `facetTypes.1.member` is stated at `facetTypes`, one step out (rule 5).
///
/// Bounded by [`MAX_ELEMENT_DEPTH`], so a path that keeps looking like an element step
/// cannot nest a requirement without end. The element spelling itself is
/// `service_common::validation::list_of_element`'s, the one place in the tree that mirrors
/// the validating walk's path bookkeeping.
fn collapsed(path: &str) -> (&str, usize) {
    let mut at = path;
    let mut depth = 0;
    while depth < MAX_ELEMENT_DEPTH {
        match list_of_element(at) {
            Some(collection) => {
                at = collection;
                depth += 1;
            }
            None => break,
        }
    }
    (at, depth)
}

/// What the collection at `path` HELD, as a clause states it.
///
/// The request's own list when it is in hand (`service_common::validation::submitted_lists`
/// renders every element the caller sent, in position order); otherwise the elements of that
/// list the walk REFUSED, which is as much of the member's value as the violations
/// themselves know. The fallback understates a list of which only some elements were
/// refused, and is only reached on the path that is handed the violations alone
/// ([`crate::service::EmulatedService::override_refusal`]).
fn collection_value(
    violations: &[Violation],
    path: &str,
    lists: &HashMap<String, String>,
) -> Option<String> {
    if let Some(rendered) = lists.get(path) {
        return Some(rendered.clone());
    }
    let mut elements: Vec<&str> = Vec::new();
    for violation in violations {
        let (collection, depth) = collapsed(&violation.path);
        if depth == 0 || collection != path {
            continue;
        }
        if let Some(value) = violation.value.as_deref() {
            if !elements.contains(&value) {
                elements.push(value);
            }
        }
    }
    (!elements.is_empty()).then(|| rendered_list(&elements))
}

/// The requirement half of a clause: what the value had to satisfy, as this service states
/// it.
///
/// `None` for a violation class this service has no wording for, which lets [`refusal`]
/// decline the whole refusal and keep the framework's wording rather than invent a sentence
/// for it.
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // The anchors a model writes to demand a whole-value match are dropped by the
        // tree's one renderer of that rule, rather than by a second copy here.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!(
                "Member must satisfy enum value set: [{}]",
                stated_value_set(allowed).join(", ")
            )
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// One enum's value set as this service STATES it, beside the set the MODEL declares for it.
///
/// A row is used only when the membership the framework reports is exactly that row's
/// `modelled` set, so an enum this service states nothing for — and a model change that adds
/// or removes a value — keeps what the framework reported instead of silently dropping a
/// value from the clause or inventing one.
struct StatedValueSet {
    /// The values the MODEL declares, which is the membership the framework reports.
    modelled: &'static [&'static str],
    /// The set as this service lists it: fixed text of the refusal, carrying nothing of the
    /// request.
    stated: &'static [&'static str],
}

/// Every enum value set recorded behaviour establishes a stated form for, one row per enum.
///
/// The `modelled` side is the model-derived constant generated from that enum, so a row
/// cannot fall behind the model; the `stated` side is fixed text of this service's refusals.
///
/// Where a recorded refusal predates a value the model has since gained, the recorded order
/// is kept for the values it covered and the values it did not follow them in the model's own
/// declaration order — so every value the clause has to list is listed, and no recorded
/// position is moved to make room for one.
const STATED_VALUE_SETS: &[StatedValueSet] = &[
    // `SearchFilterType`: the nine values the recorded refusal listed, in the order it listed
    // them — the model declares `MIN_AVERAGE_CUSTOMER_RATING` before
    // `NUMBER_OF_PRODUCTS` — followed by the six the model has gained since.
    StatedValueSet {
        modelled: crate::ops::types::SEARCH_FILTER_TYPE_VALUES,
        stated: &[
            "MAX_AVERAGE_CUSTOMER_RATING",
            "FULFILLMENT_OPTION_TYPE",
            "MIN_AVERAGE_CUSTOMER_RATING",
            "PRICING_MODEL",
            "NUMBER_OF_PRODUCTS",
            "DEPLOYED_ON_AWS",
            "PUBLISHER",
            "PRICING_UNIT",
            "CATEGORY",
            "CERTIFICATES",
            "PRODUCT_ID",
            "SELLER_ENGAGEMENT",
            "SPECIALIZATION",
            "SPECIALIZATION_CATEGORY",
            "TRUST_CENTER",
        ],
    },
    // `PurchaseOptionFilterType`: the same five values, in this service's own order.
    StatedValueSet {
        modelled: crate::ops::types::PURCHASE_OPTION_FILTER_TYPE_VALUES,
        stated: &[
            "PURCHASE_OPTION_TYPE",
            "VISIBILITY_SCOPE",
            "PRODUCT_ID",
            "AVAILABILITY_STATUS",
            "SELLER_OF_RECORD_PROFILE_ID",
        ],
    },
    // `SearchFacetType`: the nine values the recorded refusal listed, in an order of its own
    // — one of them, `CERTIFICATES`, a facet type the model did not declare when that
    // refusal was recorded and does now — followed by the four the model has gained since.
    StatedValueSet {
        modelled: crate::ops::types::SEARCH_FACET_TYPE_VALUES,
        stated: &[
            "FULFILLMENT_OPTION_TYPE",
            "PRICING_MODEL",
            "NUMBER_OF_PRODUCTS",
            "DEPLOYED_ON_AWS",
            "CERTIFICATES",
            "PUBLISHER",
            "AVERAGE_CUSTOMER_RATING",
            "PRICING_UNIT",
            "CATEGORY",
            "SELLER_ENGAGEMENT",
            "SPECIALIZATION",
            "SPECIALIZATION_CATEGORY",
            "TRUST_CENTER",
        ],
    },
];

/// The admissible values as this service LISTS them in an enum clause.
///
/// The membership the framework reports is the enum shape's own, in the order the model
/// happens to declare the values in. This service states a value set the same way on every
/// run and for every member the set constrains ([`STATED_VALUE_SETS`]); a set no row matches
/// is listed as reported.
///
/// Public so a test reads the same constants the refusal does.
pub(crate) fn stated_value_set(reported: &[String]) -> Vec<&str> {
    for row in STATED_VALUE_SETS {
        if row.modelled.len() == reported.len()
            && row
                .modelled
                .iter()
                .all(|value| reported.iter().any(|found| found == value))
        {
            return row.stated.to_vec();
        }
    }
    reported.iter().map(String::as_str).collect()
}

/// The rejected value as the report echoes it: entity-escaped, then entity-escaped AGAIN, so
/// a caller's `&` reaches the client as `&amp;amp;`.
///
/// Recorded AWS behaviour for this service: a value carrying a markup-significant character
/// comes back doubly escaped, the validation frontend escaping it once while building the
/// report and the response serializer escaping the result again. The rule is the double
/// escape of the request-derived value; the clause around it is unchanged. Only the three
/// characters that are markup-significant in text (`&`, `<`, `>`) are escaped — quotes are
/// left alone, since the clause quotes the value with `'` of its own and no recorded response
/// shows a quote escaped.
fn entity_escaped_twice(value: &str) -> String {
    entity_escaped(&entity_escaped(value))
}

/// One entity-escaping pass over the characters that are markup-significant in text. `&` is
/// handled first by construction: each character is mapped independently, so an entity this
/// pass introduces is never rescanned.
///
/// Deliberately NOT the framework's `xml_escape`: that one also escapes `"` and `'` and emits
/// numeric references for control characters, which would alter values no recorded response
/// attests, and it is an XML-envelope concern while this is a step in building the report's
/// text.
fn entity_escaped(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every stated value set holds exactly the membership its row's model-derived
    /// constant does, EXCEPT where recorded behaviour states a value the model does not
    /// declare — so a row cannot silently drop a modelled value from a clause.
    #[test]
    fn a_stated_value_set_states_every_modelled_value() {
        for row in STATED_VALUE_SETS {
            for value in row.modelled {
                assert!(
                    row.stated.contains(value),
                    "case {:?}: the stated set must carry every modelled value, got {:?}",
                    row.modelled,
                    row.stated
                );
            }
            assert!(
                row.stated.len() >= row.modelled.len(),
                "case {:?}: the stated set may add a value the model does not declare but \
                 never hold fewer, got {:?}",
                row.modelled,
                row.stated
            );
        }
    }

    /// RULE: a set this service states no order for is listed as the framework reported it.
    #[test]
    fn an_enum_with_no_stated_set_is_listed_as_reported() {
        let reported: Vec<String> = ["OFFER", "OFFERSET"]
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        assert_eq!(
            stated_value_set(&reported),
            vec!["OFFER", "OFFERSET"],
            "case PurchaseOptionType: expected the reported membership and order"
        );
    }
}
