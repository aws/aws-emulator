// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request its front door refused before any handler ran.
//!
//! The framework checks a request against the constraints the model declares and hands the
//! service the refusal STRUCTURALLY — one [`Violation`] per offending position, carrying the
//! member path it sits at, the constraint class it broke and the value it held — and leaves
//! the words to the service, because no model records them. [`refused`] is those words, and
//! it is reached from the two seams that see such a refusal: the per-operation wrapper in
//! [`super`] (which also hands over the request's own lists and the operation's input
//! schema) and [`smithy_server_runtime::Service::override_refusal`].
//!
//! Three rules distinguish this service's wording from the framework's default. None is
//! keyed to an operation or to one member: each holds wherever the constraint it is about
//! is declared.
//!
//! 1. a clause about a COLLECTION member — its own `@length` — quotes the collection the
//!    request carried rather than the `<n> items` summary the validating walk stores in
//!    place of a value it has no rendering for. A list of scalars is quoted as the elements
//!    the request sent (`service_common::validation::submitted_lists`, collected from the
//!    request by the wrapper); a list of modelled STRUCTURES is quoted as the layer in
//!    front of this service prints one ([`list_length_clause`]);
//! 2. a constraint the model declares on the ELEMENTS of a collection member is stated at
//!    that MEMBER — no element position and no `member` step — quoting the collection and
//!    nesting every requirement the element shape declares inside
//!    `Member must satisfy constraint: [ … ]`. The whole set, not just the constraint that
//!    happened to fail: a caller correcting the request is told what each element of that
//!    member has to satisfy. A constraint the COLLECTION itself broke keeps its own
//!    requirement (rule 1), and so does one on a member inside a structure element;
//! 3. an enumerated type's value set is listed in the order the front door's own table of
//!    it is walked — neither the model's declaration order nor sorted
//!    (`service_common::validation_message::hashed_enum_order`) — over the values this
//!    service PUBLISHES for that type ([`PUBLISHED_ENUM_VALUE_SETS`] holds the one whose
//!    published membership is not the shipped model's).
//!
//! Rules 1 and 2 are AWS's own wording rather than this service's, and both are implemented
//! once for the fleet in [`service_common::validation_message`], which this module
//! DELEGATES to rather than rewording: the envelope and its error count, the member paths,
//! the nested element sets, the clause order and every requirement sentence stay the
//! framework's and that module's, as does the order rule 3 lists a set in. What this
//! service states, and all it states, is WHICH values one enumerated type admits, because
//! that is a fact about this service.
//!
//! One limit of rule 2, which the shared rendering states too: an element set is read off
//! the operation input schema codegen derives from the model, and a member schema carries
//! its own traits and not the members of the shape it targets — so only a TOP-LEVEL
//! collection member resolves, and a refusal reached through
//! `override_refusal` (which is handed no schema at all) keeps the framework's element
//! clause.

use ::std::collections::{HashMap, HashSet};

use ::aws_smithy_schema::Schema;
use ::service_common::validation_message;
use ::smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// `StackAttribute`'s admissible values as this service PUBLISHES them: the seventeen the
/// shipped model declares (`smithy_query(query="shape", shape="StackAttribute")`, also kept
/// as `crate::ops::types::STACK_ATTRIBUTE_VALUES`) plus the one the refusal of a value
/// outside the set advertises and the shipped enum does not declare — the computer-use
/// interface of agent access.
///
/// Which values a member ADMITS is not decided here — that is the enum the member targets,
/// so the boundary still refuses a value the shipped model does not declare — and this only
/// decides which attributes the refusal SAYS a stack has.
///
/// In the model's DECLARATION order, with the one the shipped enum does not declare last.
/// The order a refusal lists is not this one: it is the order the front door walks its own
/// table of these values (`service_common::validation_message::hashed_enum_order`). What
/// the order here DOES decide is the one thing that walk cannot — two values the table puts
/// in the same place are read from the one pushed in last, and a value is pushed in when
/// the type DECLARES it. It is NOT the order of the convenience constant beside it
/// (`crate::ops::types::STACK_ATTRIBUTE_VALUES`, which codegen alphabetises and which is
/// used here only to match the row to its type, as a SET).
const PUBLISHED_STACK_ATTRIBUTES: &[&str] = &[
    "STORAGE_CONNECTORS",
    "STORAGE_CONNECTOR_HOMEFOLDERS",
    "STORAGE_CONNECTOR_GOOGLE_DRIVE",
    "STORAGE_CONNECTOR_ONE_DRIVE",
    "REDIRECT_URL",
    "FEEDBACK_URL",
    "THEME_NAME",
    "USER_SETTINGS",
    "EMBED_HOST_DOMAINS",
    "IAM_ROLE_ARN",
    "ACCESS_ENDPOINTS",
    "STREAMING_EXPERIENCE_SETTINGS",
    "CONTENT_REDIRECTION",
    "WEB_AUTHN_REDIRECTION",
    "AGENT_ACCESS_CONFIG",
    "STORAGE_CONNECTOR_AUTHENTICATION",
    "AGENT_ACCESS_CONFIG_KMS_KEY_ARN",
    "AGENT_ACCESS_CONFIG_COMPUTER_USE_INTERFACE_ID",
];

/// The enumerated types whose admissible set this service publishes other than the shipped
/// model declares it: one row per type, the model's declared values beside the service's
/// ([`validation_message::stated_enum_value_set`] matches a row to a clause's set as a SET,
/// so the model half may be written in any order).
///
/// Every other enumerated type of this model is listed exactly as the model declares it, so
/// a type the model gains needs no row here.
const PUBLISHED_ENUM_VALUE_SETS: &validation_message::StatedEnumValueSets<'static> = &[(
    crate::ops::types::STACK_ATTRIBUTE_VALUES,
    PUBLISHED_STACK_ATTRIBUTES,
)];

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `input_schemas` are the input shapes the violations were found against — one, the
/// operation's own, when a request is refused by the wrapper in [`super`], and none when the
/// refusal reached [`smithy_server_runtime::Service::override_refusal`] instead — and
/// `lists` are the lists of scalars the request carried, keyed by the member path the walk
/// reports each at (`service_common::validation::submitted_lists`).
///
/// `None` covers a refusal that is not a constraint failure at all: an authentication
/// refusal is not this service's to reword.
pub(crate) fn refused(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    lists: &HashMap<String, String>,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // Which collections the walk WITHHELD a value for: a member the model marks `@sensitive`,
    // or one whose elements are, has its value dropped as the violation is recorded, and a
    // clause about it states the constraint and no value. The value is not put back here —
    // not for the member's own clause and not for the one stated at it for its elements — so
    // this layer cannot re-introduce a value the walk decided to withhold.
    let withheld: HashSet<&str> = violations
        .iter()
        .filter(|violation| violation.sensitive)
        .map(|violation| {
            ::service_common::validation::list_of_element(&violation.path)
                .unwrap_or(violation.path.as_str())
        })
        .collect();
    // Each violation restated BEFORE the clauses are built: the value half of a collection's
    // own clause (rule 1) and the set an enumerated type advertises (rule 3). Bounded by the
    // ceiling the framework's own walk collects under, so the work — and the length of the
    // message — is a constant of this crate rather than something one request chooses.
    let restated: Vec<Violation> = violations
        .iter()
        .take(::service_common::validation::MAX_VIOLATIONS)
        .map(|violation| restated(violation, lists))
        .collect();
    validation_message::aws_validation_refusal_over_elements_stating(
        &Refusal::Invalid(restated),
        // The expression a `@pattern` clause advertises is the one the model declares: this
        // service's refusals quote it with the whole-value anchors the model writes.
        str::to_string,
        |collection| {
            (!withheld.contains(collection))
                .then(|| lists.get(collection).cloned())
                .flatten()
        },
        |collection| element_set(collection, input_schemas),
    )
}

/// `violation` with the two halves this service states its own way: the value a clause about
/// a COLLECTION member quotes, and the value set an enumerated type advertises.
///
/// Everything else is carried through untouched — the member path, the constraint class and
/// the `@sensitive` withholding the walk applied, which [`quoted_collection`] honours rather
/// than quoting a value back over.
fn restated(violation: &Violation, lists: &HashMap<String, String>) -> Violation {
    Violation {
        path: violation.path.clone(),
        kind: published(&violation.kind),
        value: quoted_collection(violation, lists).or_else(|| violation.value.clone()),
        sensitive: violation.sensitive,
    }
}

/// The collection `violation` is about, as a clause quotes it, or `None` when `violation` is
/// not a collection's own `@length` clause this layer can state the value of.
///
/// A `@length` on a list is checked ON THE MEMBER, before the validating walk descends into
/// the elements, so the only rendering the walk has for the rejected value there is a
/// SUMMARY of its size — `<n> items`
/// (`vendor/smithy-schema-server/src/constraints.rs`) — which is not a value any caller sent
/// and not a rendering AWS emits. AWS quotes the LIST. Which rendering that is depends on
/// what the elements are: the elements themselves for a list of scalars, which only the
/// REQUEST can answer (`lists`), and the printed element the layer in front of this service
/// writes for a list of modelled structures ([`super::list_length_clause`]).
///
/// `None` for a member whose value the validating walk WITHHELD: a clause about a
/// `@sensitive` collection states the constraint and no value, and quoting the request's own
/// elements there would undo that.
fn quoted_collection(violation: &Violation, lists: &HashMap<String, String>) -> Option<String> {
    if violation.sensitive
        || !matches!(
            violation.kind,
            ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
        )
    {
        return None;
    }
    // The summary the walk stored is the only signal the violation carries about the shape
    // that broke the bound (`validation_message::list_item_count` is the shared reading of
    // it), so a string member whose own value happens to read as one is the accepted
    // collision that reading already documents.
    validation_message::list_item_count(violation.value.as_deref()?)?;
    lists
        .get(&violation.path)
        .cloned()
        .or_else(|| super::list_length_clause::quoted_list(violation))
}

/// The nested requirement a clause stated at the collection member `collection` carries: the
/// whole set of requirements the element shape declares, each sentence the framework's own,
/// with an enumerated type's value set as this service publishes it.
///
/// `None` when no handed-over input schema declares an element constraint set for that
/// member, which is what leaves the framework's own element clause standing.
fn element_set(collection: &str, input_schemas: &[&'static Schema]) -> Option<String> {
    let declared = validation_message::element_constraint_set(input_schemas, collection)?;
    let requirements: Vec<String> = declared
        .iter()
        .map(|kind| validation_message::requirement(published(kind)))
        .collect();
    Some(validation_message::element_set_requirement(&requirements))
}

/// `kind` with an enumerated type's value set stated as this service lists it — the values
/// it PUBLISHES for that type ([`PUBLISHED_ENUM_VALUE_SETS`]), in the order the front door
/// walks its own table of them — and every other constraint class returned as it was.
fn published(kind: &ViolationKind) -> ViolationKind {
    let ViolationKind::EnumValue(declared) = kind else {
        return kind.clone();
    };
    let published: Vec<String> =
        match validation_message::stated_enum_value_set(declared, PUBLISHED_ENUM_VALUE_SETS) {
            Some(service) => service.iter().map(|value| (*value).to_owned()).collect(),
            None => declared.clone(),
        };
    ViolationKind::EnumValue(
        validation_message::hashed_enum_order(&published)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation, as the validating walk records one.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// The input schemas a wrapped `UpdateStack` hands over.
    fn update_stack() -> Vec<&'static Schema> {
        vec![crate::ops::types::UPDATE_STACK_INPUT_SCHEMA]
    }

    /// RULE 2 and 3: a constraint one of a list's ELEMENTS broke is stated at the list —
    /// quoting the list the request carried, with the element shape's whole set nested —
    /// and the enumerated set it advertises is the one this service publishes, in the order
    /// its own table of the values is walked.
    #[test]
    fn an_element_constraint_is_stated_at_the_list_with_the_published_set() {
        let lists = HashMap::from([(
            "attributesToDelete".to_owned(),
            "[access_endpoints]".to_owned(),
        )]);
        let refusal = Refusal::Invalid(vec![violation(
            "attributesToDelete.1.member",
            ViolationKind::EnumValue(
                crate::ops::types::STACK_ATTRIBUTE_VALUES
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
            ),
            Some("access_endpoints"),
        )]);
        let stated = refused(&refusal, &update_stack(), &lists)
            .expect("a list-element clause is stated");
        let set = validation_message::hashed_enum_order(PUBLISHED_STACK_ATTRIBUTES).join(", ");
        let expected = format!(
            "1 validation error detected: Value '[access_endpoints]' at 'attributesToDelete' \
             failed to satisfy constraint: Member must satisfy constraint: [Member must \
             satisfy enum value set: [{set}]]"
        );
        assert_eq!(
            stated.message, expected,
            "case \"a lower-case stack attribute\": expected {expected:?}, got {:?}",
            stated.message
        );
        assert_eq!(
            stated.code, "ValidationException",
            "case \"a lower-case stack attribute\": expected ValidationException, got {:?}",
            stated.code
        );
    }

    /// RULE 1: a clause about a list's own `@length` quotes the list — the submitted
    /// elements for a list of scalars, the printed element for a list of modelled
    /// structures, `[]` for one the request carried no element in — and a clause about
    /// neither keeps the framework's own wording.
    #[test]
    fn a_collection_clause_quotes_the_collection_it_is_about() {
        let lists = HashMap::from([(
            "embedHostDomains".to_owned(),
            "[a.example, b.example]".to_owned(),
        )]);
        let rows = [
            (
                "a list of scalars over its maximum",
                violation(
                    "embedHostDomains",
                    ViolationKind::MaxLength(20),
                    Some("21 items"),
                ),
                "Value '[a.example, b.example]' at 'embedHostDomains' failed to satisfy \
                 constraint: Member must have length less than or equal to 20",
            ),
            (
                "a list the request carried no element in",
                violation("userSettings", ViolationKind::MinLength(1), Some("0 items")),
                "Value '[]' at 'userSettings' failed to satisfy constraint: Member must have \
                 length greater than or equal to 1",
            ),
            (
                "a member outside its pattern",
                violation(
                    "name",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    Some("NOPE"),
                ),
                "Value 'NOPE' at 'name' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: ^[a-z]+$",
            ),
            (
                "an absent required member",
                violation("name", ViolationKind::MissingRequiredMember, None),
                "Value null at 'name' failed to satisfy constraint: Member must not be null",
            ),
        ];
        for (case, offence, clause) in rows {
            let stated = refused(&Refusal::Invalid(vec![offence]), &update_stack(), &lists)
                .unwrap_or_else(|| panic!("case {case:?}: a clause is stated"));
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// A list of modelled structures over its maximum is quoted as the printed elements the
    /// layer in front of this service writes, one per element the request carried.
    #[test]
    fn a_structure_list_is_quoted_as_the_printed_elements() {
        let stated = refused(
            &Refusal::Invalid(vec![violation(
                "accessEndpoints",
                ViolationKind::MaxLength(4),
                Some("5 items"),
            )]),
            &update_stack(),
            &HashMap::new(),
        )
        .expect("a list-length clause is stated");
        assert_eq!(
            stated.message.matches("com.amazonaws.photon.AccessEndpoint@").count(),
            5,
            "case \"five endpoints\": expected one printed element per element, got {:?}",
            stated.message
        );
        assert!(
            !stated.message.contains("5 items"),
            "case \"five endpoints\": expected the list quoted rather than counted, got {:?}",
            stated.message
        );
    }

    /// Two clauses in one refusal: the collection is quoted and the other clause is the
    /// framework's, inside the framework's own envelope for two.
    #[test]
    fn one_quoted_clause_leaves_the_others_as_the_framework_stated_them() {
        let over = violation("accessEndpoints", ViolationKind::MaxLength(4), Some("5 items"));
        let absent = violation("name", ViolationKind::MissingRequiredMember, None);
        let stated = refused(
            &Refusal::Invalid(vec![over, absent.clone()]),
            &update_stack(),
            &HashMap::new(),
        )
        .expect("a list-length clause is stated");
        assert!(
            stated.message.starts_with("2 validation errors detected: "),
            "expected the framework's envelope for two clauses, got {:?}",
            stated.message
        );
        assert!(
            stated.message.contains(&absent.message()),
            "expected the framework's own clause for the absent member, got {:?}",
            stated.message
        );
    }

    /// A value the validating walk WITHHELD is not put back: a clause about a `@sensitive`
    /// collection member, and one stated at it for its elements, names the constraint and no
    /// value even though the request's own elements are at hand.
    #[test]
    fn a_withheld_value_is_not_quoted_back() {
        let lists = HashMap::from([
            ("userSettings".to_owned(), "[a, b]".to_owned()),
            (
                "attributesToDelete".to_owned(),
                "[access_endpoints]".to_owned(),
            ),
        ]);
        let published = validation_message::hashed_enum_order(PUBLISHED_STACK_ATTRIBUTES).join(", ");
        let rows = [
            (
                "a sensitive collection over its own bound",
                Violation {
                    path: "userSettings".to_owned(),
                    kind: ViolationKind::MaxLength(1),
                    value: None,
                    sensitive: true,
                },
                "Value at 'userSettings' failed to satisfy constraint: Member must have \
                 length less than or equal to 1"
                    .to_owned(),
            ),
            (
                "a sensitive collection's element outside its value set",
                Violation {
                    path: "attributesToDelete.1.member".to_owned(),
                    kind: ViolationKind::EnumValue(vec!["ACCESS_ENDPOINTS".to_owned()]),
                    value: None,
                    sensitive: true,
                },
                format!(
                    "Value at 'attributesToDelete' failed to satisfy constraint: Member must \
                     satisfy constraint: [Member must satisfy enum value set: [{published}]]"
                ),
            ),
        ];
        for (case, offence, clause) in rows {
            let stated = refused(
                &Refusal::Invalid(vec![offence]),
                &update_stack(),
                &lists,
            )
            .unwrap_or_else(|| panic!("case {case:?}: a clause is stated"));
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// An authentication refusal is not this service's to reword.
    #[test]
    fn an_authentication_refusal_keeps_the_frameworks_wording() {
        let stated = refused(&Refusal::Unauthenticated, &update_stack(), &HashMap::new());
        assert!(
            stated.is_none(),
            "expected the framework's own wording, got {stated:?}"
        );
    }
}
