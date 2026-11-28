// SPDX-License-Identifier: CC0-1.0

//! How this service words the requests the framework refused before any handler ran.
//!
//! The boundary validator finds a value that failed the rules the model declares and hands
//! this service the violations ([`Violation`]: the member path, what it failed, and the value
//! as the walk rendered it), with the lists the request carried where the seam it arrived on
//! has them ([`crate::service`] owns that seam). The envelope over those clauses, each clause
//! itself, the `@pattern` rendering and the grouping of a constraint a list's ELEMENTS broke
//! are the SHARED ones ([`service_common::validation_message`]), so this service cannot word
//! one constraint differently from the next; what lives here is only the four choices that
//! are this service's own, and the first three are made by handing the violations through
//! before that shared rendering runs:
//!
//! * a rejected value is echoed only up to [`MAX_ECHOED_VALUE`] bytes, and a longer one
//!   states that bounded prefix followed by [`ELISION`] — the marker this service's own
//!   recorded clauses carry, which names no byte count (the other marker in the tree,
//!   `... [<n> bytes]`, is what [`service_common::validation::echo_held_to`] keeps). A
//!   caller controls the length of what it sends, and a value that failed a MAXIMUM bound is
//!   by definition longer than the member may hold, so echoing it whole reproduces in a
//!   response body — which a client logs, traces and attaches to a ticket — exactly what the
//!   service refused to store. It holds for the value a clause states at a COLLECTION member
//!   too — the list the request carried there, rendered — since that rendering is no shorter
//!   than the value that broke the rule;
//! * an enumerated type's admitted values are stated in the order THIS SERVICE lists that
//!   set in ([`crate::state::STATED_VALUE_SETS`]), which is a property of the enumerated
//!   type rather than of the member, the operation or the request that carried the value;
//! * a `Name` the service cannot use carries the one sentence stating what a name may be
//!   built from ([`crate::state::INVALID_SECRET_NAME_MESSAGE`]), the same sentence the
//!   create's own charset check answers with, so one unusable name reads the same whether
//!   the boundary or the handler refused it; and a value past one of the maximums this
//!   service enforces ITSELF ([`STATED_MAXIMUMS`]), or outside one of the grammars it states
//!   itself ([`STATED_GRAMMARS`]), states that operation's own modelled refusal instead of a
//!   constraint clause \u2014 as does a `SecretId` past the size a `SecretId` may be
//!   ([`crate::state::SECRET_ID_TOO_LONG_MESSAGE`]);
//! * a constraint one of a LIST member's elements broke is stated at that member, carrying
//!   the element shape's whole declared constraint set ([`element_set`]) and the list the
//!   REQUEST held there, where the framework reports the offending element alone and the one
//!   constraint it broke. The set is read off the model by the member's own name, because a
//!   list member of this model sits inside a structure its input nests as often as directly
//!   on the input; the `@pattern` inside that set is quoted as the MODEL writes it, which is
//!   the rendering a nested set states — a clause stated at a member of its own quotes the
//!   expression this service PUBLISHES for that member's shape ([`PUBLISHED_AS_DECLARED`]),
//!   which for most of them is the unanchored rendering of the same expression.
//!
//! One reading sits beside those: WHICH members a clause states no value for, which is the
//! model's answer rather than a choice ([`WITHHELD_MEMBERS`]).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use ::aws_smithy_schema::Schema;
use service_common::sensitive_members::WithheldMembers;
use service_common::validation::{list_of_element, unanchored_unless_published_as_declared};
use service_common::validation_message::{element_constraint_set, element_set_requirement};
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// This service's Smithy model: the one source for which of its members are `@sensitive`,
/// and so for which values a clause states none.
const MODEL_JSON: &str =
    include_str!("../../smithy-model/com-amazonaws-secretsmanager-secretsmanager.json");

/// The members of this model whose value a clause WITHHOLDS, read off [`MODEL_JSON`] rather
/// than listed here — this model marks the material a caller submits, `SecretString`,
/// `SecretBinary` and the rotation token.
///
/// Read from the model rather than from the violation's own flag, because the two seams this
/// module serves hand that flag over differently: the walk that collects a request's lists
/// puts a withheld value BACK (`service_common::validation`, which states why) and clears the
/// flag with it, so a decision taken on the flag alone would echo into a response exactly the
/// material the member is marked to keep out of one. The flag is still honoured beside this,
/// so a member the model marks only on one of two inputs of one name keeps its clause.
static WITHHELD_MEMBERS: LazyLock<WithheldMembers> =
    LazyLock::new(|| WithheldMembers::from_model(MODEL_JSON));

/// The error code every constraint refusal of this service travels under, which is also
/// the code the shared rendering answers: a client branches on the code, so rewording a
/// clause must not move it.
const CONSTRAINT_CODE: &str = "ValidationException";

/// The code a parameter refusal of this service travels under: "The parameter name or
/// value is invalid." (`InvalidParameterException`, this service's Smithy model).
const PARAMETER_CODE: &str = "InvalidParameterException";

/// The code a refusal about the REQUEST travels under: "You provided a parameter value
/// that is not valid for the current state of the resource."
/// (`InvalidRequestException`, this service's Smithy model) — the class the batched read's
/// page-size rule is answered in.
const REQUEST_CODE: &str = "InvalidRequestException";

/// The code a refusal of a pagination token this service cannot resume from travels under:
/// "The `NextToken` value is invalid." (`InvalidNextTokenException`, this service's Smithy
/// model), which every paginated operation of this service models — so an SDK paginator
/// reads "start the scan again" rather than the permanent client fault a constraint clause
/// reports.
const NEXT_TOKEN_CODE: &str = "InvalidNextTokenException";

/// The `@pattern` expressions this service publishes EXACTLY as the model writes them,
/// anchors included, keyed by the expression because what a service publishes belongs to the
/// SHAPE and so holds for every member targeting it.
///
/// One shape of this model is published that way: `RotationTokenType`
/// (`smithy_query(query="shape",shape="RotationTokenType")`), whose recorded refusal quotes
/// the whole-value form. Every other expression of this model is published without the pair
/// the model writes around it, which is the rendering the shared reading drops
/// ([`unanchored_unless_published_as_declared`], which owns both rules).
const PUBLISHED_AS_DECLARED: &[&str] = &[r"^[a-zA-Z0-9\-]+$"];

/// How a clause of this service quotes a `@pattern` back: the expression this service
/// publishes for the member's shape ([`PUBLISHED_AS_DECLARED`]). The CHECK always used the
/// model's pattern as written.
fn quoted_pattern(regex: &str) -> String {
    unanchored_unless_published_as_declared(regex, PUBLISHED_AS_DECLARED)
}

/// How many bytes of a rejected value a clause of this service echoes before the rest is
/// elided.
const MAX_ECHOED_VALUE: usize = 64;

/// What stands in for the bytes an elided echo leaves out.
const ELISION: &str = " ...";

/// The member path `CreateSecret`'s `Name` is reported at, as the framework spells a
/// top-level member (lowerCamel). It is the only member of this service's model at that
/// path, so the name rule below reaches nothing else.
const NAME_PATH: &str = "name";

/// The member path a `SecretId` is reported at, as the framework spells a top-level member
/// (lowerCamel) — the one member of this model at that path, and the member every operation
/// addressed to one secret carries.
const SECRET_ID_PATH: &str = "secretId";

/// Whether `violation` is a `SecretId` past the size a `SecretId` may be.
///
/// A size limit belongs to the SHAPE the member targets (`SecretIdType`, this service's
/// Smithy model), so it holds at every operation that carries one — and
/// `InvalidParameterException` is one of this service's common errors, so every one of them
/// models the refusal. A `SecretId` BELOW the minimum is not past a size limit and keeps its
/// own answer.
fn secret_id_past_the_size_limit(violation: &Violation) -> bool {
    violation.path == SECRET_ID_PATH && matches!(violation.kind, ViolationKind::MaxLength(_))
}

/// One MAXIMUM this service refuses in its OWN words rather than with the shared
/// constraint clause: the operation whose input declares it, the member path the boundary
/// walk reports it at, the modelled error code the refusal travels under, and the sentence
/// it states.
///
/// Secrets Manager enforces some of its documented parameter limits in the service itself:
/// such a request is answered with one of THAT OPERATION's modelled errors and a sentence
/// stating the rule, so a client that branches on the modelled code to page smaller or to
/// split a document sees it — where the shared `ValidationException` envelope is a code the
/// operation does not model at all, and states the value it refused. Every other bound this
/// model declares keeps that shared clause, so only the maximums the service states itself
/// are listed in [`STATED_MAXIMUMS`].
///
/// A MAXIMUM, not a minimum: each sentence says the value is past a documented ceiling, so
/// it is answered for a value that is too large or too long and never for one below a
/// floor, which keeps its own clause.
struct StatedMaximum {
    operation: &'static str,
    path: &'static str,
    code: &'static str,
    /// The sentence, built where that operation's own refusals are worded.
    message: fn() -> String,
}

/// Every maximum of this service stated that way, keyed by the operation as well as the
/// member: a limit belongs to the parameter of ONE operation, and the same member name on
/// another operation (a page size with a bound of its own) keeps the shared clause.
///
/// The path is the member path with every list POSITION dropped ([`positionless`]), so a
/// member of a list's ELEMENTS is named once however many positions a request carried.
const STATED_MAXIMUMS: &[StatedMaximum] = &[
    // The batched read's id list and its page size: both bound by the one batch that read
    // answers, and both modelled errors of that operation.
    StatedMaximum {
        operation: "BatchGetSecretValue",
        path: "secretIdList",
        code: PARAMETER_CODE,
        message: crate::ops::batch_get_secret_value::too_many_ids_message,
    },
    StatedMaximum {
        operation: "BatchGetSecretValue",
        path: "maxResults",
        code: REQUEST_CODE,
        message: crate::ops::batch_get_secret_value::max_results_above_batch_message,
    },
    // A resource policy document past the size limit Secrets Manager documents for one
    // (`crate::state::RESOURCE_POLICY_CHARS`), at both members that carry such a document —
    // the limit is the DOCUMENT's, so the attach and the pre-flight validation stand or
    // fall by it alike, and neither quotes the document back.
    StatedMaximum {
        operation: "PutResourcePolicy",
        path: "resourcePolicy",
        code: PARAMETER_CODE,
        message: resource_policy_too_long_message,
    },
    StatedMaximum {
        operation: "ValidateResourcePolicy",
        path: "resourcePolicy",
        code: PARAMETER_CODE,
        message: resource_policy_too_long_message,
    },
    // How many values ONE filter carries (`crate::state::FILTER_VALUES_PER_FILTER`), at
    // both operations that select with filters: the count is the `Filter` shape's, so the
    // listing and the batched read answer it alike, and the sentence names the count
    // instead of quoting the values back.
    StatedMaximum {
        operation: "BatchGetSecretValue",
        path: "filters.member.values",
        code: PARAMETER_CODE,
        message: crate::state::too_many_filter_values_message,
    },
    StatedMaximum {
        operation: "ListSecrets",
        path: "filters.member.values",
        code: PARAMETER_CODE,
        message: crate::state::too_many_filter_values_message,
    },
    // A pagination token longer than one this service could have minted cannot be resumed
    // from, which is this service's invalid-token refusal rather than a schema clause — at
    // every operation that pages, each of which models that error.
    StatedMaximum {
        operation: "BatchGetSecretValue",
        path: "nextToken",
        code: NEXT_TOKEN_CODE,
        message: invalid_next_token_message,
    },
    StatedMaximum {
        operation: "ListSecrets",
        path: "nextToken",
        code: NEXT_TOKEN_CODE,
        message: invalid_next_token_message,
    },
    StatedMaximum {
        operation: "ListSecretVersionIds",
        path: "nextToken",
        code: NEXT_TOKEN_CODE,
        message: invalid_next_token_message,
    },
    StatedMaximum {
        operation: "ListResourcePolicies",
        path: "nextToken",
        code: NEXT_TOKEN_CODE,
        message: invalid_next_token_message,
    },
];

/// The sentence a document past [`crate::state::RESOURCE_POLICY_CHARS`] is refused with,
/// as a function so [`STATED_MAXIMUMS`] can name it beside the ones that state a bound.
fn resource_policy_too_long_message() -> String {
    crate::state::RESOURCE_POLICY_TOO_LONG_MESSAGE.to_string()
}

/// The sentence a pagination token this service cannot resume from is refused with, the
/// same one every operation's own handler answers an unusable cursor with
/// ([`crate::state::INVALID_NEXT_TOKEN_MESSAGE`]), as a function so [`STATED_MAXIMUMS`] can
/// name it.
fn invalid_next_token_message() -> String {
    crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string()
}

/// One GRAMMAR this service refuses in its OWN words rather than with the shared constraint
/// clause: the member path the value is reported at ([`positionless`]), the modelled error
/// code the refusal travels under, and the sentence built from the rejected value.
///
/// The counterpart of [`STATED_MAXIMUMS`] for a `@pattern`: a value outside the character
/// set a parameter admits is a parameter the service cannot use, so it is answered with that
/// operation's own modelled error and a sentence naming the value — never with the clause
/// nesting the whole declared set, which travels under a code the operation does not model.
/// A grammar belongs to the SHAPE the member targets, so a row holds for every operation
/// that binds it.
struct StatedGrammar {
    path: &'static str,
    code: &'static str,
    /// The sentence, built from the value the request carried.
    message: fn(&str) -> String,
}

/// Every grammar of this service stated that way: the values of a `Filter`, whose
/// `@pattern` (`FilterValueStringType`, this service's Smithy model) is the character set a
/// search value may be built from.
const STATED_GRAMMARS: &[StatedGrammar] = &[StatedGrammar {
    path: "filters.member.values.member",
    code: PARAMETER_CODE,
    message: crate::state::invalid_filter_value_message,
}];

/// `path` with every list POSITION dropped: `filters.1.member.values` reads
/// `filters.member.values`, and a top-level member is unchanged.
///
/// A rule this service states for a member states it for that member wherever a request
/// carried it, so the tables above are keyed by the path a member is reported at with the
/// positions of the lists above it left out.
fn positionless(path: &str) -> String {
    let steps: Vec<&str> = path
        .split('.')
        .filter(|step| !step.bytes().all(|byte| byte.is_ascii_digit()))
        .collect();
    steps.join(".")
}

/// Whether `kind` is a value past a MAXIMUM the model declares — too long, or too large.
fn past_a_maximum(kind: &ViolationKind) -> bool {
    matches!(
        kind,
        ViolationKind::MaxLength(_) | ViolationKind::MaxValue(_)
    )
}

/// How `operation` states a maximum one of `violations` broke, or `None` when it states
/// none — which leaves the shared constraint clause to answer.
fn stated_maximum(operation: Option<&str>, violations: &[Violation]) -> Option<Refused> {
    let operation = operation?;
    let stated = STATED_MAXIMUMS.iter().find(|maximum| {
        maximum.operation == operation
            && violations.iter().any(|violation| {
                positionless(&violation.path) == maximum.path && past_a_maximum(&violation.kind)
            })
    })?;
    Some(Refused {
        code: stated.code.to_string(),
        message: (stated.message)(),
    })
}

/// How this service states a grammar one of `violations` broke ([`STATED_GRAMMARS`]), or
/// `None` when it states none — which leaves the shared constraint clause to answer.
///
/// The sentence names the value the request carried AS SENT, not held to the bound a
/// constraint clause echoes under: that bound is a property of the clause this service's
/// recordings show it in, and this sentence is the one AWS states with the value copied from
/// the request. The value is still bounded — the member's own declared maximum, and the
/// ceiling the collecting walk restores an echo under
/// (`service_common::validation::MAX_RESTORED_ECHO`) — so one request cannot size the
/// response it is refused with.
///
/// A violation whose value no walk could carry keeps the shared CLAUSE instead: this
/// sentence is nothing but the fixed text and that value, so a refusal that could only state
/// an empty one tells a caller less than the clause naming the member and its whole
/// constraint set.
fn stated_grammar(violations: &[Violation]) -> Option<Refused> {
    let (stated, violation) = STATED_GRAMMARS.iter().find_map(|grammar| {
        let broken = violations.iter().find(|violation| {
            positionless(&violation.path) == grammar.path
                && matches!(violation.kind, ViolationKind::Pattern(_))
        })?;
        Some((grammar, broken))
    })?;
    let value = violation.value.as_deref().filter(|value| !value.is_empty())?;
    Some(Refused {
        code: stated.code.to_string(),
        message: (stated.message)(value),
    })
}

/// How this service words `refusal`, or `None` to keep the framework's own wording — for the
/// seam that is handed the violations ALONE, with no request to read a list out of.
pub(super) fn reword(refusal: &Refusal) -> Option<Refused> {
    reword_quoting(None, refusal, HashMap::new)
}

/// How this service words `refusal`, or `None` to keep the framework's own wording.
///
/// `operation` is the modelled operation the request named, or `None` where the seam does
/// not know it: a limit this service enforces itself belongs to one operation's parameter
/// ([`STATED_MAXIMUMS`]), so a refusal whose operation is unknown keeps the shared clause.
///
/// `lists` yields every list of scalar elements the REQUEST carried, keyed by the member path
/// the walk reports that list at and already rendered
/// (`service_common::validation::submitted_lists`) — the value a clause stated at a list
/// member needs and no violation carries. It is a closure because it is only read for a
/// violation found on a list ELEMENT, so a refusal that has none never walks the request
/// again, and the request-less seam passes one that yields nothing.
pub(super) fn reword_quoting(
    operation: Option<&str>,
    refusal: &Refusal,
    lists: impl FnOnce() -> HashMap<String, String>,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // A documented limit this service enforces itself is answered with the operation's own
    // modelled error and the sentence stating that limit ([`STATED_MAXIMUMS`]), never with
    // the shared constraint clause: the clause carries a code the operation does not model
    // and echoes the value the service refused.
    if let Some(stated) = stated_maximum(operation, violations) {
        return Some(stated);
    }
    // A value outside the character set a parameter admits is answered the same way, with
    // the sentence naming that value ([`STATED_GRAMMARS`]).
    if let Some(stated) = stated_grammar(violations) {
        return Some(stated);
    }
    // A `SecretId` past the size a `SecretId` may be is refused for its SIZE, with the
    // sentence naming that member — the same answer a handler that reads the identifier
    // itself gives one that REACHES the limit without breaking the modelled bound
    // ([`crate::state::unusable_secret_id`]), so an over-long identifier reads alike on
    // either side of the model's own ceiling wherever that handler-side reading is in
    // place.
    if violations.iter().any(secret_id_past_the_size_limit) {
        return Some(Refused {
            code: PARAMETER_CODE.to_string(),
            message: crate::state::SECRET_ID_TOO_LONG_MESSAGE.to_string(),
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
    // Read only for a violation one of a list's elements earned: a refusal with none never
    // walks the request a second time.
    let lists = if stated
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        lists()
    } else {
        HashMap::new()
    };
    // A `@sensitive` member's value is withheld, which the runtime schemas cannot say: the
    // model does ([`WITHHELD_MEMBERS`]), and the walk records on the violation what it
    // withheld itself. The shared rendering takes that decision by member path, so both
    // readings are stated over paths.
    let withheld: HashSet<String> = stated
        .iter()
        .filter(|violation| violation.sensitive)
        .map(|violation| violation.path.clone())
        .collect();
    // The envelope, the member paths, the `@pattern` rendering, the grouping over a list's
    // elements and every requirement's wording are the shared ones. The collection a
    // grouped clause quotes is held to what a clause of this service echoes
    // ([`echoed`]), exactly as the value half of every other clause is: a collection the
    // caller sent is no shorter than the element that broke the rule.
    service_common::validation_message::aws_validation_refusal_over_elements_stating_quoting_withholding(
        &Refusal::Invalid(stated),
        quoted_pattern,
        |collection| lists.get(collection).map(|list| echoed(list)),
        |path| withheld.contains(path) || WITHHELD_MEMBERS.withholds_value(path),
        element_set,
    )
}

/// What every element of the list member `collection` has to satisfy, as the clause stated at
/// that member nests it — or `None` for a member whose elements this model declares no
/// constraints for (a list of structures), which keeps the framework's own per-element clause.
///
/// The set is the element shape's WHOLE declared set, read off the model because the refusal
/// carries only the constraints that FAILED, and looked up by the member's OWN name in the
/// shape that declares it — the LAST step of the path the clause is stated at, since a list
/// member of this model sits either directly on an operation input (`secretIdList`) or inside
/// a structure an input nests (`filters.1.member.values`), which is why the set cannot be
/// looked up by that path. The reading, the sentences and the nesting are the shared ones.
fn element_set(collection: &str) -> Option<String> {
    let member = collection.rsplit('.').next()?;
    let requirements: Vec<String> = element_constraint_set(&shapes(), member)?
        .into_iter()
        .map(service_common::validation_message::requirement)
        .collect();
    Some(element_set_requirement(&requirements))
}

/// Every shape of this service a list member can be looked up in: each operation INPUT, and
/// each structure an input nests that declares a list member of its own.
///
/// All of them, not the ones one report happened to cite: an element constraint set is a
/// property of the shape that declares the member, so leaving one out would silently state
/// less than the model declares about that member's elements. Regenerate the input half with
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`; the nested half is the structures those
/// inputs reach that declare a list member — `Filter`'s `Values`
/// (`smithy_query(query="shape",shape="Filter")`), the only one in this model.
fn shapes() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::BATCH_GET_SECRET_VALUE_INPUT_SCHEMA,
        t::CANCEL_ROTATE_SECRET_INPUT_SCHEMA,
        t::CREATE_SECRET_INPUT_SCHEMA,
        t::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        t::DELETE_SECRET_INPUT_SCHEMA,
        t::DESCRIBE_SECRET_INPUT_SCHEMA,
        t::GET_RANDOM_PASSWORD_INPUT_SCHEMA,
        t::GET_RESOURCE_POLICY_INPUT_SCHEMA,
        t::GET_SECRET_VALUE_INPUT_SCHEMA,
        t::LIST_SECRETS_INPUT_SCHEMA,
        t::LIST_SECRET_VERSION_IDS_INPUT_SCHEMA,
        t::PUT_RESOURCE_POLICY_INPUT_SCHEMA,
        t::PUT_SECRET_VALUE_INPUT_SCHEMA,
        t::REMOVE_REGIONS_FROM_REPLICATION_INPUT_SCHEMA,
        t::REPLICATE_SECRET_TO_REGIONS_INPUT_SCHEMA,
        t::RESTORE_SECRET_INPUT_SCHEMA,
        t::ROTATE_SECRET_INPUT_SCHEMA,
        t::STOP_REPLICATION_TO_REPLICA_INPUT_SCHEMA,
        t::TAG_RESOURCE_INPUT_SCHEMA,
        t::UNTAG_RESOURCE_INPUT_SCHEMA,
        t::UPDATE_SECRET_INPUT_SCHEMA,
        t::UPDATE_SECRET_VERSION_STAGE_INPUT_SCHEMA,
        t::VALIDATE_RESOURCE_POLICY_INPUT_SCHEMA,
        &t::Filter::SCHEMA,
    ]
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

    /// What `operation` answers one violation with.
    fn answered(operation: &str, violation: Violation) -> Refused {
        reword_quoting(
            Some(operation),
            &Refusal::Invalid(vec![violation]),
            HashMap::new,
        )
        .expect("this service words every constraint class it is handed")
    }

    /// RULE: a maximum this service enforces ITSELF is stated as that operation's own
    /// modelled refusal with the sentence stating the limit, while every other declared
    /// bound — the same member name on an operation with a bound of its own, and the
    /// MINIMUM end of the same member — keeps the shared constraint clause.
    ///
    /// The rows run over both limits, over both members a policy document reaches, over
    /// the two constraint classes a maximum is declared in (`@range` and `@length`), and
    /// over the two contrasts.
    #[test]
    fn a_maximum_the_service_states_itself_is_that_operations_own_refusal() {
        let policy = crate::state::RESOURCE_POLICY_TOO_LONG_MESSAGE.to_string();
        let rows: Vec<(&str, &str, Violation, &str, String)> = vec![
            (
                "the batched read's page size",
                "BatchGetSecretValue",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("20".to_string()),
                    Some("21"),
                ),
                REQUEST_CODE,
                crate::ops::batch_get_secret_value::max_results_above_batch_message(),
            ),
            (
                "the batched read's id list",
                "BatchGetSecretValue",
                violation("secretIdList", ViolationKind::MaxLength(20), Some("[a, b]")),
                PARAMETER_CODE,
                crate::ops::batch_get_secret_value::too_many_ids_message(),
            ),
            (
                "an attached policy document",
                "PutResourcePolicy",
                violation(
                    "resourcePolicy",
                    ViolationKind::MaxLength(20480),
                    Some("{\"Version\""),
                ),
                PARAMETER_CODE,
                policy.clone(),
            ),
            (
                "a validated policy document",
                "ValidateResourcePolicy",
                violation(
                    "resourcePolicy",
                    ViolationKind::MaxLength(20480),
                    Some("{\"Version\""),
                ),
                PARAMETER_CODE,
                policy,
            ),
        ];
        for (case, operation, broken, code, message) in rows {
            let value = broken.value.clone();
            let stated = answered(operation, broken);
            assert_eq!(
                (stated.code.as_str(), stated.message.as_str()),
                (code, message.as_str()),
                "case {case:?}: expected the operation's own refusal, got {stated:?}"
            );
            if let Some(value) = value {
                assert!(
                    !stated.message.contains(&value),
                    "case {case:?}: expected the sentence to carry no part of the value, got \
                     {:?}",
                    stated.message
                );
            }
        }

        // The contrasts: the listing's page size has a maximum of its own that this service
        // does not state itself, and the MINIMUM end of a member whose maximum it does
        // state is not that sentence either.
        let listing = answered(
            "ListSecrets",
            violation(
                "maxResults",
                ViolationKind::MaxValue("100".to_string()),
                Some("101"),
            ),
        );
        assert_eq!(
            listing.code, CONSTRAINT_CODE,
            "case \"the listing's page size\": expected the shared clause, got {listing:?}"
        );
        let below = answered(
            "BatchGetSecretValue",
            violation(
                "maxResults",
                ViolationKind::MinValue("1".to_string()),
                Some("0"),
            ),
        );
        assert_eq!(
            below.code, CONSTRAINT_CODE,
            "case \"a page size below its minimum\": expected the shared clause, got {below:?}"
        );
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

    /// RULE: a clause quotes the expression this service PUBLISHES for the member's shape —
    /// the whole-value form for the shapes it publishes that way
    /// ([`PUBLISHED_AS_DECLARED`]), and the unanchored rendering of the model's text for
    /// every other.
    ///
    /// The rows run over both renderings, over the two shapes this model declares a
    /// `@pattern` on, and over a `@sensitive` member (whose clause states the expression and
    /// no value) beside one that is not.
    #[test]
    fn a_pattern_is_stated_as_the_service_publishes_that_shapes_expression() {
        let rows: Vec<(&str, &str, &str, Option<&str>, String)> = vec![
            (
                "the rotation token's expression, published as declared",
                "rotationToken",
                r"^[a-zA-Z0-9\-]+$",
                None,
                format!(
                    "1 validation error detected: Value at 'rotationToken' failed to satisfy \
                     constraint: Member must satisfy regular expression pattern: {}",
                    r"^[a-zA-Z0-9\-]+$"
                ),
            ),
            (
                "a rotation window, published without the pair",
                "rotationRules.duration",
                "^[0-9]+h$",
                Some("1x"),
                "1 validation error detected: Value '1x' at 'rotationRules.duration' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: [0-9]+h"
                    .to_string(),
            ),
        ];
        for (case, path, pattern, value, expected) in rows {
            let stated = said(violation(
                path,
                ViolationKind::Pattern(pattern.to_string()),
                value,
            ));
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: a maximum or a grammar this service states itself is keyed by the member a
    /// violation is reported at WITHOUT the positions of the lists above it, so the rule
    /// holds at every position a request carried that member in.
    #[test]
    fn a_member_of_a_lists_elements_is_named_without_its_position() {
        for (case, path, expected) in [
            ("a top-level member", "nextToken", "nextToken"),
            (
                "a member inside a list's element",
                "filters.1.member.values",
                "filters.member.values",
            ),
            (
                "an element of that member",
                "filters.10.member.values.3.member",
                "filters.member.values.member",
            ),
        ] {
            let stated = positionless(path);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: a filter carrying more values than one filter admits, and a pagination token
    /// past the ceiling a token has, are each answered with that operation's OWN modelled
    /// refusal and the sentence stating the rule — at every operation that binds the member,
    /// and whatever position the request carried it in.
    #[test]
    fn a_filter_and_a_token_past_their_ceilings_state_the_operations_own_refusal() {
        let rows: Vec<(&str, &str, Violation, &str, String)> = vec![
            (
                "a filter of the batched read carrying too many values",
                "BatchGetSecretValue",
                violation(
                    "filters.1.member.values",
                    ViolationKind::MaxLength(10),
                    Some("[a, b]"),
                ),
                PARAMETER_CODE,
                crate::state::too_many_filter_values_message(),
            ),
            (
                "a later filter of the listing carrying too many values",
                "ListSecrets",
                violation(
                    "filters.3.member.values",
                    ViolationKind::MaxLength(10),
                    Some("[a, b]"),
                ),
                PARAMETER_CODE,
                crate::state::too_many_filter_values_message(),
            ),
            (
                "the batched read's pagination token",
                "BatchGetSecretValue",
                violation("nextToken", ViolationKind::MaxLength(4096), Some("QQQQ")),
                NEXT_TOKEN_CODE,
                crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
            ),
            (
                "the listing's pagination token",
                "ListSecrets",
                violation("nextToken", ViolationKind::MaxLength(4096), Some("QQQQ")),
                NEXT_TOKEN_CODE,
                crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
            ),
            (
                "the version listing's pagination token",
                "ListSecretVersionIds",
                violation("nextToken", ViolationKind::MaxLength(4096), Some("QQQQ")),
                NEXT_TOKEN_CODE,
                crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
            ),
            (
                "the policy listing's pagination token",
                "ListResourcePolicies",
                violation("nextToken", ViolationKind::MaxLength(4096), Some("QQQQ")),
                NEXT_TOKEN_CODE,
                crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
            ),
        ];
        for (case, operation, broken, code, message) in rows {
            let stated = answered(operation, broken);
            assert_eq!(
                (stated.code.as_str(), stated.message.as_str()),
                (code, message.as_str()),
                "case {case:?}: expected the operation's own refusal, got {stated:?}"
            );
        }
        // The contrast: a token BELOW its minimum is not a token past a ceiling, so it
        // keeps the shared clause.
        let below = answered(
            "ListSecrets",
            violation("nextToken", ViolationKind::MinLength(1), Some("")),
        );
        assert_eq!(
            below.code, CONSTRAINT_CODE,
            "case \"an empty pagination token\": expected the shared clause, got {below:?}"
        );
    }

    /// RULE: a filter value outside the character set a search value may be built from is
    /// that operation's own parameter refusal naming the value as the request sent it, not a
    /// constraint clause.
    #[test]
    fn a_filter_value_outside_its_grammar_names_that_value() {
        let pattern = || ViolationKind::Pattern(r"^\!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*$".to_string());
        // Past the bound a constraint CLAUSE of this service echoes under, which this
        // sentence is not held to: AWS copies the value from the request.
        let long = format!("{}#", "x".repeat(MAX_ECHOED_VALUE));
        let rows: Vec<(&str, Violation, String)> = vec![
            (
                "a value carrying a character outside the set",
                violation("filters.1.member.values.1.member", pattern(), Some("tag#")),
                crate::state::invalid_filter_value_message("tag#"),
            ),
            (
                "a later value of a later filter",
                violation("filters.2.member.values.3.member", pattern(), Some("a*")),
                crate::state::invalid_filter_value_message("a*"),
            ),
            (
                "a value longer than a clause would echo",
                violation(
                    "filters.1.member.values.1.member",
                    pattern(),
                    Some(long.as_str()),
                ),
                crate::state::invalid_filter_value_message(&long),
            ),
        ];
        for (case, broken, message) in rows {
            let stated = answered("BatchGetSecretValue", broken);
            assert_eq!(
                (stated.code.as_str(), stated.message.as_str()),
                (PARAMETER_CODE, message.as_str()),
                "case {case:?}: expected the parameter refusal, got {stated:?}"
            );
        }
        // The contrast: a filter value whose value no walk could carry keeps the shared
        // clause, since this sentence is nothing but its fixed text and that value.
        let unquotable = answered(
            "BatchGetSecretValue",
            violation("filters.1.member.values.1.member", pattern(), None),
        );
        assert_eq!(
            unquotable.code, CONSTRAINT_CODE,
            "case \"a filter value no clause could quote\": expected the shared clause, got \
             {unquotable:?}"
        );
        // The contrast: a filter value past its LENGTH is not outside the grammar, so it
        // keeps the clause nesting the element shape's whole set.
        let long_value = answered(
            "BatchGetSecretValue",
            violation(
                "filters.1.member.values.1.member",
                ViolationKind::MaxLength(512),
                Some("x"),
            ),
        );
        assert_eq!(
            long_value.code, CONSTRAINT_CODE,
            "case \"a filter value past its maximum\": expected the shared clause, got \
             {long_value:?}"
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
    fn an_enum_clause_states_a_set_the_service_pins_no_order_of_as_the_model_declares_it() {
        // `StatusType`, the replication status an answer reports
        // (`smithy_query(query="shape",shape="StatusType")`): this service's recordings show
        // no order of its own for it, so the clause states the set exactly as the violation
        // presented it.
        let declared = ["InProgress", "Failed", "InSync"];
        let stated = said(violation(
            "status",
            ViolationKind::EnumValue(declared.iter().map(|value| value.to_string()).collect()),
            Some("Sideways"),
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value 'Sideways' at 'status' failed to satisfy \
             constraint: Member must satisfy enum value set: [InProgress, Failed, InSync]",
            "case \"a set with no stated order\": got {stated:?}"
        );
    }

    /// RULE: an enumerated type this service states an order for is stated in THAT order
    /// wherever the value was carried — `SortOrderType`'s set reads `[desc, asc]`, which is
    /// neither the order the model declares nor the sorted one.
    #[test]
    fn the_sort_order_set_is_stated_in_the_order_the_service_lists_it() {
        let declared: Vec<String> = crate::ops::types::SORT_ORDER_TYPE_VALUES
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        let stated = said(violation(
            "sortOrder",
            ViolationKind::EnumValue(declared),
            Some("ASC"),
        ));
        assert_eq!(
            stated,
            "1 validation error detected: Value 'ASC' at 'sortOrder' failed to satisfy \
             constraint: Member must satisfy enum value set: [desc, asc]",
            "case \"the sort order set\": got {stated:?}"
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

    /// RULE: the values a clause withholds are exactly the members this model marks
    /// `@sensitive` — the material a caller submits — read off the model rather than listed,
    /// so a clause can never echo one back into a response whichever seam stated it.
    #[test]
    fn the_withheld_members_are_the_submitted_material() {
        let mut names: Vec<&str> = WITHHELD_MEMBERS
            .names()
            .iter()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["rotationToken", "secretBinary", "secretString"],
            "case \"this model's sensitive input members\": got {names:?}"
        );
        for path in [
            "secretString",
            "secretBinary",
            "rotationToken",
            "secretValues.1.member.secretString",
        ] {
            assert!(
                WITHHELD_MEMBERS.withholds_value(path),
                "case {path:?}: expected the value to be withheld"
            );
        }
        for path in ["name", "description", "filters.1.member.values"] {
            assert!(
                !WITHHELD_MEMBERS.withholds_value(path),
                "case {path:?}: expected the value to be stated"
            );
        }
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
