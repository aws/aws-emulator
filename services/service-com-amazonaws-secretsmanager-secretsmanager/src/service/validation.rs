// SPDX-License-Identifier: CC0-1.0

//! How this service words the requests the framework refused before any handler ran.
//!
//! The boundary validator finds a value that failed the rules the model declares and hands
//! this service the violations alone ([`Violation`]: the member path, what it failed, and
//! the value as the walk rendered it). The envelope over those clauses, each clause itself
//! and the `@pattern` rendering are the SHARED ones
//! ([`service_common::validation_message::aws_validation_refusal_withholding_violations`]),
//! so this service cannot word one constraint differently from the next; what lives here is
//! only the three choices that are this service's own, and they are made by handing the
//! violations through before that shared rendering runs:
//!
//! * a rejected value is echoed only up to [`MAX_ECHOED_VALUE`] bytes, and a longer one
//!   states that bounded prefix followed by [`ELISION`] — the marker this service's own
//!   recorded clauses carry, which names no byte count (the other marker in the tree,
//!   `... [<n> bytes]`, is what [`service_common::validation::echo_held_to`] keeps). A
//!   caller controls the length of what it sends, and a value that failed a MAXIMUM bound is
//!   by definition longer than the member may hold, so echoing it whole reproduces in a
//!   response body — which a client logs, traces and attaches to a ticket — exactly what the
//!   service refused to store;
//! * an enumerated type's admitted values are stated in the order THIS SERVICE lists that
//!   set in ([`crate::state::STATED_VALUE_SETS`]), which is a property of the enumerated
//!   type rather than of the member, the operation or the request that carried the value;
//! * a `Name` the service cannot use carries the one sentence stating what a name may be
//!   built from ([`crate::state::INVALID_SECRET_NAME_MESSAGE`]), the same sentence the
//!   create's own charset check answers with, so one unusable name reads the same whether
//!   the boundary or the handler refused it; and a `SecretIdList` longer than one batch
//!   states the bound as the batched read's own parameter refusal.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code every constraint refusal of this service travels under, which is also
/// the code the shared rendering answers: a client branches on the code, so rewording a
/// clause must not move it.
const CONSTRAINT_CODE: &str = "ValidationException";

/// The code the batched read's parameter refusal travels under: "The parameter name or
/// value is invalid." (`InvalidParameterException`, this service's Smithy model).
const PARAMETER_CODE: &str = "InvalidParameterException";

/// How many bytes of a rejected value a clause of this service echoes before the rest is
/// elided.
const MAX_ECHOED_VALUE: usize = 64;

/// What stands in for the bytes an elided echo leaves out.
const ELISION: &str = " ...";

/// The member path `CreateSecret`'s `Name` is reported at, as the framework spells a
/// top-level member (lowerCamel). It is the only member of this service's model at that
/// path, so the name rule below reaches nothing else.
const NAME_PATH: &str = "name";

/// The path the batched read's id list is reported at.
const SECRET_ID_LIST_PATH: &str = "secretIdList";

/// How this service words `refusal`, or `None` to keep the framework's own wording.
pub(super) fn reword(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // Secrets Manager answers a `SecretIdList` longer than one batch with
    // `InvalidParameterException` and a sentence stating the bound, and a client reading
    // the batched API branches on that modelled code rather than on a constraint refusal.
    if violations.iter().any(|violation| {
        violation.path == SECRET_ID_LIST_PATH
            && matches!(violation.kind, ViolationKind::MaxLength(_))
    }) {
        return Some(Refused {
            code: PARAMETER_CODE.to_string(),
            message: crate::ops::batch_get_secret_value::too_many_ids_message(),
        });
    }
    // A name outside what a name may be is one refusal with one sentence, whichever bound
    // it fell outside. An ABSENT name is not an unusable one — it is a member the shape
    // requires — so it keeps the shared "must not be null" clause.
    if violations.iter().any(unusable_name) {
        return Some(Refused {
            code: CONSTRAINT_CODE.to_string(),
            message: crate::state::INVALID_SECRET_NAME_MESSAGE.to_string(),
        });
    }
    let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
    // The envelope, the member paths, the `@pattern` rendering and every requirement's
    // wording are the shared ones; a `@sensitive` member's value is withheld, which the
    // runtime schemas cannot say and the walk records on the violation.
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        &Refusal::Invalid(stated),
        |violation| violation.sensitive,
    )
}

/// Whether `violation` is a `Name` the service cannot use: a value that failed one of the
/// rules a name has to satisfy, rather than an absent member.
fn unusable_name(violation: &Violation) -> bool {
    violation.path == NAME_PATH
        && matches!(
            violation.kind,
            ViolationKind::MinLength(_) | ViolationKind::MaxLength(_) | ViolationKind::Pattern(_)
        )
}

/// `violation` as this service states it: the admitted values of an enumerated type in the
/// order this service lists them, and the echoed value held to what a clause may carry.
fn as_stated(violation: &Violation) -> Violation {
    let ordered = service_common::validation_message::in_stated_enum_order(
        violation,
        crate::state::STATED_VALUE_SETS,
    );
    Violation {
        value: ordered.value.as_deref().map(echoed),
        ..ordered
    }
}

/// `value` as a clause of this service may echo it ([`MAX_ECHOED_VALUE`], [`ELISION`]).
fn echoed(value: &str) -> String {
    service_common::validation::echo_elided_with(value, MAX_ECHOED_VALUE, ELISION).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A violation of `path` carrying `value`, as the framework's walk records one.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// The message this service words for one violation.
    fn said(violation: Violation) -> String {
        reword(&Refusal::Invalid(vec![violation]))
            .expect("this service words every constraint class it is handed")
            .message
    }

    #[test]
    fn a_value_within_the_echo_bound_is_stated_as_sent() {
        let value = "d".repeat(MAX_ECHOED_VALUE);
        let stated = said(violation(
            "description",
            ViolationKind::MaxLength(2048),
            Some(value.as_str()),
        ));
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'description' failed to satisfy \
             constraint: Member must have length less than or equal to 2048"
        );
        assert_eq!(
            stated, expected,
            "case \"at the echo bound\": expected {expected:?}, got {stated:?}"
        );
    }

    #[test]
    fn a_value_past_the_echo_bound_is_stated_elided() {
        let stated = said(violation(
            "tags.1.member.key",
            ViolationKind::MaxLength(128),
            Some("k".repeat(MAX_ECHOED_VALUE + 1).as_str()),
        ));
        let expected = format!(
            "1 validation error detected: Value '{}{ELISION}' at 'tags.1.member.key' failed to \
             satisfy constraint: Member must have length less than or equal to 128",
            "k".repeat(MAX_ECHOED_VALUE)
        );
        assert_eq!(
            stated, expected,
            "case \"one past the echo bound\": expected {expected:?}, got {stated:?}"
        );
    }

    #[test]
    fn a_pattern_is_stated_without_its_whole_value_anchors() {
        let stated = said(violation(
            "rotationRules.duration",
            ViolationKind::Pattern("^[0-9]+h$".to_string()),
            Some("1x"),
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value '1x' at 'rotationRules.duration' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: [0-9]+h",
            "case \"an anchored pattern\": got {stated:?}"
        );
    }

    #[test]
    fn an_enum_clause_states_the_admitted_set_in_the_order_the_service_states_it() {
        // `FilterNameStringType`, as the model declares it
        // (`smithy_query(query="shape",shape="FilterNameStringType")`); the clause states
        // the same values in the order recorded behaviour shows this service listing them.
        let declared = [
            "description",
            "name",
            "tag-key",
            "tag-value",
            "primary-region",
            "owning-service",
            "all",
        ];
        let stated = said(violation(
            "filters.1.member.key",
            ViolationKind::EnumValue(declared.iter().map(|value| value.to_string()).collect()),
            Some("secret-name"),
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value 'secret-name' at 'filters.1.member.key' failed \
             to satisfy constraint: Member must satisfy enum value set: [all, owning-service, \
             name, tag-key, description, tag-value, primary-region]",
            "case \"a set the service states an order for\": got {stated:?}"
        );
    }

    #[test]
    fn a_value_set_the_service_states_no_order_for_is_stated_as_the_model_declares_it() {
        let declared = ["asc", "desc"];
        let stated = said(violation(
            "sortOrder",
            ViolationKind::EnumValue(declared.iter().map(|value| value.to_string()).collect()),
            Some("sideways"),
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value 'sideways' at 'sortOrder' failed to satisfy \
             constraint: Member must satisfy enum value set: [asc, desc]",
            "case \"a set with no stated order\": got {stated:?}"
        );
    }

    #[test]
    fn an_absent_member_is_stated_as_the_null_it_was() {
        let stated = said(violation(
            "secretId",
            ViolationKind::MissingRequiredMember,
            None,
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value null at 'secretId' failed to satisfy constraint: \
             Member must not be null",
            "case \"an absent member\": got {stated:?}"
        );
    }

    #[test]
    fn a_sensitive_members_value_is_withheld() {
        let mut withheld = violation("secretString", ViolationKind::MaxLength(65536), None);
        withheld.sensitive = true;
        let stated = said(withheld);
        assert_eq!(
            stated,
            "1 validation error detected: Value at 'secretString' failed to satisfy constraint: \
             Member must have length less than or equal to 65536",
            "case \"a sensitive member\": got {stated:?}"
        );
    }

    #[test]
    fn several_violations_are_counted_and_joined() {
        let refusal = Refusal::Invalid(vec![
            violation("maxResults", ViolationKind::MaxValue("100".to_string()), Some("101")),
            violation("nextToken", ViolationKind::MinLength(1), Some("")),
        ]);
        let stated = reword(&refusal).expect("two violations are worded").message;
        assert_eq!(
            stated,
            "2 validation errors detected: Value '101' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 100; Value '' at \
             'nextToken' failed to satisfy constraint: Member must have length greater than or \
             equal to 1",
            "case \"two violations\": got {stated:?}"
        );
    }

    #[test]
    fn an_unusable_name_carries_the_one_sentence_for_a_name() {
        for (case, kind) in [
            ("a name past its maximum", ViolationKind::MaxLength(512)),
            ("an empty name", ViolationKind::MinLength(1)),
        ] {
            let stated = said(violation(NAME_PATH, kind, Some("n")));
            assert_eq!(
                stated,
                crate::state::INVALID_SECRET_NAME_MESSAGE,
                "case {case:?}: expected the name sentence, got {stated:?}"
            );
        }
    }

    #[test]
    fn an_absent_name_keeps_the_shared_required_member_clause() {
        let stated = said(violation(
            NAME_PATH,
            ViolationKind::MissingRequiredMember,
            None,
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value null at 'name' failed to satisfy constraint: \
             Member must not be null",
            "case \"an absent Name\": got {stated:?}"
        );
    }

    #[test]
    fn a_multibyte_value_is_elided_on_a_character_boundary() {
        // "é" is two bytes, so a run of them has no boundary at an odd byte offset.
        let value = "é".repeat(MAX_ECHOED_VALUE);
        let shown = echoed(&value);
        assert!(
            shown.ends_with(ELISION) && shown.len() <= MAX_ECHOED_VALUE + ELISION.len(),
            "case \"a multi-byte value\": expected a bounded elision, got {shown:?}"
        );
    }
}
