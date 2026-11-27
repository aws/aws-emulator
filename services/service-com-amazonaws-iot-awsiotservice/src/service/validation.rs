// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! AWS IoT answers a request its validation layer will not accept with the error shape
//! this service's front door names for one — `InvalidRequestException`, the legacy IoT
//! exception family, rather than the generic constraint shape the framework picks — and
//! with AWS's own envelope over one clause per constraint the request broke:
//! `N validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: <requirement>`. The envelope and the clauses are
//! [`::service_common::validation_message::aws_validation_refusal_withholding_violations`]'s
//! — the member paths and every requirement are the framework's, so this service cannot
//! drift from them — and only the error code is this service's own.
//!
//! The other thing chosen here is the VALUE half of a clause, per constraint CLASS:
//!
//! * a member the request did not carry has no value to quote, and AWS states that clause
//!   with none at all — `Value at '<member path>' failed to satisfy constraint: Member must
//!   not be null` — where the framework writes the `null` it read;
//! * a member outside a `@pattern` this service PUBLISHES with the value that missed it
//!   ([`PATTERNS_STATED_WITH_THE_REJECTED_VALUE`]) is stated WITH that value, single-quoted,
//!   beside the expression it had to match; every OTHER member outside its `@pattern` is
//!   stated with no value at all, the clause naming the expression alone — which is what this
//!   service's recorded refusals of a domain-configuration name, of a policy version id and
//!   of a transfer's target account show. Which of the two a member takes is read off the
//!   EXPRESSION, and an expression belongs to the SHAPE the member targets, so it holds for
//!   every operation that takes a member of that shape and wherever in the request the value
//!   was carried;
//! * a member outside either end of its `@length` is stated with no value as well: the
//!   clause names the member and the bound it broke, and the text that was too long or too
//!   short — which is the whole of what the caller sent for that member — is not reflected
//!   back. A value and a length bound say the same thing twice, and this service's recorded
//!   refusals of a member that broke its bound AND its expression together state neither
//!   clause with a value;
//! * a COLLECTION member outside its `@length` is the one length clause that DOES state a
//!   value, and the value is the collection ITSELF — a list as `[…]`, a map as `{…}`. The
//!   framework's walk, which length-checks the member before it descends into the entries,
//!   has only a summary of its SIZE (`<n> items`, `<n> entries`) and no value any caller
//!   sent, so there is nothing of the caller's own to say twice. A summary of size ZERO
//!   determines that collection completely, so the clause states it from the summary alone
//!   ([`::service_common::validation_message::quoting_an_empty_submitted_collection`], where
//!   the rendering lives); a summary of any other size needs the request's own elements,
//!   which this seam is not handed, and keeps the framework's valueless clause;
//! * a member outside either end of its `@range` is stated with no value either: the clause
//!   names the member and the numeric bound it broke — `Value at '<member path>'
//!   failed to satisfy constraint: Member must have value less than or equal to <bound>` —
//!   and the number that fell outside the bound is not reflected back, which is what this
//!   service's recorded refusal of a role alias's credential duration past its maximum
//!   shows;
//! * every other class keeps the value the request submitted — a value outside an
//!   enumerated set is quoted beside the set it had to be in — which is what the framework
//!   already states.
//!
//! The second thing chosen here is how a `@pattern` requirement QUOTES its expression: the
//! expression inside the whole-value anchors the model wraps it in, except where what those
//! anchors enclose carries an anchor of its OWN — an alternation whose branches are each
//! anchored — which is quoted as the model declares it, anchors included
//! ([`::service_common::validation::unanchored_keeping_inner_anchors`], whose own docs carry
//! the derivation: dropping that pair from such a text states a requirement of neither
//! branch, so what a caller lifts out of the message would admit values this service
//! refuses). It is decided from the expression's own syntax, so it belongs to the `@pattern`
//! and covers every member of every operation that carries one; the CHECK always runs the
//! model's pattern as written.
//!
//! The next thing chosen here is the ORDER an enumerated type's value set is listed in: the
//! front door in front of this service holds a type's admissible values in a HASH TABLE
//! keyed on each value's own text and enumerates that table, so the set is stated in
//! ascending bucket order rather than in the order the model declares the type
//! ([`::service_common::validation_message::in_hashed_enum_order`], whose own docs carry the
//! derivation). It is computed from the declared values themselves, so it belongs to the
//! enumerated TYPE: every enum of this model is listed that way and a type the model gains
//! needs nothing here.
//!
//! Nothing here is keyed to an operation, to a member path or to a submitted value, so one
//! rule covers every `@required` member, every `@length` and `@range` bound, every
//! `@pattern` and every enum of every operation alike — and the one split the classes make,
//! the expressions published with the rejected value against every other member, is read off
//! the `@pattern` of the SHAPE the model gives the member rather than off the request.
//!
//! The error-shape NAME this chooses is the bare one, because naming it on the wire is not
//! this seam's: the shape name a chosen refusal carries is reduced by the framework to the
//! characters a shape id holds, so the namespace AWS IoT qualifies it with cannot travel
//! through here. `ShapeNamespacedErrorType`, in front of every operation, is what writes
//! that name — for this shape wherever it comes from, so a refusal worded here and one a
//! handler answers name it identically.
//!
//! WHICH shape is named is a property of the OPERATION, not of this seam: the classic IoT
//! surface refuses an invalid request with the legacy exception above, while the planes
//! whose operations model `ValidationException` name that one. This seam is handed the
//! violations alone and never the operation they were found in, so it states the classic
//! surface's shape, and the other planes are served in front of it
//! ([`super::modelled_refusal`]) — which states the same envelope, clauses and value
//! rendering under the shape those operations model.

use std::collections::HashSet;

use ::smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` covers both what this service does not reword (an authentication refusal is no
/// input-constraint failure, and its code and status are not this service's to change) and
/// a refusal with nothing to state.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    refused_under(refusal, super::INVALID_REQUEST_EXCEPTION, &HashSet::new())
}

/// The same statement of `refusal`, under the error shape `code` and with the member paths
/// in `quoting_a_collection` stated WITH their value — for the planes of this service whose
/// operations model ANOTHER shape for an invalid request ([`super::modelled_refusal`]), so
/// one envelope, one clause set and one value rendering serve every operation and only the
/// shape name differs.
///
/// A path belongs in that set when the CALLER of this function has already put the submitted
/// COLLECTION into the clause's value half — which the layer holding the request can do for a
/// collection of any size, where this seam can only do it for an empty one
/// ([`as_this_service_states`]).
pub(crate) fn refused_under(
    refusal: &Refusal,
    code: &str,
    quoting_a_collection: &HashSet<String>,
) -> Option<Refused> {
    let (stated, mut stating_collections) = as_this_service_states(refusal);
    stating_collections.extend(quoting_a_collection.iter().cloned());
    ::service_common::validation_message::aws_validation_refusal_withholding_violations_with(
        &stated,
        quoted_expression,
        |violation| states_no_value(violation, &stating_collections),
    )
    .map(|stated| Refused {
        code: code.to_owned(),
        message: stated.message,
    })
}

/// `refusal` with the two things this service states of its own — every enumerated type's
/// value set in the order its front door states it, and the EMPTY collection a clause about
/// a collection member quotes where the framework's walk summarised its size
/// ([`::service_common::validation_message::quoting_an_empty_submitted_collection`]) — beside
/// the member paths whose clause now states such a collection.
///
/// The paths are collected HERE, where the rewriting happens, rather than read back off the
/// rewritten value: whether a clause states a collection is a fact about what this service
/// put there, not about how the value happens to be spelled. Every other violation is
/// untouched.
fn as_this_service_states(refusal: &Refusal) -> (Refusal, HashSet<String>) {
    let Refusal::Invalid(violations) = refusal else {
        return (refusal.clone(), HashSet::new());
    };
    let mut stating_collections = HashSet::new();
    let stated = violations
        .iter()
        .map(::service_common::validation_message::in_hashed_enum_order)
        .map(|violation| {
            let path = violation.path.clone();
            let value = violation.value.clone();
            let quoted =
                ::service_common::validation_message::quoting_an_empty_submitted_collection(
                    violation,
                );
            if quoted.value != value {
                stating_collections.insert(path);
            }
            quoted
        })
        .collect();
    (Refusal::Invalid(stated), stating_collections)
}

/// How a clause of this service quotes the expression a `@pattern` requirement names.
///
/// The published `@pattern` is the expression this service advertises with the whole-value
/// anchors written around it by the model, so a clause quotes it with that pair dropped —
/// except where what the pair encloses carries an anchor of its OWN, which is already a
/// whole-value expression before any wrapping and is quoted as the model declares it. The
/// reading of the pair, and of an anchor, is the shared one
/// ([`::service_common::validation::unanchored_keeping_inner_anchors`], whose docs carry the
/// derivation), called rather than restated; only this RENDERING is chosen here, and the
/// check a violation comes from always ran the model's pattern as written.
fn quoted_expression(expression: &str) -> String {
    ::service_common::validation::unanchored_keeping_inner_anchors(expression)
}

/// The expression every name type of the THING REGISTRY carries, as a clause quotes it:
/// without the whole-value anchors the model writes around it.
///
/// `ThingName`, `ThingGroupName`, `ThingTypeName` and `BillingGroupName` are one string type
/// repeated — `length 1..128, pattern ^[a-zA-Z0-9:_-]+$`
/// (`smithy_query(query="shape",shape="ThingName,ThingGroupName,ThingTypeName,BillingGroupName")`)
/// — so recognising the expression recognises a member that carries a registry NAME, on any
/// operation that takes one and wherever in the request it is bound. The anchors are already
/// off by the time a clause is stated ([`quoted_expression`]), which is the form this names.
///
/// This text is the MODEL's, so it cannot be allowed to drift from it silently: the suite
/// `tests/all/not_found_wording/clause_value_classes.rs` drives all four of those name types
/// through the generated schemas over the wire and asserts the clause quotes exactly this
/// expression, so a model that re-spells the registry's pattern — or a change in how a
/// `@pattern` is requoted — fails there rather than quietly reclassifying the whole class.
const REGISTRY_NAME_EXPRESSION: &str = "[a-zA-Z0-9:_-]+";

/// The expression the DAY OF MONTH a Device Defender audit schedule runs on carries, as a
/// clause quotes it: the model's own text, because its two branches each carry the anchors
/// that make this a whole-value match ([`quoted_expression`]).
///
/// `com.amazonaws.iot#DayOfMonth` is `pattern ^([1-9]|[12][0-9]|3[01])$|^LAST$`
/// (`smithy_query(query="shape",shape="DayOfMonth")`), the type of the `dayOfMonth` member of
/// every operation that schedules an audit, so recognising the expression recognises such a
/// member on each of them.
const DAY_OF_MONTH_EXPRESSION: &str = "^([1-9]|[12][0-9]|3[01])$|^LAST$";

/// The expressions whose `@pattern` clause this service states the REJECTED VALUE beside,
/// each as [`quoted_expression`] quotes it.
///
/// Which half of such a clause is stated is reference data about the member's SHAPE rather
/// than something readable off the request or off the requirement: this service's recorded
/// refusals quote the submitted value beside some of its expressions (a thing-registry name,
/// the day of month an audit schedule runs on) and name the expression alone beside others (a
/// domain-configuration name, a policy version id, a transfer's target account). An
/// expression belongs to the shape, and the shape to every member targeting it, so a row here
/// holds for every operation that carries such a member, in whatever binding — and an
/// expression no row names keeps the valueless clause.
const PATTERNS_STATED_WITH_THE_REJECTED_VALUE: &[&str] =
    &[REGISTRY_NAME_EXPRESSION, DAY_OF_MONTH_EXPRESSION];

/// Whether the clause for `violation` states no value: the member was ABSENT, so there is
/// no value to quote; it fell outside a `@pattern` this service does not publish with the
/// value that missed it, whose clause names the expression alone; it fell outside its
/// `@length` or its `@range`, whose clause names the bound rather than the value that broke
/// it; or it is one this service never discloses.
///
/// What those classes have in common is that the clause already states the REQUIREMENT the
/// value broke — an expression, a length bound, a numeric bound — so quoting the value says
/// the same thing twice. The classes stated WITH a value are the one whose requirement is a
/// SET of admissible values, which the framework already writes that way, and a value outside
/// one of the expressions this service publishes with it
/// ([`PATTERNS_STATED_WITH_THE_REJECTED_VALUE`]). That last one is why this is asked about
/// the violation rather than about its member path: the same member path states its value for
/// this requirement and withholds it for the `@length` bound of the very same type.
///
/// A `@length` broken by a COLLECTION is the one length clause stated WITH a value: the clause
/// states the collection itself, because the walk's own rendering of one is a size no caller
/// sent rather than a value that would say the bound twice. `quoting_a_collection` names the
/// member paths where the collection was restored into the clause; a collection NOBODY could
/// restore keeps the summary the walk left, which the shared clause builder drops the same
/// way.
fn states_no_value(violation: &Violation, quoting_a_collection: &HashSet<String>) -> bool {
    let withheld_by_class = match &violation.kind {
        ViolationKind::Pattern(expression) => {
            !PATTERNS_STATED_WITH_THE_REJECTED_VALUE.contains(&expression.as_str())
        }
        ViolationKind::MissingRequiredMember => true,
        // A numeric bound already states the interval the value had to lie in, so the
        // number that fell outside it says the same thing twice, exactly as the text that
        // broke a length bound does.
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_) => true,
        // The text that broke a length bound says the bound twice; the COLLECTION that
        // broke one is the value the clause states.
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_) => {
            !quoting_a_collection.contains(&violation.path)
        }
        // The classes whose requirement is a SET of admissible values, or the elements
        // themselves, are stated BESIDE the value they rejected.
        ViolationKind::EnumValue(_) | ViolationKind::UniqueItems => false,
        // `ViolationKind` is `#[non_exhaustive]`, so a class the framework gains cannot be
        // named here: until this service's recordings say otherwise it keeps the framework's
        // own value half, which is the value the request submitted.
        _ => false,
    };
    withheld_by_class || violation.sensitive
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// RULE: a member the request did not carry, a member outside a `@pattern` this service
    /// does not publish with the value that missed it, a member whose own TEXT is outside
    /// its `@length` and a member outside its `@range` are each stated with NO value; a
    /// value outside one of the expressions this service DOES publish with it, a COLLECTION
    /// outside its `@length` (stated with the collection itself where the walk's summary
    /// determines it) and every other constraint class state the value the request
    /// submitted; and a `@pattern` is quoted without the whole-value anchors a model wraps
    /// it in, unless what they enclose carries an anchor of its own, in which case the
    /// model's own text is quoted.
    #[test]
    fn an_absent_member_states_no_value_and_every_other_class_states_one() {
        // The rows span the classes the framework reports: an absent member, a value
        // outside a pattern (one whose expression this service publishes with the value and
        // one it does not, and an expression that anchors its own branches), one past each
        // end of a length bound, one past each end of a range bound and one outside an
        // enumerated set.
        let rows = [
            (
                "a member the request did not carry",
                violation("policyDocument", ViolationKind::MissingRequiredMember, None),
                "1 validation error detected: Value at 'policyDocument' failed to satisfy \
                 constraint: Member must not be null",
            ),
            (
                // A registry NAME is the one `@pattern` class stated with its value: the
                // same expression covers a thing, a thing group, a thing type and a
                // billing group, so this row stands for all four.
                "a registry name outside its expression",
                violation(
                    "thingName",
                    ViolationKind::Pattern("^[a-zA-Z0-9:_-]+$".to_owned()),
                    Some("not a thing name"),
                ),
                "1 validation error detected: Value 'not a thing name' at 'thingName' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: [a-zA-Z0-9:_-]+",
            ),
            (
                "a member of another plane outside its pattern",
                violation(
                    "policyName",
                    ViolationKind::Pattern("^[\\w+=,.@-]+$".to_owned()),
                    Some("not a policy name"),
                ),
                "1 validation error detected: Value at 'policyName' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: [\\w+=,.@-]+",
            ),
            (
                // An expression whose own BRANCHES carry the anchors: the pair at the two
                // ends of the text belongs to those branches, so the expression is quoted as
                // the model declares it, and this is the second shape whose clause states the
                // value that missed it.
                "a day of month outside its expression",
                violation(
                    "dayOfMonth",
                    ViolationKind::Pattern(DAY_OF_MONTH_EXPRESSION.to_owned()),
                    Some("99"),
                ),
                "1 validation error detected: Value '99' at 'dayOfMonth' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 ^([1-9]|[12][0-9]|3[01])$|^LAST$",
            ),
            (
                "a member past its length maximum",
                violation("policyName", ViolationKind::MaxLength(128), Some("pppp")),
                "1 validation error detected: Value at 'policyName' failed to satisfy \
                 constraint: Member must have length less than or equal to 128",
            ),
            (
                "a member short of its length minimum",
                violation("targetAwsAccount", ViolationKind::MinLength(12), Some("12345")),
                "1 validation error detected: Value at 'targetAwsAccount' failed to satisfy \
                 constraint: Member must have length greater than or equal to 12",
            ),
            (
                "a member past its range maximum",
                violation(
                    "credentialDurationSeconds",
                    ViolationKind::MaxValue("43200".to_owned()),
                    Some("46800"),
                ),
                "1 validation error detected: Value at 'credentialDurationSeconds' \
                 failed to satisfy constraint: Member must have value less than or equal to \
                 43200",
            ),
            (
                "a member short of its range minimum",
                violation(
                    "credentialDurationSeconds",
                    ViolationKind::MinValue("900".to_owned()),
                    Some("899"),
                ),
                "1 validation error detected: Value at 'credentialDurationSeconds' \
                 failed to satisfy constraint: Member must have value greater than or equal \
                 to 900",
            ),
            (
                "a member outside its value set",
                violation(
                    "status",
                    ViolationKind::EnumValue(vec!["ACTIVE".to_owned()]),
                    Some("MAYBE"),
                ),
                "1 validation error detected: Value 'MAYBE' at 'status' failed to satisfy \
                 constraint: Member must satisfy enum value set: [ACTIVE]",
            ),
            (
                // The walk summarises an empty list as `0 items`, which no caller sent;
                // the clause states the collection itself.
                "an empty list at a member whose length has a minimum",
                violation(
                    "mandatoryParameters",
                    ViolationKind::MinLength(1),
                    Some("0 items"),
                ),
                "1 validation error detected: Value '[]' at 'mandatoryParameters' failed to \
                 satisfy constraint: Member must have length greater than or equal to 1",
            ),
            (
                "an empty map at a member whose length has a minimum",
                violation(
                    "tokenSigningPublicKeys",
                    ViolationKind::MinLength(1),
                    Some("0 entries"),
                ),
                "1 validation error detected: Value '{}' at 'tokenSigningPublicKeys' failed \
                 to satisfy constraint: Member must have length greater than or equal to 1",
            ),
            (
                // A collection the summary does not determine: the elements are not here,
                // so the clause states the requirement and no value rather than a size
                // nobody submitted.
                "a list past its length maximum",
                violation("queries", ViolationKind::MaxLength(1), Some("2 items")),
                "1 validation error detected: Value at 'queries' failed to satisfy \
                 constraint: Member must have length less than or equal to 1",
            ),
            (
                // `LogTargetType`, declared DEFAULT, THING_GROUP, CLIENT_ID, SOURCE_IP,
                // PRINCIPAL_ID, DEVICE_DEFENDER
                // (`smithy_query(query="shape",shape="LogTargetType")`), is stated in its
                // hash table's bucket order — 1, 11, 12, 12, 14, 15 — which is neither the
                // declaration order nor the alphabetical one, and where two values share a
                // bucket the later-declared one is stated first.
                "a member outside a value set of several members",
                violation(
                    "targetType",
                    ViolationKind::EnumValue(vec![
                        "DEFAULT".to_owned(),
                        "THING_GROUP".to_owned(),
                        "CLIENT_ID".to_owned(),
                        "SOURCE_IP".to_owned(),
                        "PRINCIPAL_ID".to_owned(),
                        "DEVICE_DEFENDER".to_owned(),
                    ]),
                    Some("NOT_A_REAL_TYPE"),
                ),
                "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'targetType' failed \
                 to satisfy constraint: Member must satisfy enum value set: [DEFAULT, \
                 SOURCE_IP, DEVICE_DEFENDER, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]",
            ),
        ];

        for (case, violation, expected) in rows {
            let stated = refused(&Refusal::Invalid(vec![violation]))
                .unwrap_or_else(|| panic!("case {case:?}: expected a stated refusal"));
            assert_eq!(
                stated.code,
                super::super::INVALID_REQUEST_EXCEPTION,
                "case {case:?}: expected this service's own refusal class, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: a refusal that is no constraint violation keeps the framework's own wording.
    #[test]
    fn a_refusal_that_is_no_constraint_violation_is_left_alone() {
        for (case, refusal) in [
            ("no violations at all", Refusal::Invalid(Vec::new())),
            ("an unauthenticated caller", Refusal::Unauthenticated),
        ] {
            assert!(
                refused(&refusal).is_none(),
                "case {case:?}: expected the framework's own wording to stand"
            );
        }
    }
}
