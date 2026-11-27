// SPDX-License-Identifier: CC0-1.0

//! The VARIABLE-COERCION stage of the GraphQL data plane's request pipeline: what the values a
//! request carries in `variables` are checked against before the operation they parameterise is
//! executed.
//!
//! GraphQL coerces an operation's variable values against the variable definitions the
//! operation declares, and it does so BEFORE any field is resolved: "if the value provided for
//! a variable is not compatible with its declared type, a request error is raised". So a
//! request whose variables do not satisfy those definitions resolved nothing at all — it is
//! answered with the validation errors and a `data` of null ([`super::Answer::invalid`]), not
//! with a field that happens to resolve to nothing.
//!
//! Two failures of a variable value are what the definitions themselves decide:
//!
//! 1. a variable the request supplies NO value for (or an explicit null), whose declared type
//!    is non-null and which declares no default value — "if no value is provided … and the
//!    variable's type is non-null, a request error is raised";
//! 2. a value that cannot be COERCED to the type the definition declares.
//!
//! Everything a definition names BEYOND GraphQL's own type system — an enum or an input object
//! of the API's schema — is left to the execution: this stage resolves no type of the API, so
//! it judges a value only against the scalar the definition names.
//!
//! What it costs is bounded by the document that declared the variables: the definitions are
//! the operation's own, read under the parser's token bound, and the walk of one declared type
//! is bounded by the parser's recursion bound, so the errors reported are bounded by the
//! request that asked for them ([`super::MAX_FIELD_ERRORS`]).

use super::*;

/// The validation errors one request's variable values carry against `definitions`: one entry
/// per variable definition the request does not satisfy, at the position in the document that
/// definition was written at.
///
/// `source` is the document's own text, which each error's position is counted in
/// ([`DocumentLocation`]).
pub(super) fn invalid_variables(
    source: &str,
    definitions: Option<&::apollo_parser::cst::VariableDefinitions>,
    variables: &JsonMap<String, Json>,
) -> Vec<Json> {
    use ::apollo_parser::cst;

    let mut errors: Vec<Json> = Vec::new();
    let Some(definitions) = definitions else {
        return errors;
    };
    for definition in definitions.variable_definitions() {
        if errors.len() >= MAX_FIELD_ERRORS {
            return errors;
        }
        let Some(name) = definition
            .variable()
            .and_then(|variable| variable.name())
            .map(|name| name.text().to_string())
        else {
            continue;
        };
        let Some(declared) = crate::state::type_reference(definition.ty()) else {
            continue;
        };
        // The type as the DOCUMENT wrote it (`ID!`), which is how the refusal names it.
        let written = definition
            .ty()
            .map(|declared| cst::CstNode::syntax(&declared).text().to_string())
            .unwrap_or_default()
            .trim()
            .to_string();
        let location = DocumentLocation::of(source, cst::CstNode::syntax(&definition));
        // A variable the request carries no value for, and one it carries an explicit null
        // for, both have no value: "if a variable is not provided, it is considered to have no
        // value".
        let message = match variables.get(&name).filter(|value| !value.is_null()) {
            None => {
                let unsatisfied = definition.default_value().is_none()
                    && matches!(declared, crate::state::TypeRef::NonNull(_));
                unsatisfied.then(|| coerced_null_message(&name, &written))
            }
            Some(value) => uncoercible(value, &declared)
                .map(|(expected, refused)| invalid_value_message(&name, &expected, refused)),
        };
        let Some(message) = message else {
            continue;
        };
        let mut error = JsonMap::new();
        error.insert("message".to_string(), Json::String(message));
        super::document_location::located(location, &mut error);
        errors.push(Json::Object(error));
    }
    errors
}

/// The sentence a variable that has no value for a NON-NULL type is refused with: the
/// variable's own name and the type it was declared with, as the document wrote both.
fn coerced_null_message(name: &str, declared: &str) -> String {
    format!("Variable '{name}' has coerced Null value for NonNull type '{declared}'")
}

/// The sentence a value that cannot be coerced to the type it was supplied for is refused
/// with: the variable's own name, the type the value is being coerced to, and WHAT was
/// supplied instead ([`value_kind`]).
fn invalid_value_message(name: &str, expected: &str, value: &Json) -> String {
    format!(
        "Variable '{name}' has an invalid value : Expected type '{expected}' but was '{}'.",
        value_kind(value)
    )
}

/// The type the coercion of `value` to `declared` fails at and the value it fails ON, or
/// `None` when the value coerces.
///
/// The wrappers are GraphQL's own coercion rules: a non-null type takes no null, and a value
/// supplied for a LIST type that is not a list coerces as a list of one ("if the value passed
/// as an input to a list type is not a list …, the result of input coercion is a list of size
/// one") — so the value the coercion fails on is an ELEMENT of a list the request supplied
/// where the declaration nests one, rather than the list itself.
///
/// The recursion is bounded by the declared type itself, which was read under the parser's
/// recursion bound (`crate::state::DEFINITION_RECURSION_LIMIT`), so it goes no deeper than the
/// document's own nesting.
fn uncoercible<'a>(
    value: &'a Json,
    declared: &crate::state::TypeRef,
) -> Option<(String, &'a Json)> {
    use crate::state::TypeRef;

    match declared {
        TypeRef::NonNull(inner) => match value.is_null() {
            true => Some((named_type(inner), value)),
            false => uncoercible(value, inner),
        },
        TypeRef::List(inner) => match value {
            Json::Array(elements) => elements
                .iter()
                .find_map(|element| uncoercible(element, inner)),
            single => uncoercible(single, inner),
        },
        TypeRef::Named(name) => {
            (!coercible_scalar(value, name)).then(|| (name.clone(), value))
        }
    }
}

/// Whether `value` coerces to the type `name` names.
///
/// The rule is GraphQL's own input coercion for the five scalars it defines and AppSync's for
/// the ones it adds (`crate::state::BUILT_IN_SCALARS`): `Int` takes an integer "representable
/// as a 32-bit signed integer", `Float` any number, `String` a string, `Boolean` a boolean and
/// `ID` "either a string or an integer". AppSync's own scalars are each of those two forms,
/// and `AWSJSON` is "any valid JSON".
///
/// Every OTHER name is an enum or an input object of the API's schema, which this stage does
/// not resolve: a value supplied for one is left to the execution rather than refused here.
fn coercible_scalar(value: &Json, name: &str) -> bool {
    match name {
        "Int" => value
            .as_i64()
            .is_some_and(|number| i32::try_from(number).is_ok()),
        "Float" => value.is_number(),
        "String" | "AWSDate" | "AWSTime" | "AWSDateTime" | "AWSEmail" | "AWSURL" | "AWSPhone"
        | "AWSIPAddress" => value.is_string(),
        "Boolean" => value.is_boolean(),
        "ID" => value.is_string() || value.as_i64().is_some(),
        "AWSTimestamp" => value.as_i64().is_some(),
        _ => true,
    }
}

/// What a supplied value IS, as GraphQL names the forms a JSON value takes — which is what the
/// refusal states was supplied instead of the type it names.
fn value_kind(value: &Json) -> &'static str {
    match value {
        Json::Null => "Null",
        Json::Bool(_) => "Boolean",
        Json::Number(number) if number.is_f64() => "Float",
        Json::Number(_) => "Int",
        Json::String(_) => "String",
        Json::Array(_) => "List",
        Json::Object(_) => "Object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read `query`'s only operation and judge `variables` against the variables it declares,
    /// as the reading of a request does.
    fn judged(query: &str, variables: Json) -> Vec<Json> {
        let parsed = ::apollo_parser::Parser::new(query).parse();
        let document = parsed.document();
        let definitions = document
            .definitions()
            .find_map(|definition| match definition {
                ::apollo_parser::cst::Definition::OperationDefinition(operation) => {
                    operation.variable_definitions()
                }
                _ => None,
            });
        invalid_variables(
            query,
            definitions.as_ref(),
            &variables.as_object().cloned().unwrap_or_default(),
        )
    }

    /// A variable value is judged against the type its definition declares, over the forms a
    /// value takes and the wrappers a declaration carries.
    #[test]
    fn a_variable_value_is_judged_against_the_type_it_was_declared_with() {
        /// One row: what the operation declares, what the request carries, and the sentence
        /// the request earns (none for a request that satisfies the declaration).
        struct Row {
            case: &'static str,
            declares: &'static str,
            carries: Json,
            refused: Option<&'static str>,
        }
        // The rows differ along the form of the value, the type it is declared with and the
        // wrappers that declaration carries. The scalar names are
        // `crate::state::BUILT_IN_SCALARS`.
        let rows: &[Row] = &[
            Row {
                case: "a string for a non-null ID",
                declares: "$id: ID!",
                carries: json!({"id": "abc"}),
                refused: None,
            },
            Row {
                case: "an integer for a non-null ID, which an ID takes as well",
                declares: "$id: ID!",
                carries: json!({"id": 7}),
                refused: None,
            },
            Row {
                case: "nothing at all for a non-null ID",
                declares: "$id: ID!",
                carries: json!({}),
                refused: Some("Variable 'id' has coerced Null value for NonNull type 'ID!'"),
            },
            Row {
                case: "an explicit null for a non-null ID",
                declares: "$id: ID!",
                carries: json!({"id": Json::Null}),
                refused: Some("Variable 'id' has coerced Null value for NonNull type 'ID!'"),
            },
            Row {
                case: "nothing at all for a nullable ID",
                declares: "$id: ID",
                carries: json!({}),
                refused: None,
            },
            Row {
                case: "nothing at all for a non-null ID that declares a default",
                declares: "$id: ID! = \"seeded\"",
                carries: json!({}),
                refused: None,
            },
            Row {
                case: "an object for a non-null ID",
                declares: "$id: ID!",
                carries: json!({"id": {"not": "an ID"}}),
                refused: Some(
                    "Variable 'id' has an invalid value : Expected type 'ID' but was 'Object'.",
                ),
            },
            Row {
                case: "a boolean for a String",
                declares: "$title: String",
                carries: json!({"title": true}),
                refused: Some(
                    "Variable 'title' has an invalid value : Expected type 'String' but was \
                     'Boolean'.",
                ),
            },
            Row {
                case: "a fractional number for an Int",
                declares: "$count: Int",
                carries: json!({"count": 1.5}),
                refused: Some(
                    "Variable 'count' has an invalid value : Expected type 'Int' but was \
                     'Float'.",
                ),
            },
            Row {
                case: "an integer past what an Int represents",
                declares: "$count: Int",
                carries: json!({"count": 2_147_483_648_i64}),
                refused: Some(
                    "Variable 'count' has an invalid value : Expected type 'Int' but was 'Int'.",
                ),
            },
            Row {
                case: "an integer for a Float, which a Float takes",
                declares: "$ratio: Float",
                carries: json!({"ratio": 3}),
                refused: None,
            },
            Row {
                case: "a string for an AWSTimestamp",
                declares: "$at: AWSTimestamp",
                carries: json!({"at": "1970"}),
                refused: Some(
                    "Variable 'at' has an invalid value : Expected type 'AWSTimestamp' but was \
                     'String'.",
                ),
            },
            Row {
                case: "an object for AWSJSON, which takes any JSON",
                declares: "$document: AWSJSON",
                carries: json!({"document": {"any": ["json"]}}),
                refused: None,
            },
            Row {
                case: "a list of strings for a list of IDs",
                declares: "$ids: [ID!]",
                carries: json!({"ids": ["a", "b"]}),
                refused: None,
            },
            Row {
                case: "one string for a list of IDs, which coerces as a list of one",
                declares: "$ids: [ID!]",
                carries: json!({"ids": "a"}),
                refused: None,
            },
            Row {
                case: "a list holding a null for a list of non-null IDs",
                declares: "$ids: [ID!]",
                carries: json!({"ids": ["a", Json::Null]}),
                refused: Some(
                    "Variable 'ids' has an invalid value : Expected type 'ID' but was 'Null'.",
                ),
            },
            Row {
                case: "an object for an input type of the API's own schema",
                declares: "$note: NoteInput",
                carries: json!({"note": {"title": "a"}}),
                refused: None,
            },
        ];
        for row in rows {
            let query = format!("query Q({}) {{ echo }}", row.declares);
            let errors = judged(&query, row.carries.clone());
            let reported: Vec<&str> = errors
                .iter()
                .filter_map(|error| error["message"].as_str())
                .collect();
            let expected: Vec<&str> = row.refused.into_iter().collect();
            assert_eq!(
                reported, expected,
                "case {:?}: expected {expected:?}, got {errors:?}",
                row.case
            );
            // Every error names the position in the document it is about, and nothing else: a
            // request error carries no path and no error type.
            for error in &errors {
                let mut members: Vec<&str> = error
                    .as_object()
                    .map(|error| error.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                members.sort_unstable();
                assert_eq!(
                    members,
                    vec!["locations", "message"],
                    "case {:?}: expected the message and its locations alone, got {error:?}",
                    row.case
                );
                assert!(
                    error["locations"][0]["line"].as_u64() == Some(1)
                        && error["locations"][0]["column"].as_u64().is_some(),
                    "case {:?}: expected a position on the document's one line, got {error:?}",
                    row.case
                );
            }
        }
    }
}
