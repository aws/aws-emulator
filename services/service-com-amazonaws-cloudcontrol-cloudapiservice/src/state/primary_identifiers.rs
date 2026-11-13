// SPDX-License-Identifier: CC0-1.0

//! How the handler that owns a resource type NAMES the resource inside its own diagnostic.
//!
//! A resource type's registry schema points `primaryIdentifier` at the property that
//! identifies one of its resources, and a request may carry that identifier either as the
//! bare value or as the schema's identifier map — "Primary identifiers can be specified as
//! a string or JSON" (`GetResource$Identifier`). Which of the two a HANDLER echoes back
//! when it reports a resource it could not find is that handler's own business: one built
//! on the generic resource-provider base class repeats the identifier the request carried,
//! while one that resolves that identifier against the type's schema first names the
//! resource as the schema's identifier map, `{"/properties/<property>":"<value>"}`.
//!
//! So the rendering is PER TYPE, and this module is where that is stated: the types whose
//! handler is recorded naming the resource as the map, with the property their schema's
//! `primaryIdentifier` points at. Every other type is named by the identifier the request
//! carried.

use super::*;

/// The resource types whose handler names the resource as the PRIMARY-IDENTIFIER OBJECT
/// the type's registry schema declares, with the property that schema's
/// `primaryIdentifier` points at.
///
/// Catalog source: the refusal each type's handler is RECORDED answering through this API.
/// A type absent from here is named by the identifier the request carried, which is what
/// the handlers recorded for the other types of this emulator do.
const RENDERED_AS_SCHEMA_MAP: &[(&str, &str)] = &[("AWS::Logs::LogGroup", "LogGroupName")];

/// The resource as the handler that owns `type_name` names it: the identifier the request
/// carried, or the type's primary-identifier object where that handler renders one
/// ([`RENDERED_AS_SCHEMA_MAP`]).
pub fn handler_rendered_identifier(type_name: &str, identifier: &str) -> String {
    let Some((_, property)) = RENDERED_AS_SCHEMA_MAP
        .iter()
        .find(|(name, _)| *name == type_name)
    else {
        return identifier.to_string();
    };
    // The map is written by the JSON writer, so it carries the separators JSON gives it
    // and no whitespace of its own.
    let mut map = ::serde_json::Map::new();
    map.insert(
        format!("{PROPERTY_POINTER_PREFIX}{property}"),
        Value::String(identifier.to_string()),
    );
    Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type whose handler names the resource as the schema's identifier map is rendered
    /// as that map, built from the identifier the request carried; every other type is
    /// named by that identifier itself.
    #[test]
    fn the_identifier_is_rendered_as_the_owning_handler_names_it() {
        let rows = [
            (
                "AWS::Logs::LogGroup",
                "app-logs",
                "{\"/properties/LogGroupName\":\"app-logs\"}",
            ),
            (
                "AWS::Logs::LogGroup",
                "/aws/lambda/fn",
                "{\"/properties/LogGroupName\":\"/aws/lambda/fn\"}",
            ),
            ("AWS::SQS::Queue", "https://queue/url", "https://queue/url"),
            ("AWS::Kinesis::Stream", "events", "events"),
        ];
        for (type_name, identifier, expected) in rows {
            let rendered = handler_rendered_identifier(type_name, identifier);
            assert_eq!(
                rendered, expected,
                "type {type_name:?} with {identifier:?}: expected {expected:?}, got \
                 {rendered:?}"
            );
        }
    }

    /// The map is the JSON form a request may carry an identifier in, so it reads back as
    /// one object of the schema's pointer and the value.
    #[test]
    fn the_rendered_map_is_the_schemas_pointer_and_the_value() {
        let rendered = handler_rendered_identifier("AWS::Logs::LogGroup", "app-logs");
        let parsed: Value = ::serde_json::from_str(&rendered)
            .expect("the rendered identifier must be the JSON the reader takes");
        assert_eq!(
            parsed,
            ::serde_json::json!({ "/properties/LogGroupName": "app-logs" }),
            "unexpected identifier map, got {rendered:?}"
        );
    }
}
