// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The centralized walk enforces the modelled input constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending member, carrying the dotted
//! path it sits at, what it failed and what it held — because no model records the sentence
//! a service says. This module is the choices this service makes about that sentence, and it
//! makes each of them for a CLASS of members or for an enumerated TYPE rather than for an
//! operation:
//!
//! * the clause of a parameter this API's READ/WRITE DATA PLANE reads off the request line
//!   names the offending member and the requirement it broke and NOTHING of the value —
//!   `Value at '<member path>' failed to satisfy constraint: …` — while the control plane's
//!   path labels and body members keep quoting the value the request carried, which is the
//!   half a caller fixes the request from. The data plane is the set of operations the model
//!   serves on its own `storage-` endpoint ([`DATA_PLANE_INPUT_SCHEMAS`]), and the
//!   parameters in question are the ones it binds to the QUERY STRING, read off the model so
//!   a member whose binding changes there changes here with it. A member ABSENT from the
//!   request is not one of them wherever it sits: there was no value to drop, and the `null`
//!   such a clause states is a fact of the request's shape;
//! * an enumerated type's value SET is listed in the order this front door walks its own
//!   table of the admissible values — a table keyed on each value's own text, read bucket by
//!   bucket ([`in_hashed_enum_order`]) — and not in the order the model declares them. The
//!   order is derived from the values themselves, so it belongs to the enumerated TYPE: one
//!   type is listed alike wherever it is carried, at whatever depth and in whatever
//!   operation, and a type the model gains needs nothing added here;
//! * one enumerated type admits a value the shipped model does not yet declare, and the
//!   clause names the set the SERVICE publishes rather than the set the schema carries
//!   ([`PUBLISHED_ENUM_VALUE_SETS`]). That is reference data about the type, keyed by the
//!   declared set, so it holds for every member targeting it.
//!
//! The envelope, the clause, the member paths, the `@pattern` rendering (the expression
//! without the whole-value anchors the model wraps it in) and every constraint class's
//! requirement belong to [`service_common::validation_message`] — the shared home for this
//! wording — and to the framework behind it. None of them is spelled here, so this service
//! cannot word a constraint differently from the services that state the same sentence.

use std::collections::HashSet;
use std::sync::LazyLock;

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints;
use service_common::validation_message::{
    aws_validation_refusal_withholding_violations, in_hashed_enum_order, in_stated_enum_order,
    StatedEnumValueSets,
};
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// `None` for anything that is not an input-constraint refusal: an authentication refusal is
/// a different failure with its own code and status, and nothing about it is this service's
/// to word. The code travels with the message and stays the modelled `ValidationException`
/// (HTTP 400) a client branches on.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // An enumerated type's set is restated BEFORE the envelope words it — first the values
    // the service publishes for that type, then the order its table of them is walked — so
    // only the values and their order move and every other violation class comes back
    // untouched. Doing it on the one refusal this service states is what makes the listing
    // hold for every enum of this model.
    let stated = Refusal::Invalid(
        violations
            .iter()
            .map(|violation| in_stated_enum_order(violation, PUBLISHED_ENUM_VALUE_SETS))
            .map(|violation| in_hashed_enum_order(&violation))
            .collect(),
    );
    aws_validation_refusal_withholding_violations(&stated, states_no_value)
}

/// Whether the clause for `violation` states no value: a parameter the data plane reads off
/// the request line, and nothing else.
///
/// A member the request OMITTED is excluded whatever it is bound to: its clause states the
/// `null` the request's shape had rather than a value, so there is nothing for this to drop.
///
/// A `@sensitive` member needs no entry here: the validating walk never stores such a
/// value, and the clause it hands over already names the member alone, so this decision
/// cannot put one back.
fn states_no_value(violation: &Violation) -> bool {
    !matches!(violation.kind, ViolationKind::MissingRequiredMember)
        && DATA_PLANE_QUERY_MEMBERS.contains(violation.path.as_str())
}

/// The value sets this service publishes for an enumerated type whose published set is not
/// the one the shipped schema carries, keyed by the DECLARED set.
///
/// `StorageType` is modelled with two values and the service admits a third: a refusal of a
/// value outside it names all three. Keyed by the declared set rather than by a member, so
/// every member targeting the type — in any operation, at any depth — names the same set;
/// the ORDER a clause lists it in is still the table walk's, applied after this.
///
/// Source: the `storageType` member of `StartRunRequest` and of `DefaultRunSetting` targets
/// `com.amazonaws.omics#StorageType`, declared `["STATIC", "DYNAMIC"]`
/// (`smithy_query(query="shape", shape="StorageType")`).
static PUBLISHED_ENUM_VALUE_SETS: &StatedEnumValueSets<'static> =
    &[(&["STATIC", "DYNAMIC"], &["S3", "STATIC", "DYNAMIC"])];

/// The member paths whose value this API's data plane reads off the QUERY STRING.
///
/// Built once off the generated schemas of the operations the model serves on its own
/// `storage-` endpoint, so the binding is the MODEL's: a member that stops being a query
/// parameter there stops being one here.
///
/// A member NAME the control plane also declares cannot be told apart from this one by the
/// violation's path — the path is all a refusal carries — so such a name is stated this way
/// wherever it is reached. `partSource` is the one such name in this model (a query
/// parameter of the data plane's part upload, a body member of the control plane's part
/// listing); the alternative, dropping it, would state the data plane's own refusal of it
/// the control plane's way, which is the refusal the recordings pin.
static DATA_PLANE_QUERY_MEMBERS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut query: HashSet<String> = HashSet::new();
    for schema in DATA_PLANE_INPUT_SCHEMAS {
        for member in schema.members() {
            let Some(name) = member.member_name() else {
                continue;
            };
            if member.http_query().is_some() || member.http_query_params().is_some() {
                query.insert(constraints::path::member(name));
            }
        }
    }
    query
});

/// The INPUT shapes of the operations this model serves on its `storage-` endpoint — the
/// read/write data plane, as opposed to the `control-storage-` and `workflows-` control
/// planes.
///
/// Source: the `@endpoint` trait of each operation
/// (`smithy_query(query="operation", operation="<op>")` reports it as
/// `endpoint.hostPrefix`). `CompleteMultipartReadSetUpload` is listed although it binds
/// nothing to the query string, so the set stays the data plane's rather than a list of the
/// operations that happen to have query parameters today.
const DATA_PLANE_INPUT_SCHEMAS: &[&Schema] = &[
    crate::ops::types::COMPLETE_MULTIPART_READ_SET_UPLOAD_INPUT_SCHEMA,
    crate::ops::types::GET_READ_SET_INPUT_SCHEMA,
    crate::ops::types::GET_REFERENCE_INPUT_SCHEMA,
    crate::ops::types::UPLOAD_READ_SET_PART_INPUT_SCHEMA,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some("submitted".to_owned()),
            sensitive: false,
        }
    }

    /// The whole message this service states for one violation.
    fn stated(violation: Violation) -> String {
        refused(&Refusal::Invalid(vec![violation]))
            .expect("an input-constraint refusal is this service's to state")
            .message
    }

    /// The data plane's query parameters are exactly the ones the model binds there: the
    /// path and payload members of the same operations are not among them.
    #[test]
    fn the_data_planes_query_parameters_come_from_the_model() {
        for parameter in ["partNumber", "partSource", "file"] {
            assert!(
                DATA_PLANE_QUERY_MEMBERS.contains(parameter),
                "{parameter:?} is a query parameter of this model's data plane, got \
                 {DATA_PLANE_QUERY_MEMBERS:?}"
            );
        }
        for elsewhere in ["id", "sequenceStoreId", "uploadId", "referenceStoreId", "payload"] {
            assert!(
                !DATA_PLANE_QUERY_MEMBERS.contains(elsewhere),
                "{elsewhere:?} is not bound to the query string, got \
                 {DATA_PLANE_QUERY_MEMBERS:?}"
            );
        }
    }

    /// A control-plane member states the value the request carried, whatever class it broke;
    /// the `@pattern` class states the expression without the whole-value anchors the model
    /// wraps it in.
    #[test]
    fn a_control_plane_clause_states_the_submitted_value() {
        for (kind, expected) in [
            (
                ViolationKind::MinLength(1),
                "Member must have length greater than or equal to 1",
            ),
            (
                ViolationKind::MinValue("0".to_owned()),
                "Member must have value greater than or equal to 0",
            ),
            (
                ViolationKind::MaxValue("100000".to_owned()),
                "Member must have value less than or equal to 100000",
            ),
            (
                ViolationKind::Pattern("^[0-9]+$".to_owned()),
                "Member must satisfy regular expression pattern: [0-9]+",
            ),
            (ViolationKind::UniqueItems, "Member must have unique values"),
        ] {
            let message = stated(violation("priority", kind.clone()));
            let expected = format!(
                "1 validation error detected: Value 'submitted' at 'priority' failed to satisfy \
                 constraint: {expected}"
            );
            assert_eq!(
                message, expected,
                "case {kind:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// A data-plane query parameter states the requirement alone, whatever class it broke:
    /// the decision is the member's and not the constraint class's.
    #[test]
    fn a_data_plane_query_parameter_states_no_value() {
        for (path, kind, expected) in [
            (
                "partNumber",
                ViolationKind::MinValue("1".to_owned()),
                "Member must have value greater than or equal to 1",
            ),
            (
                "partNumber",
                ViolationKind::MaxValue("10000".to_owned()),
                "Member must have value less than or equal to 10000",
            ),
            (
                "partSource",
                ViolationKind::EnumValue(vec!["SOURCE1".to_owned(), "SOURCE2".to_owned()]),
                "Member must satisfy enum value set: [SOURCE1, SOURCE2]",
            ),
        ] {
            let message = stated(violation(path, kind.clone()));
            let expected = format!(
                "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
                 {expected}"
            );
            assert_eq!(
                message, expected,
                "case {path:?}/{kind:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// An absent required member keeps the `null` the request's shape states — on the data
    /// plane's own query parameters too, because absence is not a value that could be
    /// withheld.
    #[test]
    fn an_absent_member_is_stated_as_null() {
        for path in ["defaultRunSetting", "partNumber", "partSource"] {
            let message = stated(Violation {
                path: path.to_owned(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            });
            let expected = format!(
                "1 validation error detected: Value null at '{path}' failed to satisfy \
                 constraint: Member must not be null"
            );
            assert_eq!(
                message, expected,
                "case {path:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// The published value set is named for the type whose schema lags it, and the listing
    /// order is the table walk's for every enum — however the schema presented the values.
    #[test]
    fn an_enum_value_set_is_the_published_one_in_the_order_the_table_is_walked() {
        for (label, declared, listed) in [
            (
                "a type whose published set the schema carries",
                vec!["OFF", "FATAL", "ERROR", "ALL"],
                "[ALL, FATAL, ERROR, OFF]",
            ),
            (
                "the same type presented in another order",
                vec!["ALL", "ERROR", "FATAL", "OFF"],
                "[ALL, FATAL, ERROR, OFF]",
            ),
            (
                "a type the service publishes a further value for",
                vec!["STATIC", "DYNAMIC"],
                "[S3, STATIC, DYNAMIC]",
            ),
        ] {
            let message = stated(violation(
                "logLevel",
                ViolationKind::EnumValue(declared.iter().map(|v| (*v).to_owned()).collect()),
            ));
            let expected = format!(
                "1 validation error detected: Value 'submitted' at 'logLevel' failed to satisfy \
                 constraint: Member must satisfy enum value set: {listed}"
            );
            assert_eq!(
                message, expected,
                "case {label:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// An authentication refusal keeps the framework's wording, and the code a constraint
    /// refusal travels with stays the modelled one.
    #[test]
    fn only_an_input_constraint_refusal_is_reworded() {
        assert_eq!(
            refused(&Refusal::Unauthenticated),
            None,
            "an authentication refusal is not this service's to word"
        );
        let chosen = refused(&Refusal::Invalid(vec![violation(
            "partNumber",
            ViolationKind::MinValue("1".to_owned()),
        )]))
        .expect("an input-constraint refusal is reworded");
        assert_eq!(
            chosen.code, "ValidationException",
            "the code stays the modelled one, got {:?}",
            chosen.code
        );
    }
}
