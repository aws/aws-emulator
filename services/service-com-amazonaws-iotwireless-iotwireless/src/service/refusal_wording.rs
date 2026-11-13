// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request its front door refused before any handler ran.
//!
//! # The rule
//!
//! A refused value is reported in AWS's constraint envelope —
//! `N validation error(s) detected: Value['<value>'] at '<member path>' failed to satisfy
//! constraint: <requirement>` — whose only request-derived parts are the member path and,
//! on the stack that quotes it, the rejected value. The envelope, the member paths and
//! every requirement clause are the framework's, so nothing about them is spelled here
//! ([`service_common::validation_message`]); what this service chooses, and all it
//! chooses, is:
//!
//! * the ORDER an enumerated type's admissible values are listed in. This front door
//!   holds those values in a hash table keyed on each value's own text and enumerates the
//!   TABLE rather than the type's declaration, so the set reads neither in the model's
//!   declaration order nor sorted
//!   (`service_common::validation_message::in_hashed_enum_order`). The order is computed
//!   from the declared values themselves, so it belongs to the enumerated TYPE and a type
//!   the model gains needs no row anywhere.
//! * which values an enumerated type's set HOLDS, for the one type this service publishes
//!   a wider set of than the shipped model declares ([`PUBLISHED_ENUM_VALUE_SETS`]).
//! * how a `@pattern` requirement is quoted: as the model writes the expression, anchors
//!   included ([`as_modelled`]).
//! * whether the clause states the rejected VALUE at all, which is a property of the
//!   stack that fronts the refused operation and so is decided in
//!   [`super::refusal_envelope`]: the words for both stacks are composed here, over the
//!   same violations, and that layer answers each operation with its own. What the
//!   withholding stack withholds is the value the REQUEST SUBMITTED; a member the request
//!   did not carry keeps the `null` the walk reports it as.

use std::collections::BTreeSet;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use service_common::validation_message;
use smithy_server_runtime::{Refusal, Refused};

/// `ExpressionType`'s admissible values as this service PUBLISHES them, which is a wider
/// set than the shipped model's enum declares (`smithy_query(query="shape",
/// shape="ExpressionType")` lists `RuleName` and `MqttTopic` alone). A destination's
/// expression may also name an SNS topic, and the refusal of a value outside the set is
/// where the service states the whole of it.
///
/// Order is not stated here: every set is listed in the order the front door walks its own
/// table of it ([`stated`]).
pub const PUBLISHED_EXPRESSION_TYPES: &[&str] = &["RuleName", "MqttTopic", "SnsTopic"];

/// The enumerated types whose admissible set this service publishes other than the shipped
/// model declares it: one row per type, the model's declared values beside the service's.
///
/// Matched as a SET, so every member targeting the type is stated alike wherever it is
/// carried. Which values a member ADMITS is still the model's: a value the shipped enum
/// does not declare is refused by the boundary validator before any handler runs, and this
/// table only decides what the refusal SAYS the service accepts.
const PUBLISHED_ENUM_VALUE_SETS: &validation_message::StatedEnumValueSets<'static> = &[(
    crate::ops::types::EXPRESSION_TYPE_VALUES,
    PUBLISHED_EXPRESSION_TYPES,
)];

/// `violation` with its enumerated value set as this service states one: the values the
/// service publishes, in the order its front door's own table is walked.
///
/// Only the values move. The member path, the value half and the requirement's wording
/// keep coming from the one place that writes them, and a violation of any other class
/// comes back untouched.
fn stated(violation: &Violation) -> Violation {
    let published = validation_message::in_stated_enum_order(violation, PUBLISHED_ENUM_VALUE_SETS);
    validation_message::in_hashed_enum_order(&published)
}

/// The two wordings one refusal has, over the same violations: with the rejected value
/// QUOTED, and with that half WITHHELD.
///
/// Both come out of the one shared body, so the envelope, the member paths, the clause
/// count and every requirement are identical in the two and only the value half differs.
/// `None` for anything that is not a constraint refusal, and for a refusal with nothing to
/// state.
pub struct Wordings {
    /// The clause quoting the value the request submitted.
    pub quoting: Refused,
    /// The same clause with the submitted value withheld.
    pub withholding: Refused,
}

/// Word `refusal` both ways, or `None` to keep the framework's own wording — for an
/// authentication refusal, which is not a modelled-input failure and is not restated here.
///
/// What the withholding stack withholds is the value the REQUEST SUBMITTED. A member the
/// request did not carry submitted none, so its clause keeps the `null` the walk reports it
/// as — the requirement (`Member must not be null`) is the whole of what such a clause has
/// to say, and a clause naming no value at all would not say the member was absent.
pub fn wordings(refusal: &Refusal) -> Option<Wordings> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let violations: Vec<Violation> = violations.iter().map(stated).collect();
    // The members the request did not carry, which are the clauses that keep their value
    // half. A member is either absent or present, so one path cannot be both.
    let absent: BTreeSet<String> = violations
        .iter()
        .filter(|violation| violation.value.is_none())
        .map(|violation| violation.path.clone())
        .collect();
    let stated = Refusal::Invalid(violations);
    // `str::to_string` is the `@pattern` quoting this service shows: the expression as the
    // MODEL writes it, the anchors a whole-value pattern wraps it in included. The CHECK
    // always used the model's pattern as written, so this is only how it is quoted back.
    Some(Wordings {
        quoting: validation_message::aws_validation_refusal_with(&stated, str::to_string)?,
        withholding: validation_message::aws_validation_refusal_withholding_with(
            &stated,
            str::to_string,
            |path| !absent.contains(path),
        )?,
    })
}

/// The whole message this service states for a value outside an enumerated member's
/// published set, for a HANDLER that checks one the boundary validator cannot — the
/// published set being wider than the model's enum, a handler is where a value inside it
/// is admitted and a value outside it refused.
///
/// The same envelope, clause and value half as a refusal the front door raised for the
/// same member, built from the same two layers, so one rejected enumerated value reads
/// alike whichever of the two refused it.
///
/// TOTAL: a clause this builds always carries the value half the drop removes, and one clause
/// is always an envelope, so neither shared layer can decline it — and a handler refusing a
/// request must have a sentence to refuse it with, so each falls back to what it was handed
/// rather than to no message at all.
#[must_use]
pub fn enum_value_set_message(member_path: &str, published: &[&str]) -> String {
    let violation = stated(&Violation {
        path: member_path.to_string(),
        kind: ViolationKind::EnumValue(published.iter().map(|value| (*value).to_string()).collect()),
        // No value half: the clause this service states for a refused enumerated member
        // names the member and the set, and the framework's `null` is not that set either.
        value: None,
        sensitive: false,
    });
    let clause =
        validation_message::valueless_clause(&violation).unwrap_or_else(|| violation.message());
    validation_message::envelope_over(std::slice::from_ref(&clause)).unwrap_or(clause)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of each class, at `path`, holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// RULE: an enumerated type's set is listed in the order this front door's table is
    /// walked, over the values the service publishes — asserted for every enumerated type
    /// a recording of this service pins, plus one the table widens.
    #[test]
    fn an_enumerated_set_is_listed_as_the_service_lists_it() {
        // (case, the values the walk carried, the set as the service states it)
        let rows: &[(&str, &[&str], &str)] = &[
            (
                "the destination expression types, which the service publishes wider",
                crate::ops::types::EXPRESSION_TYPE_VALUES,
                "[RuleName, MqttTopic, SnsTopic]",
            ),
            (
                "the trace log levels",
                crate::ops::types::LOG_LEVEL_VALUES,
                "[ERROR, INFO, DISABLED]",
            ),
            (
                "the supported RF regions",
                crate::ops::types::SUPPORTED_RF_REGION_VALUES,
                "[CN470, EU433, AU915, EU868, AS923-1, AS923-2, CN779, AS923-3, AS923-4, KR920, \
                 IN865, RU864, US915]",
            ),
        ];

        for (case, declared, expected) in rows {
            let carried = violation(
                "member",
                ViolationKind::EnumValue(
                    declared.iter().map(|value| (*value).to_string()).collect(),
                ),
                Some("nope"),
            );
            let message = carried.message();
            let stated = stated(&carried).message();
            assert!(
                stated.ends_with(*expected),
                "case {case:?}: expected the set stated as {expected}, got {stated:?}"
            );
            assert!(
                stated.starts_with("Value 'nope' at 'member' failed to satisfy constraint: "),
                "case {case:?}: only the value set moves, got {stated:?}"
            );
            assert_ne!(
                message, stated,
                "case {case:?}: the service's order is not the walk's"
            );
        }
    }

    /// RULE: a violation of any other class is stated exactly as the framework states it,
    /// whichever member carried it.
    #[test]
    fn another_constraint_class_is_stated_as_the_framework_states_it() {
        // (case, the violation)
        let rows: &[(&str, Violation)] = &[
            (
                "a numeric ceiling",
                violation("loRaWAN.maxEirp", ViolationKind::MaxValue("15".to_string()), Some("999")),
            ),
            (
                "an expression pattern",
                violation("loRaWAN.gatewayEui", ViolationKind::Pattern("^[0-9A-Fa-f]{16}$".to_string()), Some("nope")),
            ),
            (
                "a length floor",
                violation("name", ViolationKind::MinLength(1), Some("")),
            ),
            (
                "an absent required member",
                violation("loRaWAN", ViolationKind::MissingRequiredMember, None),
            ),
        ];

        for (case, violation) in rows {
            assert_eq!(
                stated(violation).message(),
                violation.message(),
                "case {case:?}: expected the framework's own clause"
            );
        }
    }

    /// RULE: the two wordings of one refusal differ in the value half ALONE.
    #[test]
    fn the_two_wordings_differ_only_in_the_value_half() {
        let refusal = Refusal::Invalid(vec![violation(
            "expressionType",
            ViolationKind::EnumValue(
                crate::ops::types::EXPRESSION_TYPE_VALUES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
            ),
            Some("NotARealExpressionType"),
        )]);
        let worded = wordings(&refusal).expect("a constraint refusal is worded");
        assert_eq!(
            worded.quoting.message,
            "1 validation error detected: Value 'NotARealExpressionType' at 'expressionType' \
             failed to satisfy constraint: Member must satisfy enum value set: [RuleName, \
             MqttTopic, SnsTopic]",
            "expected the quoting stack's sentence, got {:?}",
            worded.quoting.message
        );
        assert_eq!(
            worded.withholding.message,
            "1 validation error detected: Value at 'expressionType' failed to satisfy constraint: \
             Member must satisfy enum value set: [RuleName, MqttTopic, SnsTopic]",
            "expected the withholding stack's sentence, got {:?}",
            worded.withholding.message
        );
        assert_eq!(
            worded.quoting.code, worded.withholding.code,
            "the code travels with the message on both stacks"
        );
    }

    /// RULE: the value half a clause keeps is the one the request SUBMITTED, so an absent
    /// member is stated the same way on both stacks.
    #[test]
    fn an_absent_member_keeps_its_value_half_on_both_stacks() {
        let refusal = Refusal::Invalid(vec![violation(
            "loRaWAN",
            ViolationKind::MissingRequiredMember,
            None,
        )]);
        let worded = wordings(&refusal).expect("a constraint refusal is worded");
        let expected = "1 validation error detected: Value null at 'loRaWAN' failed to satisfy \
                        constraint: Member must not be null";
        assert_eq!(
            worded.withholding.message, expected,
            "expected the absent member stated with its null, got {:?}",
            worded.withholding.message
        );
        assert_eq!(
            worded.quoting.message, expected,
            "expected the same sentence on the quoting stack, got {:?}",
            worded.quoting.message
        );
    }

    /// RULE: a handler's own refusal of an enumerated member states the same sentence the
    /// front door would for that member and set.
    #[test]
    fn a_handler_states_the_front_doors_sentence() {
        let stated = enum_value_set_message("expressionType", PUBLISHED_EXPRESSION_TYPES);
        assert_eq!(
            stated,
            "1 validation error detected: Value at 'expressionType' failed to satisfy constraint: \
             Member must satisfy enum value set: [RuleName, MqttTopic, SnsTopic]",
            "expected the one enumerated sentence, got {stated:?}"
        );
    }

    /// RULE: an authentication refusal keeps the framework's own wording.
    #[test]
    fn an_authentication_refusal_is_not_restated() {
        assert!(
            wordings(&Refusal::Unauthenticated).is_none(),
            "expected the framework's own wording for a refusal that is not about a value"
        );
    }
}
