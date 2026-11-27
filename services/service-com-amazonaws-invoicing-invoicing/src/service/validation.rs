// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the boundary validator refused, in the modelled
//! `ValidationException`.
//!
//! The framework walks every modelled input constraint and hands the result over
//! STRUCTURALLY — one [`Violation`] per offending value, carrying the path it sits at,
//! what it failed and what it held — and leaves the wording to the service, because no
//! model records it. This module is that wording for every modelled input constraint of
//! this service: ONE formatter, reached by every refusal this service answers, so no
//! operation can word a refusal its own way.
//!
//! The rules it implements, as AWS states a constraint violation:
//!
//! * a clause names the offending MEMBER PATH and the CONSTRAINT that member violated,
//!   and NOTHING of the value the caller sent: `Value at '<path>' failed to satisfy
//!   constraint: <requirement>`. An absent member is the one exception, reported as the
//!   `null` it was — a fact of the request's shape rather than a rendering of a value.
//!   Echoing the value back is wrong whatever the member: the clause's job is to say
//!   what the request must satisfy, which the rejected value does not help with, and
//!   anything that surfaces or persists the message would otherwise reproduce caller
//!   input AWS deliberately omits;
//! * a `fieldList` ENTRY's own message states that same requirement about the VALUE the
//!   entry refused and names no member path at all — `Value [redacted] failed to satisfy
//!   constraint: <requirement>` — because the entry already identifies its member in its
//!   own `name` member, so the message has nothing left to identify and the withheld
//!   value is what stands at the head of the sentence, as the literal [`REDACTED_VALUE`]
//!   marker. An absent member keeps the `null` there for the same reason as above: there
//!   was no value to withhold (see [`field_clause`]);
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors a
//!   model spells it with to demand a whole-string match: a caller that lifts the
//!   advertised expression out of the message and re-applies it locally must get the
//!   expression the service advertises;
//! * the clauses are counted and joined into the AWS envelope — `1 validation error
//!   detected: <clause>` for one, `N validation errors detected: <clause>; <clause>`
//!   for several — in the order the walk found them;
//! * the envelope is written into a member the model BOUNDS, so a refusal states as
//!   many WHOLE clauses as that bound leaves room for and the count it states is the
//!   number of clauses it states — a message is never longer than the member it travels
//!   in can carry (see [`MAX_MESSAGE_LENGTH`] and [`bounded_message`]).
//!
//! The last of those is a rule about the body this service WRITES rather than about the
//! request, so it holds of every value any refusal of this service reports, not only of
//! its prose: an ARN reported back to a client travels in a member the model bounds too,
//! and [`reportable_resource_arn`] is that rule for an identifier, which — unlike a
//! sentence — cannot be cut down to the bound and still name what was asked about.
//!
//! Both halves of a refusal body are built from the same violation and state the same
//! REQUIREMENT for it — the `message` envelope and each `fieldList` entry's own `message`
//! (see [`field_validation_failed`]) reach it through the one [`requirement`], so the two
//! cannot word one constraint two ways. They differ only in how the member is identified:
//! the envelope is prose about a whole request, so it names the member path, while an
//! entry is a structure whose `name` names it.
//!
//! Nothing here is keyed to an operation, a member name or a value: the rule is applied
//! to whatever [`Violation`] the walk produces, so it holds for every constrained member
//! of every operation of this service — `InvoiceReceiver`'s `@pattern`, `Name`'s length
//! and pattern, `MaxResults`' range, the ARN patterns, a member nested in `Rule` or in a
//! `ResourceTag` alike.
//!
//! A violation class this module has no wording for declines the WHOLE refusal, which
//! keeps the framework's own sentence rather than inventing one: [`ViolationKind`] is
//! `#[non_exhaustive]`, so a class can be added upstream.
//!
//! The redaction marker is the one part of this wording a value is not simply left out
//! of: a `fieldList` entry's message is a sentence ABOUT the value, so leaving the value
//! out of it altogether would leave the sentence saying nothing about what was refused.
//! The marker says the value was withheld deliberately, which is what support tooling
//! reads it as, and it is the same text whatever the member and whatever the value — it
//! is never derived from the request.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

use crate::ops::types::{ValidationException, ValidationExceptionField};

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, which every operation declares and which the model
/// gives `@error("client")` with `httpError 400` (`smithy_query(query="shape",
/// shape="ValidationException")`). Restated here because `override_refusal` replaces
/// the code and the message TOGETHER.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `ValidationExceptionReason$fieldValidationFailed` — a named member of the request
/// carries a value the service does not accept. One of the values the modelled
/// `ValidationExceptionReason` enum admits
/// ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]).
pub(crate) const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// How many violations one refusal states. Each one is rendered TWICE into the body (a
/// clause of `message` and a `fieldList` entry), so an unbounded count turns a small
/// request into an arbitrarily large refusal. The bound is the boundary validator's own
/// (`MAX_VIOLATIONS` in `aws_smithy_schema_server::constraints`), so the two refusal
/// paths read alike.
const MAX_VIOLATIONS: usize = 100;

/// The longest text a message this service answers with may carry.
///
/// Every `message` member of every error shape of this model — and a
/// `ValidationExceptionField`'s own `name` and `message` beside them — targets
/// `BasicString`, which the model gives `@length(min: 0, max: 1024)`
/// (`smithy_query(query="shape", shape="ValidationException,BasicString")`). A text past
/// it is a value the model says that member never holds, so it is the service's to keep
/// inside the bound, whatever the request that produced it.
///
/// `@length` on a string counts CHARACTERS, which is what the boundary validator counts
/// for a request's own members too.
pub(crate) const MAX_MESSAGE_LENGTH: usize = 1024;

/// `text` as it can travel in a member the model bounds at [`MAX_MESSAGE_LENGTH`]: the
/// text itself when it fits, else its first [`MAX_MESSAGE_LENGTH`] characters.
///
/// The cut is on a CHARACTER boundary, so what a client reads is still the text's own
/// prefix rather than a broken encoding.
pub(crate) fn bounded_message(text: String) -> String {
    match text.char_indices().nth(MAX_MESSAGE_LENGTH) {
        Some((end, _)) => text[..end].to_string(),
        None => text,
    }
}

/// The lengths an ARN this service REPORTS may run to: the SHORTEST one such a member
/// holds, with [`MAX_REPORTED_ARN_LENGTH`] the longest.
///
/// Every member of every error shape of this model that reports the resource an operation
/// was asked about — `ResourceNotFoundException$resourceName`, and the same member of
/// `AccessDeniedException` and of `ValidationException` — targets `InvoiceUnitArnString`,
/// which the model gives `@length(min: 1, max: 256)` (`smithy_query(query="shape",
/// shape="ResourceNotFoundException,InvoiceUnitArnString")`). An ARN outside that is a
/// value the model says those members never hold, whatever the request supplied: the tag
/// plane's `ResourceArn` is a `TagrisArn`, `@length(min: 20, max: 2048)`, so one legal
/// request can name a resource eight times longer than the member that would report it.
///
/// As with [`MAX_MESSAGE_LENGTH`], `@length` on a string counts CHARACTERS.
pub(crate) const MIN_REPORTED_ARN_LENGTH: usize = 1;

/// The longest ARN one of those members holds — the other end of
/// [`MIN_REPORTED_ARN_LENGTH`]'s range, from the same `@length`.
pub(crate) const MAX_REPORTED_ARN_LENGTH: usize = 256;

/// `arn` as it can travel in a member the model types `InvoiceUnitArnString`: the ARN
/// itself when its length is one the member admits, else NOTHING — the member is reported
/// ABSENT.
///
/// An ARN is an IDENTIFIER rather than prose, so this cannot do what [`bounded_message`]
/// does to an over-long sentence: a cut-down ARN names a different resource, or none, and
/// a client reading it mechanically would be answered something the request never asked
/// about. The member is optional, so leaving it out — "the resource is not named back
/// here" — is an answer the model admits where an over-long ARN is not, and it is the
/// choice this service already makes for an identifier that member's `@pattern` does not
/// admit (see [`crate::state::no_such_invoice`]).
pub(crate) fn reportable_resource_arn(arn: &str) -> Option<String> {
    let length = arn.chars().count();
    (MIN_REPORTED_ARN_LENGTH..=MAX_REPORTED_ARN_LENGTH)
        .contains(&length)
        .then(|| arn.to_string())
}

/// The token a `fieldList` entry's message states where the value it refused would go.
///
/// The value itself is never reproduced (see this module's docs), and an entry's message
/// is a sentence about that value, so this marker is what stands in its place — a fixed
/// text, the same for every member and every value, that says the value was withheld
/// rather than that there was none.
const REDACTED_VALUE: &str = "[redacted]";

/// The sentence a refusal falls back to when no violation has a wording here: the
/// documentation of the modelled shape itself (`smithy_query(query="shape",
/// shape="ValidationException")`).
const VALIDATION_EXCEPTION_DOC: &str =
    "The input fails to satisfy the constraints specified by an Amazon Web Services service.";

/// A `@pattern` expression without the anchors a model spells it with.
///
/// A `$` preceded by an odd number of backslashes is an ESCAPED dollar sign — a literal
/// character of the expression rather than the end anchor — so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let escapes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if escapes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The requirement a violation states, worded as AWS words it. `None` for a class this
/// service has no wording for.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let text = match kind {
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
    };
    Some(text)
}

/// One violation as the clause that goes into the envelope, or `None` for a class this
/// service has no wording for.
///
/// The rejected value is never rendered. An absent member is reported as the `null` it
/// was, which is the shape of the request rather than a value the caller sent.
pub(crate) fn clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let subject = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Value null",
        _ => "Value",
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// One violation as the `message` of the `fieldList` entry that reports it: the VALUE the
/// entry refused and the requirement that value failed, and no member path — the entry
/// names its member in its own `name` member, so its message identifies it nowhere a
/// second time. `None` for a class this service has no wording for, as [`clause`].
///
/// The rejected value is never rendered here either: a value the request DID carry is
/// stated as the [`REDACTED_VALUE`] marker, whatever the member and whatever the value,
/// and an absent member — which had no value to withhold — keeps the `null` the request's
/// shape held.
pub(crate) fn field_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let value = match &violation.kind {
        ViolationKind::MissingRequiredMember => "null",
        _ => REDACTED_VALUE,
    };
    Some(format!(
        "Value {value} failed to satisfy constraint: {requirement}"
    ))
}

/// The AWS envelope over `clauses`, which COUNTS the clauses it states: `None` for no
/// clause at all, which is not a refusal.
fn joined(clauses: &[String]) -> Option<String> {
    match clauses {
        [] => None,
        [only] => Some(format!("1 validation error detected: {only}")),
        several => Some(format!(
            "{} validation errors detected: {}",
            several.len(),
            several.join("; ")
        )),
    }
}

/// How many of `clauses` one envelope states: as many as [`MAX_MESSAGE_LENGTH`] leaves
/// room for, and never fewer than one.
///
/// A clause is stated WHOLE or not at all, so the last one that fits is where the
/// envelope stops; joining one more clause only ever makes the envelope longer, so the
/// first count that does not fit is the first count to stop at. One clause is always
/// stated: a refusal names the member it refused even when that one clause is longer
/// than the whole member the envelope travels in, where [`bounded_message`] is what
/// keeps it inside the bound.
fn stated_count(clauses: &[String]) -> usize {
    let mut stated = 1;
    for count in 2..=clauses.len() {
        match joined(&clauses[..count]) {
            Some(text) if text.chars().count() <= MAX_MESSAGE_LENGTH => stated = count,
            _ => break,
        }
    }
    stated
}

/// The violations one refusal STATES and the `message` that states them, which both
/// halves of the body are then built from.
///
/// Two bounds apply: at most [`MAX_VIOLATIONS`] violations, and at most as many clauses
/// as [`MAX_MESSAGE_LENGTH`] leaves room for. `None` declines the whole refusal — see
/// [`clause`].
fn stated(violations: &[Violation]) -> Option<(&[Violation], String)> {
    let violations = &violations[..violations.len().min(MAX_VIOLATIONS)];
    let clauses = violations
        .iter()
        .map(clause)
        .collect::<Option<Vec<String>>>()?;
    if clauses.is_empty() {
        // No violation at all is not a refusal this module words.
        return None;
    }
    let count = stated_count(&clauses);
    let message = bounded_message(joined(&clauses[..count])?);
    Some((&violations[..count], message))
}

/// The `message` of a refused request: the AWS envelope over one clause per violation,
/// in the order the walk found them, inside the bound of the member it travels in.
/// `None` declines the whole refusal — see [`clause`] and [`stated`].
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    stated(violations).map(|(_, message)| message)
}

/// The 400 `ValidationException` that states `violations` — a member constraint the
/// request failed — in the shape's own modelled members.
///
/// ONE rendering for every refused member, wherever the refusal is found: `message` is
/// the `N validation error(s) detected: …` envelope, `reason` is
/// [`REASON_FIELD_VALIDATION_FAILED`], and `fieldList` carries one entry per violation —
/// `name` the dotted member path, `message` that violation's own [`field_clause`] naming
/// the constraint it failed. A client can therefore branch on `reason` and report which
/// member was rejected and why, instead of parsing the prose.
///
/// Both halves state the SAME violations: the ones [`stated`] leaves after the count and
/// the message bounds, so the count the envelope states is the number of entries the
/// field list carries. Every text is held to [`MAX_MESSAGE_LENGTH`], the bound the model
/// gives each of the members it travels in.
///
/// This body is answered by the input validator in FRONT of the service rather than by the
/// plane the request was for (see this module's docs and [`crate::service::FrontDoor`]), so
/// its text reaches the wire under the member name the model spells — the member every
/// recorded refusal of a request carries it in — rather than under the answering plane's
/// ([`crate::service::error_body_members`]).
pub(crate) fn field_validation_failed(violations: &[Violation]) -> ValidationException {
    let capped = &violations[..violations.len().min(MAX_VIOLATIONS)];
    // A violation class with no wording here declines the envelope, which keeps the
    // shape's own documented sentence and states every violation the count bound admits.
    let (violations, message) =
        stated(violations).unwrap_or((capped, VALIDATION_EXCEPTION_DOC.to_string()));
    ValidationException {
        message: Some(message),
        resource_name: None,
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(
            violations
                .iter()
                .map(|violation| ValidationExceptionField {
                    name: bounded_message(violation.path.clone()),
                    // The same requirement the envelope states for this member, about
                    // the value the entry refused: the member is named by `name` just
                    // above, so the message states no path of its own.
                    message: bounded_message(
                        field_clause(violation)
                            .unwrap_or_else(|| VALIDATION_EXCEPTION_DOC.to_string()),
                    ),
                })
                .collect(),
        ),
    }
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a
/// different failure with a different code and status, and nothing about it is this
/// service's to word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(violations)?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor stripping, over the forms a `@pattern` takes. The forms this
    /// service's own model reaches are covered end-to-end by
    /// `tests/all/validation_message_wording.rs`; these pin the ones no input shape of
    /// THIS model spells today, so the rule stays stated for a model that grows one.
    #[test]
    fn anchors_are_stripped_and_an_escaped_dollar_is_kept() {
        for (pattern, expected) in [
            // The form this model spells on `AccountIdString`.
            (r"^\d{12}$", r"\d{12}"),
            (r"^[\S\s]*$", r"[\S\s]*"),
            // Unanchored already, and anchored at one end only.
            (r"[a-z]+", "[a-z]+"),
            (r"^[a-z]+", "[a-z]+"),
            (r"[a-z]+$", "[a-z]+"),
            // An escaped `$` is a literal character of the expression.
            (r"^\$[0-9]+$", r"\$[0-9]+"),
            (r"^\$", r"\$"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            (r"^[a-z]\\$", r"[a-z]\\"),
            (r"^", ""),
        ] {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A clause never reproduces the value, whatever the violation held; an absent
    /// member keeps the `null` the request shape states.
    #[test]
    fn a_clause_states_the_member_and_the_requirement_only() {
        let rejected = "not-an-account-id";
        let cases = [
            (
                Violation {
                    path: "invoiceReceiver".to_string(),
                    kind: ViolationKind::Pattern(r"^\d{12}$".to_string()),
                    value: Some(rejected.to_string()),
                    sensitive: false,
                },
                "Value at 'invoiceReceiver' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: \\d{12}",
            ),
            (
                Violation {
                    path: "name".to_string(),
                    kind: ViolationKind::MaxLength(50),
                    value: Some(rejected.to_string()),
                    sensitive: false,
                },
                "Value at 'name' failed to satisfy constraint: Member must have length \
                 less than or equal to 50",
            ),
            (
                Violation {
                    path: "name".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "Value null at 'name' failed to satisfy constraint: Member must not be null",
            ),
        ];
        for (violation, expected) in cases {
            let actual = clause(&violation).expect("a modelled violation class is worded");
            assert_eq!(
                actual, expected,
                "case {violation:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(rejected),
                "case {violation:?}: the clause must not reproduce the value, got {actual:?}"
            );
        }
    }

    /// A `fieldList` entry's own message states the VALUE and the requirement and no
    /// member path, with the value withheld behind the marker; an absent member keeps
    /// the `null` it was. The classes this model's own input shapes reach are covered
    /// end-to-end by `tests/all/field_list_entry_message.rs`.
    #[test]
    fn a_field_clause_states_the_withheld_value_and_the_requirement_only() {
        let rejected = "not-an-account-id";
        let cases = [
            (
                Violation {
                    path: "invoiceReceiver".to_string(),
                    kind: ViolationKind::Pattern(r"^\d{12}$".to_string()),
                    value: Some(rejected.to_string()),
                    sensitive: false,
                },
                "Value [redacted] failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: \\d{12}",
            ),
            (
                Violation {
                    path: "name".to_string(),
                    kind: ViolationKind::MaxLength(50),
                    value: Some(rejected.to_string()),
                    sensitive: false,
                },
                "Value [redacted] failed to satisfy constraint: Member must have length \
                 less than or equal to 50",
            ),
            (
                Violation {
                    path: "name".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "Value null failed to satisfy constraint: Member must not be null",
            ),
        ];
        for (violation, expected) in cases {
            let actual = field_clause(&violation).expect("a modelled violation class is worded");
            assert_eq!(
                actual, expected,
                "case {violation:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(rejected),
                "case {violation:?}: the entry must not reproduce the value, got {actual:?}"
            );
            assert!(
                !actual.contains(violation.path.as_str()),
                "case {violation:?}: the entry's own name identifies the member, got \
                 {actual:?}"
            );
        }
    }

    /// The envelope counts and joins the clauses the AWS way.
    #[test]
    fn the_envelope_counts_the_clauses() {
        let one = vec![Violation {
            path: "maxResults".to_string(),
            kind: ViolationKind::MaxValue("500".to_string()),
            value: Some("501".to_string()),
            sensitive: false,
        }];
        let two = vec![
            one[0].clone(),
            Violation {
                path: "nextToken".to_string(),
                kind: ViolationKind::MinLength(1),
                value: Some(String::new()),
                sensitive: false,
            },
        ];
        for (violations, expected) in [
            (
                &one,
                "1 validation error detected: Value at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 500",
            ),
            (
                &two,
                "2 validation errors detected: Value at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 500; Value at \
                 'nextToken' failed to satisfy constraint: Member must have length greater \
                 than or equal to 1",
            ),
        ] {
            let actual = envelope(violations).expect("a worded refusal");
            assert_eq!(
                actual, expected,
                "case {violations:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The clauses one envelope states, read back off the envelope: the count it
    /// announces and the clauses themselves.
    fn stated_clauses(message: &str) -> (usize, Vec<&str>) {
        let (count, joined) = message
            .split_once(" validation error detected: ")
            .or_else(|| message.split_once(" validation errors detected: "))
            .unwrap_or_else(|| panic!("an AWS validation envelope, got {message:?}"));
        (
            count.parse().expect("the envelope counts its clauses"),
            joined.split("; ").collect(),
        )
    }

    /// Whatever the walk hands over, the envelope fits the modelled maximum of the
    /// member it travels in, states WHOLE clauses, and counts the clauses it states.
    #[test]
    fn the_envelope_fits_the_modelled_message_bound() {
        // Past `MAX_VIOLATIONS` as well as past the number of clauses that fit, so both
        // bounds are exercised.
        for count in [1usize, 2, 5, 11, 100, 200, 400] {
            let violations: Vec<Violation> = (0..count)
                .map(|index| Violation {
                    path: format!("resourceTags.{}.member.key", index + 1),
                    kind: ViolationKind::MinLength(1),
                    value: Some(String::new()),
                    sensitive: false,
                })
                .collect();
            let message = envelope(&violations).expect("a worded refusal");
            let length = message.chars().count();
            assert!(
                length <= MAX_MESSAGE_LENGTH,
                "case {count} violations: the message must fit {MAX_MESSAGE_LENGTH} \
                 characters, got {length}"
            );
            let (stated, clauses) = stated_clauses(&message);
            assert_eq!(
                stated,
                clauses.len(),
                "case {count} violations: the envelope counts the clauses it states, got \
                 {message:?}"
            );
            assert!(
                stated >= 1 && stated <= count.min(MAX_VIOLATIONS),
                "case {count} violations: expected between 1 and \
                 {} clauses, got {stated}",
                count.min(MAX_VIOLATIONS)
            );
            let expected: Vec<String> = violations
                .iter()
                .map(|violation| clause(violation).expect("a worded violation class"))
                .collect();
            for (position, stated_clause) in clauses.iter().enumerate() {
                assert_eq!(
                    *stated_clause,
                    expected[position].as_str(),
                    "case {count} violations: clause {position} is stated whole and in the \
                     order the walk found it, got {message:?}"
                );
            }
        }
    }

    /// One clause longer than the whole member the envelope travels in is stated as far
    /// as that member reaches, rather than answered past the bound.
    #[test]
    fn one_clause_past_the_bound_is_held_to_it() {
        let violation = Violation {
            path: "name".to_string(),
            kind: ViolationKind::Pattern(format!("^{}$", "a".repeat(2 * MAX_MESSAGE_LENGTH))),
            value: Some("nope".to_string()),
            sensitive: false,
        };
        let message = envelope(std::slice::from_ref(&violation)).expect("a worded refusal");
        let length = message.chars().count();
        assert_eq!(
            length, MAX_MESSAGE_LENGTH,
            "case one over-long clause: the message is stated as far as the member \
             reaches, got {length} characters"
        );
        assert!(
            message.starts_with("1 validation error detected: Value at 'name' failed to "),
            "case one over-long clause: the message is the envelope's own prefix, got \
             {message:?}"
        );
    }
}
