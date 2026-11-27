// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the framework's boundary refusal of a request — the
//! `Service::override_refusal` decision, kept here rather than inline so the rule is stated
//! once for every operation.
//!
//! The words are NOT written here: `service_common::validation_message` is the
//! cross-service home for the envelope, the clauses, the member paths and the requirement
//! sentences. This module makes only the choices that belong to a service.
//!
//! THE RULE this service states: a clause whose requirement is the SET OF ADMISSIBLE
//! VALUES itself — a `@pattern` expression, or an enumerated type's value set — names the
//! member path and that set, and states NO value; a clause whose requirement is a BOUND on
//! the value (a `@length`, a `@range`) keeps the value half the framework renders. The set
//! already says what the member admits, so the one value that missed it adds nothing, and a
//! message built that way is bounded by the model instead of by the request. A `@pattern` is
//! quoted as this service advertises the expression, WITHOUT the `^`/`$` a whole-value
//! pattern wraps it in, and a clause about a member the model marks `@sensitive`
//! ([`WITHHELD_MEMBERS`]) states no value whatever its requirement.
//!
//! A clause about a member the request did not CARRY states no value either: there is no
//! submitted value to state, so the clause is the member path and the requirement alone.
//! The framework writes the literal `null` in that half — the slot it echoes a submitted
//! value in — and this service drops it, the same drop it makes for the classes above.
//!
//! The member path a clause names is the model's own traversal to the offending member, so
//! every segment of it is a member the request's shape has: [`member_path`] states that
//! rule, and it is what restores the union variants the read of an absent member does not
//! name.
//!
//! The code stays the modelled `ValidationException` (HTTP 400) — what the framework
//! already answered — so nothing a client branches on changes, only the words.
//!
//! Nothing here is keyed to an operation, a member or a submitted value, so the rule holds
//! for every constrained member of every operation of this service, at whatever depth and
//! through whatever binding carried the value.

/// How a member path is spelled: the model's traversal to the offending member.
mod member_path;

use std::sync::LazyLock;

use service_common::validation_message;
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// This service's Smithy model: the one source for which of its members are
/// `@sensitive`, and so for which values a boundary refusal states none — and for the shape
/// graph a refusal's member path is read against ([`member_path`]).
const MODEL_JSON: &str = include_str!(
    "../../smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json"
);

/// The members of this model whose value a refusal WITHHOLDS whatever its requirement, read
/// off [`MODEL_JSON`] rather than listed here, so the decision cannot drift from the model
/// as the model grows a `@sensitive` member.
///
/// This model marks the two resource NAMES (`BlueprintName`,
/// `DataAutomationProjectName`), the library name beside them and the blueprint schema, so
/// a request refused for the shape of a value it submitted is told which member and which
/// requirement, and never handed that value back.
static WITHHELD_MEMBERS: LazyLock<service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON));

/// The shape graph of [`MODEL_JSON`], which every member path a refusal names is read
/// against ([`member_path`]), so a path states the traversal the model declares rather than
/// the one the read of an absent member could name.
static MODELLED_PATHS: LazyLock<member_path::ModelledPaths> =
    LazyLock::new(|| member_path::ModelledPaths::from_model(MODEL_JSON));

/// Whether the clause for `violation` states NO value.
///
/// True for the two classes whose requirement IS the set of values the member admits — a
/// `@pattern` expression and an enumerated type's value set — for a member the request did
/// not carry at all, which submitted no value to state, and for every violation of a member
/// the model marks `@sensitive`.
fn withholds_value(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::Pattern(_)
            | ViolationKind::EnumValue(_)
            | ViolationKind::MissingRequiredMember
    ) || WITHHELD_MEMBERS.withholds_value(&violation.path)
}

/// How this service states `refusal`, or `None` to keep the framework's own wording for
/// anything that is not a constraint refusal (an unauthenticated caller keeps the
/// framework's own code, status and words).
pub fn refused(refusal: &Refusal) -> Option<Refused> {
    validation_message::aws_validation_refusal_withholding_violations(
        &at_modelled_paths(refusal),
        withholds_value,
    )
}

/// `refusal` with every member path it reports read back against the model
/// ([`member_path`]). Nothing else about it changes: the violation's class, its value and
/// its order are the framework's.
fn at_modelled_paths(refusal: &Refusal) -> Refusal {
    match refusal {
        Refusal::Invalid(violations) => Refusal::Invalid(
            violations
                .iter()
                .map(|violation| Violation {
                    path: MODELLED_PATHS.modelled(&violation.path),
                    ..violation.clone()
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, holding `value` — a framework refusal as the
    /// validating walk reports it.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    #[test]
    fn a_pattern_clause_states_the_expression_and_no_value() {
        let refusal = Refusal::Invalid(vec![violation(
            "libraryArn",
            ViolationKind::Pattern("^arn:aws:bedrock:[a-z0-9-]*$".to_string()),
            Some("arn:aws:bedrock:us-west-2:000000000000:blueprint/x"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'libraryArn' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: arn:aws:bedrock:[a-z0-9-]*",
            "expected the pattern clause to state no value, got {:?}",
            stated.message
        );
    }

    #[test]
    fn an_enum_clause_states_the_value_set_and_no_value() {
        let refusal = Refusal::Invalid(vec![violation(
            "entityType",
            ViolationKind::EnumValue(vec!["VOCABULARY".to_string()]),
            Some("NOPE"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'entityType' failed to satisfy constraint: \
             Member must satisfy enum value set: [VOCABULARY]",
            "expected the enum clause to state no value, got {:?}",
            stated.message
        );
    }

    #[test]
    fn a_bound_clause_keeps_the_value_the_framework_renders() {
        let refusal = Refusal::Invalid(vec![violation(
            "clientToken",
            ViolationKind::MinLength(33),
            Some("abc"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value 'abc' at 'clientToken' failed to satisfy \
             constraint: Member must have length greater than or equal to 33",
            "expected a bound clause to keep its value, got {:?}",
            stated.message
        );
    }

    #[test]
    fn a_sensitive_member_states_no_value_for_a_bound_either() {
        let refusal = Refusal::Invalid(vec![violation(
            "blueprintName",
            ViolationKind::MaxLength(128),
            Some("a-name-the-caller-sent"),
        )]);
        let stated = refused(&refusal).expect("a constraint refusal is restated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'blueprintName' failed to satisfy \
             constraint: Member must have length less than or equal to 128",
            "expected a withheld member to state no value, got {:?}",
            stated.message
        );
    }

    /// RULE: a clause about a member the request did not carry states NO value — the
    /// framework's `null` half is dropped — and names the member at the traversal the model
    /// declares, whatever depth and whatever union variants that traversal crosses.
    #[test]
    fn an_absent_member_states_no_value_at_the_modelled_path() {
        // (case, the path the read reports, the path the clause states)
        let rows: [(&str, &str, &str); 4] = [
            (
                "a top-level member",
                "outputConfiguration",
                "outputConfiguration",
            ),
            (
                "a member of a nested structure",
                "outputConfiguration.s3Uri",
                "outputConfiguration.s3Uri",
            ),
            (
                "a member under a union, a list and a second union",
                "inputConfiguration.inlinePayload.1.member.phrases",
                "inputConfiguration.inlinePayload.upsertEntitiesInfo.1.member.vocabulary.phrases",
            ),
            (
                "a member of the union's other variant",
                "inputConfiguration.inlinePayload.entityIds",
                "inputConfiguration.inlinePayload.deleteEntitiesInfo.entityIds",
            ),
        ];
        for (case, reported, stated_at) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                reported,
                ViolationKind::MissingRequiredMember,
                None,
            )]);
            let stated = refused(&refusal).expect("a constraint refusal is restated");
            let expected = format!(
                "1 validation error detected: Value at '{stated_at}' failed to satisfy \
                 constraint: Member must not be null"
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }
}
