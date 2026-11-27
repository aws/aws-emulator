// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refused before any handler ran: a member
//! outside a modelled constraint, a `@required` member absent, or a value no
//! modelled type can be read from.
//!
//! # The rule
//!
//! Every such refusal is HTTP 400 with the service's OWN modelled
//! `ValidationException` — the error the model binds to every operation
//! (`smithy_query(query="digest")` reports `common_errors: [AccessDeniedException,
//! InternalServerException, ThrottlingException, ValidationException]`) — so the body
//! carries that shape's members: the `@required` `message`, the `@required` `reason`
//! drawn from `ValidationExceptionReason`, and `fieldList` naming each member that
//! failed (`smithy_query(query="shape", shape="ValidationException")`). A `@required`
//! member of a modelled error shape is never absent from a response, so a client that
//! branches on `reason` always has one to read — the same body the handler-side
//! refusals of this service already build through [`crate::state::invalid_member_value`].
//!
//! The framework's own pre-dispatch renderer gets the status (400) and the wire code
//! (`ValidationException`) right and the BODY wrong: its only seam,
//! `Service::override_refusal`, can choose a code and a sentence and nothing else, so
//! the rendered body is `message` alone. [`modeled_refusal`] therefore states the
//! refusal itself, from the framework's own deserializer and constraint walk, and
//! serializes it through the model's `ValidationException` schema. `super`'s
//! `InModelledEnvelope` is what puts it in front of every operation.
//!
//! # The refusals that are NOT a `ValidationException`
//!
//! Two classes of bad request BODY are not member-level constraint failures, and Aurora
//! DSQL answers both [`BAD_REQUEST_CODE`] — an error its REST frontend answers before any
//! operation's own errors are reachable, so there is no modelled shape to serialize:
//! a tag map carrying more entries than `TagMap` admits ([`BAD_REQUEST_MESSAGE`]), and a
//! tag key or value outside the character set both tag shapes' `@pattern` admits
//! ([`crate::state::TAG_CHARACTER_SET_MESSAGE`]). [`refusal_for`] declines each of them and
//! the framework's refusal path, worded by `super`'s `override_refusal` from these same
//! helpers, answers it.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema::serde::SerdeError;
use ::aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    DeserializableStruct, Operation, Refused, ServerHttpProtocol, Violation, ViolationKind,
};

use crate::ops::types::{ValidationException, ValidationExceptionField, SCHEMA_VALIDATION_EXCEPTION};

/// `TagMap`'s modelled maximum entry count (`smithy_query(query="shape",
/// shape="TagMap")`: `@length(min: 0, max: 200)`), read from the one place this
/// crate spells that bound. A request carrying more tags than this is a bad
/// request BODY, not a member-level constraint failure — see [`refused_for`].
const TAG_MAP_MAX_ENTRIES: i64 = crate::state::MAX_TAGS_PER_RESOURCE as i64;

/// The member every `TagMap` in this service is bound to (`tags` on
/// `CreateCluster`, `CreateStream` and `TagResource`:
/// `smithy_query(query="operation", operation="CreateCluster,CreateStream,TagResource")`).
const TAG_MAP_MEMBER: &str = "tags";

/// The error code Aurora DSQL refuses a malformed request body with, and the
/// sentence it carries. Not a modelled shape: the service's REST frontend
/// answers it before any operation's own errors are reachable.
pub(super) const BAD_REQUEST_CODE: &str = crate::state::BAD_REQUEST_CODE;
/// See [`BAD_REQUEST_CODE`].
const BAD_REQUEST_MESSAGE: &str = "Invalid request body";

/// The wire error code of `com.amazonaws.dsql#ValidationException` — the shape name,
/// which is what an SDK matches a restJson1 error on.
pub(super) const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` for a value the member's modelled TYPE could not be
/// read from: nothing was parsed, so no constraint is named. Source for the value set:
/// `smithy_query(query="shape", shape="ValidationExceptionReason")` — `unknownOperation`,
/// `cannotParse`, `fieldValidationFailed`, `deletionProtectionEnabled`, `other`.
const REASON_CANNOT_PARSE: &str = "cannotParse";

/// How this service states a set of constraint violations.
///
/// Three refusals, told apart by WHAT failed:
///
/// * a tag map over [`TAG_MAP_MAX_ENTRIES`] is a bad request body, so the answer
///   is [`BAD_REQUEST_CODE`] with [`BAD_REQUEST_MESSAGE`] — the whole batch is
///   refused and none of it is applied;
/// * a tag key or value outside the character set both tag shapes' `@pattern`
///   admits is the same class of bad body, answered with that same code and the
///   sentence that class carries
///   ([`crate::state::TAG_CHARACTER_SET_MESSAGE`]) — the one the handlers word a
///   tag KEY with, which the walk below does not reach (codegen captures a map's
///   value constraints, not its keys), so a request is refused the same way
///   whichever half of the pair failed;
/// * everything else is a `ValidationException` whose message is the AWS
///   envelope built from the member and its modelled constraint. DSQL's
///   construction states the requirement ONLY: it does not echo the rejected
///   value back to the caller, and it quotes a `@pattern` as modelled minus the
///   `^`/`$` anchors.
///
/// This is the CODE-and-sentence half of a refusal, which is all the framework's own
/// renderer can carry. The body a modelled `ValidationException` owes besides the
/// sentence is built by [`refusal_for`], and the two are worded from these same
/// helpers so the paths cannot disagree.
pub(super) fn refused_for(violations: &[Violation]) -> Refused {
    if violations.iter().any(is_tag_map_overflow) {
        return Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: BAD_REQUEST_MESSAGE.to_string(),
        };
    }
    if violations.iter().any(is_outside_the_tag_character_set) {
        return Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: crate::state::TAG_CHARACTER_SET_MESSAGE.to_string(),
        };
    }
    Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: validation_envelope(violations),
    }
}

/// Whether `violation` is a tag map carrying more entries than `TagMap` allows.
fn is_tag_map_overflow(violation: &Violation) -> bool {
    violation.path.rsplit('.').next() == Some(TAG_MAP_MEMBER)
        && matches!(violation.kind, ViolationKind::MaxLength(max) if max == TAG_MAP_MAX_ENTRIES)
}

/// Whether `violation` is a tag key or value that left the character set its shape's
/// `@pattern` admits.
///
/// The pattern is the only one declared anywhere under either member that carries
/// caller-supplied tag text — the `tags` map's values and the `tagKeys` list's
/// elements, each reported at a path that STARTS with that member
/// (`tags.<key>`, `tagKeys.<n>.member`) — so a pattern violation under one of them is
/// that character set and nothing else.
fn is_outside_the_tag_character_set(violation: &Violation) -> bool {
    let member = violation.path.split('.').next().unwrap_or_default();
    matches!(violation.kind, ViolationKind::Pattern(_))
        && (member == TAG_MAP_MEMBER || member == crate::state::TAG_KEYS_MEMBER)
}

/// `1 validation error detected: <clause>` / `N validation errors detected:
/// <clause>; <clause>`, each clause as [`clause_for`] words it.
fn validation_envelope(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(clause_for).collect();
    match clauses.len() {
        // The framework only calls this with at least one violation; an empty
        // set still has to read as a refusal rather than as an empty sentence.
        0 => "1 validation error detected".to_string(),
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// One violation as DSQL states it: the member and the requirement it failed,
/// with no echo of the value the caller sent.
fn clause_for(violation: &Violation) -> String {
    let path = &violation.path;
    // `ViolationKind` is `#[non_exhaustive]`: a class this service has no wording
    // for keeps the framework's own clause rather than being dropped.
    let Some(requirement) = requirement_for(violation) else {
        return violation.message();
    };
    if matches!(violation.kind, ViolationKind::MissingRequiredMember) {
        // An absent member has no value to withhold, so the clause names the
        // absence itself.
        return format!("Value null at '{path}' failed to satisfy constraint: {requirement}");
    }
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement `violation` failed, as DSQL words it — the half of a clause that
/// says what the member had to satisfy, and the sentence a `fieldList` entry carries
/// for that member. `None` for a constraint class this service has no wording for.
fn requirement_for(violation: &Violation) -> Option<String> {
    Some(match &violation.kind {
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
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// A `@pattern` as DSQL quotes it: the modelled regex without its leading `^`
/// and trailing `$`, which are the anchors Smithy's find-semantics make
/// redundant and which AWS does not print.
fn unanchored(pattern: &str) -> &str {
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret.strip_suffix('$').unwrap_or(without_caret)
}

/// The response this service answers a request it refuses before `O`'s handler runs,
/// or `None` when the request reaches that handler.
///
/// `None` also when the refusal is not this shape's to state (a tag map over its
/// modelled maximum) or when the modelled error cannot be serialized: both leave the
/// framework's own refusal path to answer, worded by `super`'s `override_refusal` from
/// the same helpers, rather than answering a 400 with nothing in it.
pub(super) fn modeled_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse> {
    let refused = refusal_for::<O>(protocol, req)?;
    protocol
        .serialize_error(
            &refused,
            &SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

/// The modelled `ValidationException` that refuses `req` before `O`'s handler runs, or
/// `None` when nothing does.
///
/// The three classes, in the order a request meets them:
///
/// * a value the member's modelled TYPE could not be read from — the type refused it,
///   so there is no constraint to name and the reason is [`REASON_CANNOT_PARSE`];
/// * a `@required` member absent, found while READING the request;
/// * a value that was read and then fell outside a declared constraint.
///
/// The last two are the same refusal to a caller — a field that did not validate — so
/// they are stated the same way, with [`crate::state::VALIDATION_REASON_FIELD_VALIDATION_FAILED`]
/// and a `fieldList` naming each member. Every class comes from the framework's own
/// machinery (the protocol's deserializer and `constraints::validate` over the built
/// input) over the same input schema, so this cannot find a different refusal from the
/// walk that runs behind it. A body the codec cannot OPEN at all is not a refusal of a
/// value and is left to the framework's own rendering.
fn refusal_for<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<ValidationException> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => fields_that_did_not_validate(&constraints::validate(&input, O::INPUT_SCHEMA)),
        Err(SerdeError::MissingMember { member_name }) => {
            fields_that_did_not_validate(&[Violation {
                path: member_name,
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            }])
        }
        // The body was open and read as this protocol, but a value would not go into
        // the member it is bound to. A catch-all because `SerdeError` is
        // `#[non_exhaustive]`: a codec failure this service does not name must still
        // carry a reason rather than fall through to a body without one.
        Err(unreadable) => Some(ValidationException {
            message: unreadable.to_string(),
            reason: REASON_CANNOT_PARSE.to_string(),
            field_list: None,
        }),
    }
}

/// `violations` as the modelled `ValidationException` for fields that did not
/// validate, or `None` when there is none to state — an empty set, and the two tag
/// refusals [`refused_for`] answers as a bad request body instead.
///
/// Reachable from the rest of the crate because a MODELLED constraint this walk
/// cannot reach — `@uniqueItems` over a list whose elements are structures, which
/// the walk records no rendering for — is refused by the handler that owns the
/// member, and that refusal has to read exactly as this one does.
pub(crate) fn fields_that_did_not_validate(
    violations: &[Violation],
) -> Option<ValidationException> {
    if violations.is_empty()
        || violations.iter().any(is_tag_map_overflow)
        || violations.iter().any(is_outside_the_tag_character_set)
    {
        return None;
    }
    Some(ValidationException {
        message: validation_envelope(violations),
        reason: crate::state::VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: field_list(violations),
    })
}

/// One `fieldList` entry per violated member: the member's path from the request
/// root, and the requirement it failed. `None` when no violation names a member, so
/// the optional list is absent rather than empty.
fn field_list(violations: &[Violation]) -> Option<Vec<ValidationExceptionField>> {
    let fields: Vec<ValidationExceptionField> = violations
        .iter()
        .filter(|violation| !violation.path.is_empty())
        .map(|violation| ValidationExceptionField {
            name: violation.path.clone(),
            // `name` and `message` are both `@required` on the field shape, so a
            // class this service has no wording for still carries the framework's
            // own clause rather than an empty sentence.
            message: requirement_for(violation).unwrap_or_else(|| violation.message()),
        })
        .collect();
    (!fields.is_empty()).then_some(fields)
}

#[cfg(test)]
mod tests {
    //! The wording of a refusal and the members it fills. What reaches the wire, for
    //! which request, is pinned over the router in
    //! `tests/all/boundary_validation_exception.rs`.

    use super::*;

    /// A violation of `kind` on `path`.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: None,
            sensitive: false,
        }
    }

    /// Every constraint class fills `reason` and names its member in `fieldList`,
    /// with the requirement as the clause states it.
    #[test]
    fn every_constraint_class_names_its_member_and_a_reason() {
        let classes = [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(200),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("100".to_string()),
            ViolationKind::Pattern("^[a-z0-9]{26}$".to_string()),
            ViolationKind::EnumValue(vec!["JSON".to_string()]),
            ViolationKind::UniqueItems,
        ];
        for kind in classes {
            let refused = fields_that_did_not_validate(&[violation("identifier", kind.clone())])
                .unwrap_or_else(|| panic!("case {kind:?}: must be refused"));
            assert_eq!(
                refused.reason,
                crate::state::VALIDATION_REASON_FIELD_VALIDATION_FAILED,
                "case {kind:?}: expected the field-validation reason, got {:?}",
                refused.reason
            );
            let fields = refused.field_list.unwrap_or_default();
            assert_eq!(
                fields.len(),
                1,
                "case {kind:?}: expected one field, got {fields:?}"
            );
            assert_eq!(
                fields[0].name, "identifier",
                "case {kind:?}: expected the member's name, got {:?}",
                fields[0].name
            );
            assert!(
                !fields[0].message.is_empty(),
                "case {kind:?}: the field message is @required, got {:?}",
                fields[0].message
            );
            assert!(
                refused.message.contains(&fields[0].message),
                "case {kind:?}: the envelope must state the same requirement as the \
                 field, got {:?} and {:?}",
                refused.message,
                fields[0].message
            );
        }
    }

    /// A tag map over its modelled maximum is not this shape's refusal to state.
    #[test]
    fn a_tag_map_overflow_is_left_to_the_bad_request_body() {
        let overflow = violation("tags", ViolationKind::MaxLength(TAG_MAP_MAX_ENTRIES));
        assert!(
            fields_that_did_not_validate(std::slice::from_ref(&overflow)).is_none(),
            "case a tag map over its maximum: must not be a ValidationException"
        );
        assert_eq!(
            refused_for(&[overflow]).code,
            BAD_REQUEST_CODE,
            "case a tag map over its maximum: must be the bad-request body code"
        );
    }

    /// A tag key or value outside its character set is the same class of bad body,
    /// stated with the sentence that class carries — wherever under the two tag
    /// members the walk reports it, and not for another member's pattern.
    #[test]
    fn a_tag_outside_its_character_set_is_the_bad_request_body_sentence() {
        let pattern = ViolationKind::Pattern("^[a-zA-Z0-9_.:/=+\\-@ ]*$".to_string());
        for path in ["tags.bad!key", "tags", "tagKeys.1.member"] {
            let outside = violation(path, pattern.clone());
            assert!(
                fields_that_did_not_validate(std::slice::from_ref(&outside)).is_none(),
                "case {path:?}: must not be a ValidationException"
            );
            let refused = refused_for(std::slice::from_ref(&outside));
            assert_eq!(
                refused.code, BAD_REQUEST_CODE,
                "case {path:?}: expected the bad-request body code, got {:?}",
                refused.code
            );
            assert_eq!(
                refused.message,
                crate::state::TAG_CHARACTER_SET_MESSAGE,
                "case {path:?}: expected the character-set sentence, got {:?}",
                refused.message
            );
        }
        let elsewhere = violation("identifier", pattern);
        assert_eq!(
            refused_for(std::slice::from_ref(&elsewhere)).code,
            VALIDATION_EXCEPTION_CODE,
            "case a pattern on another member: must stay a ValidationException"
        );
    }

    /// Several violations are counted in one envelope and each names its member.
    #[test]
    fn several_violations_are_stated_together() {
        let refused = fields_that_did_not_validate(&[
            violation("identifier", ViolationKind::Pattern("^[a-z0-9]{26}$".to_string())),
            violation("maxResults", ViolationKind::MinValue("1".to_string())),
        ])
        .expect("two violations are a refusal");
        assert!(
            refused.message.starts_with("2 validation errors detected: "),
            "expected the counted envelope, got {:?}",
            refused.message
        );
        let names: Vec<String> = refused
            .field_list
            .unwrap_or_default()
            .into_iter()
            .map(|field| field.name)
            .collect();
        assert_eq!(
            names,
            vec!["identifier".to_string(), "maxResults".to_string()],
            "every violated member must be named, got {names:?}"
        );
    }

    /// A `@pattern` is quoted as modelled minus its anchors, in the clause and in
    /// the field entry alike.
    #[test]
    fn a_pattern_is_quoted_unanchored() {
        let refused = fields_that_did_not_validate(&[violation(
            "identifier",
            ViolationKind::Pattern("^[a-z0-9]{26}$".to_string()),
        )])
        .expect("a pattern violation is a refusal");
        assert!(
            !refused.message.contains('^') && !refused.message.contains('$'),
            "expected the pattern unanchored, got {:?}",
            refused.message
        );
        let fields = refused.field_list.unwrap_or_default();
        assert!(
            fields[0].message.ends_with("[a-z0-9]{26}"),
            "expected the field to quote the pattern unanchored, got {:?}",
            fields[0].message
        );
    }
}
