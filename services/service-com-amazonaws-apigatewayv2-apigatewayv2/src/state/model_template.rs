// SPDX-License-Identifier: CC0-1.0

//! Which of a model's declared data types a MAPPING TEMPLATE can be generated for, and the
//! sentence API Gateway refuses one it cannot with.
//!
//! A module of its own under [`crate::state`], because the question belongs to the model's
//! SCHEMA rather than to the operation that read it: `GetModelTemplate` "gets a model
//! template" — the Velocity skeleton that maps an incoming payload onto the structure the
//! model declares (`::service_common::model_mapping_template`) — so there has to BE a
//! structure to map. An `object` is mapped property by property and an `array` item by item;
//! a schema whose root declares a scalar type (`string`, `integer`, `number`, `boolean`,
//! `null`) declares no members at all, and API Gateway answers such a request by naming that
//! data type as one it does not generate a mapping template for.

/// The JSON Schema types a mapping template is generated for: the two COMPOSITE types, whose
/// members are what a template maps.
const TEMPLATED_DATA_TYPES: [&str; 2] = ["object", "array"];

/// How API Gateway words the refusal: the fixed text `Unsupported data type for mapping
/// template: ` and the data type the model's own schema declares.
pub fn unsupported_template_data_type_message(data_type: &str) -> String {
    format!("Unsupported data type for mapping template: {data_type}")
}

/// The message a model whose schema declares a data type no mapping template maps is refused
/// with, or `None` when a template can be generated for it.
///
/// `document` is the model's parsed JSON Schema. A schema that declares NO type at all names
/// no data type to refuse — the generator reads such a value's structure from its
/// `properties` or `items`, and maps a value of unknown shape by reference — so it is
/// generated for; a `type` written as a union of names is read as the generator reads it,
/// through the first name that is not `null`.
pub fn unsupported_template_data_type(document: &::serde_json::Value) -> Option<String> {
    let declared = declared_data_type(document)?;
    (!TEMPLATED_DATA_TYPES.contains(&declared))
        .then(|| unsupported_template_data_type_message(declared))
}

/// The data type a schema's root declares, read the way
/// `::service_common::model_mapping_template` reads it: the `type` keyword as a name, or the
/// first name of a union that is not `null` (JSON Schema draft-04 §5.5.2 admits both forms).
fn declared_data_type(document: &::serde_json::Value) -> Option<&str> {
    match document.get("type") {
        Some(::serde_json::Value::String(name)) => Some(name.as_str()),
        Some(::serde_json::Value::Array(names)) => names
            .iter()
            .filter_map(::serde_json::Value::as_str)
            .find(|name| *name != "null"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_is_generated_for_the_composite_types_and_for_an_untyped_schema() {
        // The rows span what the rule generalises over: each JSON Schema type the model's
        // own schema language declares (draft-04 §5.5.2: object, array, string, integer,
        // number, boolean, null), a type written as a union, and a schema that declares
        // none at all.
        let rows: &[(&str, ::serde_json::Value, Option<&str>)] = &[
            (
                "an object",
                ::serde_json::json!({ "type": "object", "properties": {} }),
                None,
            ),
            (
                "an array",
                ::serde_json::json!({ "type": "array", "items": { "type": "string" } }),
                None,
            ),
            (
                "no declared type at all",
                ::serde_json::json!({ "properties": { "id": { "type": "string" } } }),
                None,
            ),
            (
                "a union naming the object type",
                ::serde_json::json!({ "type": ["object", "null"] }),
                None,
            ),
            (
                "a union whose first name is null and whose second is composite",
                ::serde_json::json!({ "type": ["null", "array"] }),
                None,
            ),
            (
                "a string",
                ::serde_json::json!({ "type": "string" }),
                Some("string"),
            ),
            (
                "an integer",
                ::serde_json::json!({ "type": "integer" }),
                Some("integer"),
            ),
            (
                "a number",
                ::serde_json::json!({ "type": "number" }),
                Some("number"),
            ),
            (
                "a boolean",
                ::serde_json::json!({ "type": "boolean" }),
                Some("boolean"),
            ),
            (
                "null",
                ::serde_json::json!({ "type": "null" }),
                Some("null"),
            ),
            (
                "a union naming a scalar type",
                ::serde_json::json!({ "type": ["string", "null"] }),
                Some("string"),
            ),
            (
                "a union whose first name is null and whose second is a scalar",
                ::serde_json::json!({ "type": ["null", "string"] }),
                Some("string"),
            ),
        ];
        for (case, document, unsupported) in rows {
            let refused = unsupported_template_data_type(document);
            let expected = unsupported.map(unsupported_template_data_type_message);
            assert_eq!(
                refused.as_deref(),
                expected.as_deref(),
                "case {case:?}: expected unsupported={unsupported:?}, got {refused:?}"
            );
        }
    }
}
