// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request that breaks a modelled constraint.
//!
//! The framework finds the violations (the protocol's deserializer for an absent
//! required member, `constraints::validate` for a value outside a declared bound) and
//! words them in its own conventional envelope, under its own error code. API Gateway
//! states the same facts differently in four ways, all of them properties of the service
//! or of the shape that was violated rather than of one request:
//!
//! 0. **The refusal carries the error code of the layer that made it.** A value outside a
//!    modelled constraint is refused by the layer in FRONT of the operation, under that
//!    layer's own class (`ValidationException`) rather than the operation's modelled
//!    `BadRequestException` — except for the few members this service checks in its own
//!    request handling ahead of that layer, which earn the operation's class AND the one
//!    sentence it words that refusal with; see [`VALIDATION_EXCEPTION`],
//!    [`BAD_REQUEST_EXCEPTION`] and [`SELF_VALIDATED_MEMBERS`].
//! 1. **The member path is rooted at the operation's input shape.** AWS's own model
//!    names an operation's input `<Operation>Input`, and the clause names the member by
//!    its path from that root in lowerCamel — `updateDocumentationVersionInput.
//!    patchOperations.1.member.op`, `putGatewayResponseInput.statusCode` — so a client
//!    can map the clause back to the operation AND the field. The framework's path
//!    starts at the first input member, which names no operation. [`input_root`] is
//!    that root; it is derived per operation, which the recordings prove by showing a
//!    DIFFERENT root for the same member path on `UpdateRestApi`, `UpdateApiKey` and
//!    `UpdateDocumentationVersion`. A member bound to the QUERY STRING is the exception:
//!    it is named by its own member name alone, with no root at all — see
//!    [`binds_to_query`].
//! 2. **A `@pattern` is emitted without the whole-value anchors.** The public Smithy
//!    model writes AWS's implicit whole-value match as an explicit `^…$` pair
//!    (`smithy_query(query="shape", shape="com.amazonaws.apigateway#StatusCode")` gives
//!    `^[1-5]\d\d$`); the service emits the expression itself, `[1-5]\d\d`. The pair is
//!    dropped from the RENDERING only — the check still runs the model's own pattern.
//! 3. **An enum value set is listed in the order the service declares the enum in**,
//!    which is not always the order the public model lists (see [`DECLARED_VALUE_SETS`]).
//!
//! Status (400) is the framework's own choice for a constraint failure and is left
//! alone; the error code and the constructed sentence are this service's.

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, ViolationKind};
use smithy_server_runtime::{Refused, Violation};

/// The error code a value outside a member's modelled constraint is refused under.
///
/// A constraint trait is enforced by the layer in FRONT of the operation, and that layer
/// has one error class of its own: a caller bug, reported at HTTP 400 as
/// `ValidationException`. It is deliberately NOT the operation's modelled
/// `BadRequestException` — also a 400 — which reports a SEMANTIC refusal a handler makes
/// about a well-formed request, so a client branching on the code can tell "this value
/// cannot be sent at all" from "this value cannot be used here".
///
/// A chosen [`Refused`] replaces the code and the message TOGETHER, which is what lets
/// this one seam state both.
pub(super) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code a refusal of a member this service checks ITSELF carries: the
/// client-input error its model declares ("The submitted request is not valid, for
/// example, the input is incomplete or incorrect").
pub(super) const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The input members this service is RECORDED checking in its own request handling,
/// ahead of the generic constraint layer — so a value outside the member's modelled
/// constraint is the OPERATION's semantic refusal ([`BAD_REQUEST_EXCEPTION`]) and not the
/// layer's ([`VALIDATION_EXCEPTION`]), stated in the SENTENCE that operation words it
/// with rather than in the constraint envelope.
///
/// Each entry is `(operation, the violated member's path from the operation's input
/// shape, the sentence the operation answers)`. `CreateAuthorizer`'s `type` is the one
/// such member of this service: its recorded refusal of a value outside `AuthorizerType`
/// carries `BadRequestException` and the member's own sentence, while the recorded
/// refusals of `CreateRestApi.endpointConfiguration.types`,
/// `PutGatewayResponse.statusCode`, `PutIntegrationResponse.contentHandling` and
/// `UpdateVpcLink.patchOperations[].op` — four members of four other operations — carry
/// `ValidationException` and the envelope. The set is therefore a property of the MEMBER
/// and is listed, not derived.
const SELF_VALIDATED_MEMBERS: [(&str, &str, &str); 1] = [(
    "CreateAuthorizer",
    "type",
    crate::ops::create_authorizer::INVALID_AUTHORIZER_TYPE,
)];

/// The enums of this service whose value set the recordings show in an order the public
/// Smithy model does not list it in.
///
/// `PatchOperation$op` targets `com.amazonaws.apigateway#Op`, which
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#Op")` declares as
/// `[add, remove, replace, move, copy, test]`, and `PutRestApiRequest$mode` targets
/// `com.amazonaws.apigateway#PutMode`, declared `[merge, overwrite]`. Every recorded
/// refusal of an invalid `op` — on `UpdateDocumentationVersion`, on `UpdateRestApi` and on
/// `UpdateApiKey`, three different operations — and the recorded refusal of an invalid
/// `mode` list their sets as below instead, as does the recorded refusal of an invalid
/// `type` on `PutIntegration`, whose `com.amazonaws.apigateway#IntegrationType` the public
/// model declares `[HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]`. The order is therefore a
/// property of the ENUM
/// SHAPE (the order the service's own model declares its values in), not of any one
/// request, and is recorded as such: a value set is rendered in the service's declared
/// order, which for every other enum of this service is the order the public model
/// preserves.
const DECLARED_VALUE_SETS: [&[&str]; 3] = [
    &["add", "remove", "move", "test", "replace", "copy"],
    &["overwrite", "merge"],
    &["HTTP", "MOCK", "AWS_PROXY", "HTTP_PROXY", "AWS"],
];

/// This service's wording for the violations the framework found on `operation`'s
/// request, or `None` when there are none to word.
///
/// `input_schema` is the operation's own input schema, which is what says how each
/// violated member TRAVELS — the one request-shaped fact a clause of this service depends
/// on (see [`binds_to_query`]).
pub(super) fn refusal(
    operation: &str,
    input_schema: &'static Schema,
    violations: &[Violation],
) -> Option<Refused> {
    // A member the OPERATION checks itself is its own semantic refusal, and that refusal
    // is one sentence of the operation's own: the constraint envelope reports which
    // constraint a value broke, which is not what such a refusal says.
    if let Some(stated) = stated_by_the_operation(operation, violations) {
        return Some(Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: stated.to_string(),
        });
    }
    let root = input_root(operation);
    let stated: Vec<Violation> = violations
        .iter()
        .map(|violation| as_stated(&root, input_schema, violation))
        .collect();
    // The envelope — `1 validation error detected: <clause>` / `N validation errors
    // detected: <clause>; <clause>` — and each clause's own frame are the framework's,
    // reached through the same `Violation` the walk produced, so a new constraint class
    // cannot arrive here with wording of its own.
    let message = aws_smithy_schema_server::envelope(&stated)?;
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// The sentence `operation` words this refusal with itself, or `None` when the refusal
/// belongs to the constraint layer.
///
/// A request can break more than one constraint at once; the first violated member
/// `operation` checks itself, in the order [`SELF_VALIDATED_MEMBERS`] lists them, is the
/// one stated — the refusal that operation makes before the layer in front of it is
/// reached.
fn stated_by_the_operation(operation: &str, violations: &[Violation]) -> Option<&'static str> {
    SELF_VALIDATED_MEMBERS
        .iter()
        .find(|(owner, member, _)| {
            *owner == operation
                && violations
                    .iter()
                    .any(|violation| violation.path == *member)
        })
        .map(|(_, _, sentence)| *sentence)
}

/// The root segment of every validation path of `operation`: its input shape, named as
/// AWS's own model names it (`<Operation>Input`) and cased as a path segment is.
///
/// The casing comes from [`path::member`], the framework's own spelling of a path
/// segment — the same function the walk applies to every segment BELOW this root — so
/// the root cannot be cased differently from the path it heads.
fn input_root(operation: &str) -> String {
    format!("{}Input", path::member(operation))
}

/// One violation as this service states it: the path rooted at the input shape — unless
/// the member travels in the QUERY STRING, which is named on its own — the constraint
/// rendered the way the service renders it, the value as the framework echoed it (already
/// elided to the framework's cap).
fn as_stated(root: &str, input_schema: &'static Schema, violation: &Violation) -> Violation {
    let path = if binds_to_query(input_schema, &violation.path) {
        violation.path.clone()
    } else {
        format!("{root}.{}", violation.path)
    };
    Violation {
        path,
        kind: as_rendered(&violation.kind),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// Whether `violation_path` names a member `input_schema` carries in the QUERY STRING.
///
/// Read off the member's own `@httpQuery` binding, so the MODEL decides: the clause for
/// such a member names it alone, while a member that travels in the body or in the URI
/// path is named from the operation's input shape — the recorded refusal of
/// `PutRestApi`'s `mode`, a `@httpQuery` member, names `mode`, where the recorded refusals
/// of `PutGatewayResponse`'s `statusCode` (a body member) and `PutIntegrationResponse`'s
/// (a `@httpLabel`) are both rooted. A nested path is placed by its FIRST segment, the
/// top-level member the whole subtree hangs off, exactly as the shared front-door
/// placement does it.
fn binds_to_query(input_schema: &Schema, violation_path: &str) -> bool {
    // `split` always yields at least one item, so this is the whole path when it has no
    // `.` in it.
    let member_name = violation_path.split('.').next().unwrap_or_default();
    input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_name)
            && member.http_query().is_some()
    })
}

/// A violated constraint as this service renders it: a pattern without its whole-value
/// anchors, an enum value set in the service's declared order, and every other class
/// exactly as the framework renders it.
fn as_rendered(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(regex) => ViolationKind::Pattern(unanchored(regex)),
        ViolationKind::EnumValue(values) => ViolationKind::EnumValue(declared_order(values)),
        other => other.clone(),
    }
}

/// `regex` without the whole-value anchors the public model adds to express the match
/// AWS's own pattern makes implicitly.
///
/// MIGRATION NOTE, as `services/common/src/validation.rs` asks of anything that inlines
/// this rule again: that module's docs ("Still to migrate here") name the unanchored
/// `@pattern` rendering as one of three parts of a validation message that are the same
/// for every service and belong there, carried inlined in ~24 service crates today
/// (`grep -rln 'fn unanchored' services/*/src`). This is a 25th copy, pinned by this
/// service's own recordings, and is to be DELETED by the change that owns that
/// migration rather than copied again. Which values a value set lists, and in which
/// order, stays here: that is this service's dialect.
///
/// The pair is dropped only when BOTH ends carry it AND the pair WRAPS the whole
/// expression: a `^` alone, or a `$` alone, is an expression anchored at one end — a
/// different expression, not an anchored spelling of this one — and in an alternation
/// (`^a$|^b$`, `^a|b$`) the outer `^` and `$` govern only their own branch, so dropping
/// them would state an expression the service never evaluated. Both are emitted
/// verbatim. A `$` that is ESCAPED is a literal dollar, not an anchor, so it stays.
fn unanchored(regex: &str) -> String {
    let Some(body) = regex.strip_prefix('^') else {
        return regex.to_string();
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) && wraps_whole_expression(inner) => {
            inner.to_string()
        }
        _ => regex.to_string(),
    }
}

/// Whether a `^`/`$` pair around `text` anchors the WHOLE expression, which is what a
/// whole-value pattern's pair does.
///
/// It does not when `text` carries an anchor of its own (so the outer pair belongs to
/// the branches of an alternation, `^a$|^b$`), nor when `text` alternates at the top
/// level (`^a|b$`, where the `^` governs only the first branch and the `$` only the
/// last). A `^`, `$` or `|` inside a character class is a literal or a negation marker
/// rather than an anchor or an alternation, and one inside a group alternates only
/// within that group (`^(a|b)$` IS wrapped), so the scan tracks class and group
/// context. Escapes are honoured throughout.
fn wraps_whole_expression(text: &str) -> bool {
    let mut escaped = false;
    let mut in_class = false;
    let mut depth = 0_usize;
    for character in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            _ if in_class => {}
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '^' | '$' => return false,
            '|' if depth == 0 => return false,
            _ => {}
        }
    }
    true
}

/// Whether `text` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// `values` in the order the service declares the enum they came from.
///
/// A value set is identified by the values it holds (a runtime `Violation` carries the
/// set, not the shape it came from), so the enums this service declares in an order the
/// public model does not list are recognised by their own value sets. Every other value
/// set is already in its declared order and is passed through.
fn declared_order(values: &[String]) -> Vec<String> {
    for declared in DECLARED_VALUE_SETS {
        if is_set_of(values, declared) {
            return declared.iter().map(|value| (*value).to_string()).collect();
        }
    }
    values.to_vec()
}

/// Whether `values` holds exactly the values `expected` does, in any order.
fn is_set_of(values: &[String], expected: &[&str]) -> bool {
    values.len() == expected.len()
        && expected
            .iter()
            .all(|wanted| values.iter().any(|value| value == wanted))
}

#[cfg(test)]
mod tests {
    //! The three renderings this module owns, each over a set of inputs rather than the
    //! one recorded case. The end-to-end wording is pinned on the wire in
    //! `tests/all/validation_message_rules.rs`.

    use super::*;

    /// The input schema of the operation a row names: what says how each member of it
    /// travels, which is the one request-shaped fact [`refusal`] reads.
    fn schema_of(operation: &str) -> &'static Schema {
        match operation {
            "CreateAuthorizer" => crate::ops::types::CREATE_AUTHORIZER_INPUT_SCHEMA,
            "CreateDocumentationPart" => crate::ops::types::CREATE_DOCUMENTATION_PART_INPUT_SCHEMA,
            "CreateRestApi" => crate::ops::types::CREATE_REST_API_INPUT_SCHEMA,
            "GetRestApi" => crate::ops::types::GET_REST_API_INPUT_SCHEMA,
            "PutIntegrationResponse" => crate::ops::types::PUT_INTEGRATION_RESPONSE_INPUT_SCHEMA,
            "PutRestApi" => crate::ops::types::PUT_REST_API_INPUT_SCHEMA,
            "UpdateRestApi" => crate::ops::types::UPDATE_REST_API_INPUT_SCHEMA,
            other => panic!("no schema is wired for {other:?}"),
        }
    }

    /// The refusal `operation` states for `violations`, through that operation's own
    /// input schema.
    fn stated_refusal(operation: &str, violations: &[Violation]) -> Option<Refused> {
        refusal(operation, schema_of(operation), violations)
    }

    /// A path is rooted at the operation's own input shape, for every operation name
    /// shape this service's model holds.
    #[test]
    fn a_validation_path_is_rooted_at_the_operations_input_shape() {
        // (operation, expected root) — three the recordings cover plus operations whose
        // names differ in shape (one word, an acronym, a trailing plural).
        let rows: [(&str, &str); 6] = [
            ("UpdateDocumentationVersion", "updateDocumentationVersionInput"),
            ("UpdateRestApi", "updateRestApiInput"),
            ("UpdateApiKey", "updateApiKeyInput"),
            ("PutGatewayResponse", "putGatewayResponseInput"),
            ("GetSdkTypes", "getSdkTypesInput"),
            ("TestInvokeAuthorizer", "testInvokeAuthorizerInput"),
        ];

        for (operation, expected) in rows {
            let actual = input_root(operation);
            assert_eq!(
                actual, expected,
                "case {operation:?}: expected the root {expected:?}, got {actual:?}"
            );
        }
    }

    /// A pattern loses the whole-value anchor PAIR and nothing else.
    #[test]
    fn a_pattern_is_rendered_without_its_whole_value_anchors() {
        // (case, modelled pattern, rendered pattern)
        let rows: [(&str, &str, &str); 12] = [
            // Source: `smithy_query(query="shape",
            // shape="com.amazonaws.apigateway#StatusCode")`.
            ("the modelled status code", r"^[1-5]\d\d$", r"[1-5]\d\d"),
            ("an anchored character class", "^[a-z]+$", "[a-z]+"),
            ("anchored at the start only", "^arn:aws:", "^arn:aws:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("an escaped trailing dollar", r"^[a-z]+\$$", r"[a-z]+\$"),
            ("a fully anchored alternation", "^a$|^b$", "^a$|^b$"),
            ("a half-anchored alternation", "^a|b$", "^a|b$"),
            ("an alternation inside a group", "^(a|b)$", "(a|b)"),
            (
                "a class holding a literal caret",
                r"^[\^a-z]+$",
                r"[\^a-z]+",
            ),
            ("a negated character class", r"^[^/]+$", r"[^/]+"),
            ("a class holding a literal dollar", "^[a-z$]+$", "[a-z$]+"),
        ];

        for (case, modelled, expected) in rows {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?} ({modelled}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The order each enum of [`DECLARED_VALUE_SETS`] is listed in, for the test rows
    /// below.
    const OP_VALUE_SET: &[&str] = DECLARED_VALUE_SETS[0];
    const PUT_MODE_VALUE_SET: &[&str] = DECLARED_VALUE_SETS[1];
    const INTEGRATION_TYPE_VALUE_SET: &[&str] = DECLARED_VALUE_SETS[2];

    /// A value set is listed in the service's declared order: the recorded order for
    /// `Op`, for `PutMode` and for `IntegrationType`, the model's own order for every
    /// other enum.
    #[test]
    fn an_enum_value_set_is_listed_in_the_services_declared_order() {
        // (case, the set as the model declares it, the set as the service lists it)
        let rows: [(&str, &[&str], &[&str]); 8] = [
            (
                "Op, in the model's declaration order",
                &["add", "remove", "replace", "move", "copy", "test"],
                OP_VALUE_SET,
            ),
            ("Op, already in the service's order", OP_VALUE_SET, OP_VALUE_SET),
            // Source: `smithy_query(query="shape",
            // shape="com.amazonaws.apigateway#PutMode")` declares `[merge, overwrite]`.
            (
                "PutMode, in the model's declaration order",
                &["merge", "overwrite"],
                PUT_MODE_VALUE_SET,
            ),
            (
                "PutMode, already in the service's order",
                PUT_MODE_VALUE_SET,
                PUT_MODE_VALUE_SET,
            ),
            // Source: `smithy_query(query="shape",
            // shape="com.amazonaws.apigateway#ApiKeySourceType")`.
            (
                "ApiKeySourceType",
                &["HEADER", "AUTHORIZER"],
                &["HEADER", "AUTHORIZER"],
            ),
            // A set that merely overlaps `Op` keeps its own order and its own members.
            (
                "a value set holding some of Op's values",
                &["add", "remove"],
                &["add", "remove"],
            ),
            // Source: `smithy_query(query="shape",
            // shape="com.amazonaws.apigateway#IntegrationType")` declares
            // `[HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]`.
            (
                "IntegrationType, in the model's declaration order",
                &["HTTP", "AWS", "MOCK", "HTTP_PROXY", "AWS_PROXY"],
                INTEGRATION_TYPE_VALUE_SET,
            ),
            (
                "IntegrationType, already in the service's order",
                INTEGRATION_TYPE_VALUE_SET,
                INTEGRATION_TYPE_VALUE_SET,
            ),
        ];

        for (case, declared, expected) in rows {
            let given: Vec<String> = declared.iter().map(|v| (*v).to_string()).collect();
            let actual = declared_order(&given);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every constraint class the framework can report is stated with the rooted path,
    /// and only the two renderings above are changed.
    #[test]
    fn every_constraint_class_is_stated_with_the_rooted_path() {
        // (case, the violation the framework found, the clause this service states)
        let rows: [(&str, Violation, &str); 4] = [
            (
                "an enum value set",
                Violation {
                    path: "patchOperations.1.member.op".to_string(),
                    kind: ViolationKind::EnumValue(
                        ["add", "remove", "replace", "move", "copy", "test"]
                            .iter()
                            .map(|v| (*v).to_string())
                            .collect(),
                    ),
                    value: Some("invalid_op".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'invalid_op' at \
                 'updateRestApiInput.patchOperations.1.member.op' failed to satisfy \
                 constraint: Member must satisfy enum value set: \
                 [add, remove, move, test, replace, copy]",
            ),
            (
                "a pattern",
                Violation {
                    path: "statusCode".to_string(),
                    kind: ViolationKind::Pattern(r"^[1-5]\d\d$".to_string()),
                    value: Some("999".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value '999' at \
                 'updateRestApiInput.statusCode' failed to satisfy constraint: Member \
                 must satisfy regular expression pattern: [1-5]\\d\\d",
            ),
            (
                "an absent required member",
                Violation {
                    path: "name".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "1 validation error detected: Value null at 'updateRestApiInput.name' \
                 failed to satisfy constraint: Member must not be null",
            ),
            (
                "a length bound",
                Violation {
                    path: "tags.someKey.member".to_string(),
                    kind: ViolationKind::MaxLength(128),
                    value: Some("x".to_string()),
                    sensitive: false,
                },
                "1 validation error detected: Value 'x' at \
                 'updateRestApiInput.tags.someKey.member' failed to satisfy constraint: \
                 Member must have length less than or equal to 128",
            ),
        ];

        for (case, found, expected) in rows {
            let stated = stated_refusal("UpdateRestApi", std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert_eq!(
                stated.code, VALIDATION_EXCEPTION,
                "case {case:?}: expected the code {VALIDATION_EXCEPTION}, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// A violated member this service checks ITSELF is the operation's own semantic
    /// refusal; every other member of the same operation, and the same member on another
    /// operation, stay on the constraint layer's class.
    #[test]
    fn only_a_self_validated_member_is_refused_as_the_operations_own_bad_request() {
        // (case, operation, the violated member's path, the expected code)
        let rows: [(&str, &str, &str, &str); 5] = [
            (
                "the one member this service checks itself",
                "CreateAuthorizer",
                "type",
                BAD_REQUEST_EXCEPTION,
            ),
            (
                "another member of the same operation",
                "CreateAuthorizer",
                "name",
                VALIDATION_EXCEPTION,
            ),
            (
                "the same member name on another operation",
                "CreateDocumentationPart",
                "type",
                VALIDATION_EXCEPTION,
            ),
            (
                "a pattern on a URI label",
                "PutIntegrationResponse",
                "statusCode",
                VALIDATION_EXCEPTION,
            ),
            (
                "a nested enum member",
                "CreateRestApi",
                "endpointConfiguration.types.0.member",
                VALIDATION_EXCEPTION,
            ),
        ];

        for (case, operation, path, expected) in rows {
            let found = Violation {
                path: path.to_string(),
                kind: ViolationKind::EnumValue(vec!["ONE".to_string(), "TWO".to_string()]),
                value: Some("THREE".to_string()),
                sensitive: false,
            };
            let stated = stated_refusal(operation, std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert_eq!(
                stated.code, expected,
                "case {case:?}: expected the code {expected}, got {:?}",
                stated.code
            );
        }
    }

    /// Nothing to word is no refusal: a request that breaks no constraint is not
    /// answered here.
    #[test]
    fn no_violations_is_no_refusal() {
        let stated = stated_refusal("GetRestApi", &[]);
        assert!(
            stated.is_none(),
            "case no violations: expected None, got {stated:?}"
        );
    }

    /// A member this service checks ITSELF is stated in the operation's own sentence,
    /// whatever the constraint the value broke and whatever else the request broke with
    /// it: the envelope reports a constraint, which is not what that refusal says.
    #[test]
    fn a_self_validated_member_is_stated_in_the_operations_own_sentence() {
        let expected = crate::ops::create_authorizer::INVALID_AUTHORIZER_TYPE;
        let outside_the_catalog = Violation {
            path: "type".to_string(),
            kind: ViolationKind::EnumValue(
                ["COGNITO_USER_POOLS", "REQUEST", "TOKEN"]
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
            ),
            value: Some("NOT_A_REAL_TYPE".to_string()),
            sensitive: false,
        };
        let absent = Violation {
            path: "type".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let another_member = Violation {
            path: "name".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };

        // (case, the violations the framework found)
        let rows: [(&str, Vec<Violation>); 3] = [
            (
                "a value outside the catalog",
                vec![outside_the_catalog.clone()],
            ),
            ("the member left out", vec![absent]),
            (
                "the member and another one together",
                vec![another_member, outside_the_catalog.clone()],
            ),
        ];

        for (case, found) in rows {
            let stated = stated_refusal("CreateAuthorizer", &found)
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            assert_eq!(
                stated.code, BAD_REQUEST_EXCEPTION,
                "case {case:?}: expected the code {BAD_REQUEST_EXCEPTION}, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }

        // The same member on another operation stays on the envelope, so the sentence is
        // this member's and not the name's.
        let elsewhere = stated_refusal(
            "CreateDocumentationPart",
            std::slice::from_ref(&outside_the_catalog),
        )
        .expect("case another operation: expected a reworded refusal");
        assert_ne!(
            elsewhere.message, expected,
            "case another operation: expected the constraint envelope, got {:?}",
            elsewhere.message
        );
    }

    /// A violated member is named from the operation's input shape unless it travels in
    /// the QUERY STRING, which is named alone.
    #[test]
    fn a_query_bound_member_is_named_without_the_input_shape_root() {
        // (case, operation, the violated member's path, the clause's member path) — the
        // rows span the three ways a member of this service travels: the query string, the
        // URI path and the body, plus a nested path under each.
        let rows: [(&str, &str, &str, &str); 5] = [
            // `PutRestApiRequest$mode` is `@httpQuery("mode")`
            // (`smithy_query(query="operation", operation="PutRestApi")`).
            ("a query member", "PutRestApi", "mode", "mode"),
            // `restApiId` is the `@httpLabel` of the same operation, so one request can
            // show both spellings at once.
            (
                "a URI label of the same operation",
                "PutRestApi",
                "restApiId",
                "putRestApiInput.restApiId",
            ),
            (
                "a body member",
                "CreateRestApi",
                "name",
                "createRestApiInput.name",
            ),
            (
                "a nested body member",
                "CreateRestApi",
                "endpointConfiguration.types.0.member",
                "createRestApiInput.endpointConfiguration.types.0.member",
            ),
            (
                "a member no schema of the operation declares",
                "PutRestApi",
                "sim2realNoSuchMember",
                "putRestApiInput.sim2realNoSuchMember",
            ),
        ];

        for (case, operation, path, expected) in rows {
            let found = Violation {
                path: path.to_string(),
                kind: ViolationKind::EnumValue(
                    ["merge", "overwrite"]
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                ),
                value: Some("append".to_string()),
                sensitive: false,
            };
            let stated = stated_refusal(operation, std::slice::from_ref(&found))
                .unwrap_or_else(|| panic!("case {case:?}: expected a reworded refusal"));
            let clause = format!(
                "1 validation error detected: Value 'append' at '{expected}' failed to \
                 satisfy constraint: Member must satisfy enum value set: \
                 [overwrite, merge]"
            );
            assert_eq!(
                stated.message, clause,
                "case {case:?}: expected {clause:?}, got {:?}",
                stated.message
            );
        }
    }
}
