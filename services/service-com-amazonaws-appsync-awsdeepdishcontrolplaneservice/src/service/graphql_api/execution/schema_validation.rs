// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENT-VALIDATION stage of the GraphQL data plane's request pipeline: what a
//! request's selection is checked against the schema the API serves for, before any field of
//! it is dispatched.
//!
//! AppSync validates the incoming document against the API's ACTIVE schema first, so a
//! selection the type it is selected from does not declare is a failure of the REQUEST — one
//! GraphQL validation error per such selection, and no `data` at all — rather than a field
//! that resolves to nothing. This module is that reading of the rule; the executor beside it
//! ([`super`]) runs only what this stage admits.
//!
//! What it costs is bounded by the reading that produced the selection: a document deeper
//! than [`MAX_SELECTION_DEPTH`] or wider than [`MAX_SELECTED_NODES`] is refused before it is
//! expanded, so this walk recurses at most that deep, visits at most that many selections,
//! and reports at most that many errors. It is synchronous and takes the service's own store
//! for the length of that bounded walk, so no lock is held across an await.

use super::*;

/// The validation errors one request's document carries against the API's ACTIVE schema:
/// one entry per selected field the type it is selected from does not declare.
///
/// A selection the schema does not declare is reported as GraphQL's own validation error,
/// naming the class of the failure, the field and the type that does not declare it, and the
/// query path it sits at. An API whose schema does not declare the ROOT operation type the
/// operation selects on declares none of its fields either, so every selection of it is
/// reported — the schema a Merged API serves for a mutation no source API of it declares a
/// mutation root for. An API holding NO type system at all is not validated against: there is
/// no schema to execute the document against, which the request is refused for before it
/// reaches this stage ([`super::serves_no_schema`]).
///
/// The walk descends while the type a field resolves to is an OBJECT type the schema
/// declares. A selection on an INTERFACE or a UNION type is written under a type condition
/// (`... on T`), which the reading splices into the parent's own selections, so the type that
/// declares those fields is no longer the one this walk holds and they are left for the
/// execution rather than reported against the wrong type. The fields GraphQL's INTROSPECTION
/// SYSTEM adds to every schema belong to no type the schema declares, so they are not
/// validated here either — except on an API that serves NO introspection, whose schema the
/// introspection meta-types are no part of, so the fields selected through the query root's
/// two meta-fields are undefined like any others (`gated_introspection_errors`).
pub(super) fn schema_validation_errors(
    context: &PlaneContext,
    root_type: &str,
    fields: &[SelectedField],
) -> Vec<Json> {
    use crate::service::graphql_api::introspection;

    let store = context.state.lock();
    let Some(api) = store.graphql_apis.get(&context.api_id) else {
        return Vec::new();
    };
    // The type whose two INTROSPECTION meta-fields this API serves nothing through: an API
    // that disables introspection serves a schema the introspection meta-types are no part
    // of, so every field selected THROUGH `__schema` or `__type(name:)` — both of which
    // belong to the query root — is a field of a type that declares none, and `None` for an
    // API that serves introspection, where those selections are answered by the executor
    // rather than reported here ([`introspection::serves_introspection`]).
    let gated_root = match introspection::serves_introspection(api) {
        true => None,
        false => Some(api.query_root_type()),
    };
    let types = &api.types;
    // A root operation type the API's schema does not declare holds no field at all, so
    // EVERY selection of the operation is undefined — a document validated against nothing is
    // not a document whose every field resolves to nothing.
    if !types.contains_key(root_type) {
        let mut errors = Vec::new();
        for field in fields {
            if gated_root.as_deref() == Some(root_type) {
                gated_introspection_errors(field, &mut Vec::new(), &mut errors);
            }
            if introspection::is_introspection_field(&field.name) {
                continue;
            }
            errors.push(json!({
                "message": undefined_field_message(
                    &field.name,
                    root_type,
                    ::std::slice::from_ref(&field.name),
                ),
            }));
        }
        return errors;
    }
    let mut errors = Vec::new();
    undefined_fields(
        types,
        root_type,
        fields,
        gated_root.as_deref(),
        &mut Vec::new(),
        &mut errors,
    );
    errors
}

/// Report every selection an API serving NO introspection leaves undefined under one field of
/// its query root, and nothing for a field that is not one of the introspection system's two
/// root meta-fields.
///
/// `__schema` and `__type(name:)` stay fields of the query root — the schema the API serves
/// is what the meta-types are taken out of — so what is undefined is each field selected
/// THROUGH one of them: the type it resolves to declares none. The selection is not
/// descended into, for the reason a field no type declares is not:
/// there is no type to validate its own sub-selection against. `__typename` is the TYPE
/// system's field rather than the introspection system's, so it is left where it is selected.
fn gated_introspection_errors(
    field: &SelectedField,
    at: &mut Vec<String>,
    errors: &mut Vec<Json>,
) {
    use crate::service::graphql_api::introspection;

    let Some(meta_type) = introspection::root_field_type(&field.name) else {
        return;
    };
    at.push(field.name.clone());
    for selected in &field.selections {
        if selected.name == introspection::TYPENAME_FIELD {
            continue;
        }
        at.push(selected.name.clone());
        errors.push(json!({
            "message": undefined_field_message(&selected.name, meta_type, at),
        }));
        at.pop();
    }
    at.pop();
}

/// Report every field of `fields` that `type_name` does not declare, and walk the
/// sub-selection of each one it does against the type that field resolves to.
///
/// `at` is the query path the walk stands at, which each error names the selection by. The
/// descent is bounded by the reading that produced `fields`: a selection set deeper than
/// [`MAX_SELECTION_DEPTH`] is refused before it is expanded, so this recursion is bounded
/// by the same depth, and the number of errors by the nodes one document may expand to.
///
/// `gated_root` names the query root of an API serving NO introspection, whose two root
/// meta-fields then declare nothing either ([`gated_introspection_errors`]).
pub(super) fn undefined_fields(
    types: &BTreeMap<String, crate::state::TypeRecord>,
    type_name: &str,
    fields: &[SelectedField],
    gated_root: Option<&str>,
    at: &mut Vec<String>,
    errors: &mut Vec<Json>,
) {
    let Some(record) = types.get(type_name) else {
        return;
    };
    if record.parsed.kind != crate::state::OBJECT_KIND {
        return;
    }
    for field in fields {
        if gated_root == Some(type_name) {
            gated_introspection_errors(field, at, errors);
        }
        if crate::service::graphql_api::introspection::is_introspection_field(&field.name) {
            continue;
        }
        at.push(field.name.clone());
        match record
            .parsed
            .fields
            .iter()
            .find(|declared| declared.name == field.name)
        {
            None => errors.push(json!({
                "message": undefined_field_message(&field.name, type_name, at),
            })),
            Some(declared) if !field.selections.is_empty() => {
                let child = declared
                    .value_type
                    .as_ref()
                    .map(named_type)
                    .unwrap_or_default();
                undefined_fields(types, &child, &field.selections, gated_root, at, errors);
            }
            Some(_) => {}
        }
        at.pop();
    }
}

/// The sentence a validation failure of the `FieldUndefined` class states: the field, the
/// type that does not declare it, and the query path the selection sits at.
pub(super) fn undefined_field_message(field_name: &str, type_name: &str, at: &[String]) -> String {
    format!(
        "{VALIDATION_ERROR} {FIELD_UNDEFINED}: Field '{field_name}' in type '{type_name}' \
         is undefined @ '{}'",
        at.join(QUERY_PATH_SEPARATOR)
    )
}

/// How a GraphQL validation failure is stated: the fixed opening of the sentence, the name
/// of the class of failure it is, and the separator the query path it was found at is
/// written with.
pub(super) const VALIDATION_ERROR: &str = "Validation error of type";
pub(super) const FIELD_UNDEFINED: &str = "FieldUndefined";
pub(super) const QUERY_PATH_SEPARATOR: &str = "/";
