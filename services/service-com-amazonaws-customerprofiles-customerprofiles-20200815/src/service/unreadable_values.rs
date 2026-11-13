// SPDX-License-Identifier: CC0-1.0

//! The refusal that precedes every other one: a value in the request BODY whose JSON type
//! the modelled member it is bound to cannot be read from.
//!
//! Such a value never becomes an input at all, so it is not a constraint violation: the
//! member was never read, so no bound on it was reached. The sentence states the two facts
//! a caller has to reconcile — the CLASS of JSON value it sent and the modelled type that
//! value could not become ("NUMBER_VALUE can not be converted to a String") — and names the
//! class rather than the value, so a long value cannot be amplified into a long response.
//! Both halves of the sentence come from the tree's one statement of it
//! ([`service_common::json_wire_values`]), so the wording cannot drift from the other
//! services that answer it.
//!
//! Nothing about the model is written down here: the member a JSON key names, its wire
//! name, its request BINDING and its modelled target type all come off the operation's own
//! input schema, which is also the schema the codec reads the body with.
//!
//! WHAT IS LEFT TO THE CODEC, deliberately:
//!
//! * bytes that are not JSON at all — there is no value whose class could be named, so the
//!   codec's own refusal stands (restated in the modelled shape by
//!   [`super::modelled_refusal`], which is where the error class and the `Message` member
//!   are decided);
//! * a value nested inside a list, a map or a structure: a member `Schema` in this
//!   framework carries its target's KIND but not its target's own schema, so an element
//!   shape is not reachable from a top-level member (the codec passes the member's own
//!   schema down to its elements). Such a value is refused by the codec and restated the
//!   same way;
//! * an `@httpPayload` operation, whose body IS one member's value rather than the input
//!   object. This model binds no payload member today; the guard is here so one added later
//!   is not read as an input object.
//!
//! THE HOME for this walk: the services that put a deserialization layer in front of their
//! handlers each carry their own copy of it
//! (`grep -rln 'fn json_type_fits' services/*/src`), written before there was anywhere
//! shared to put it; the sentences they state are already shared, and migrating the walk
//! itself is that migration's own change rather than a fourth copy of the sentences.

use aws_smithy_schema::{Schema, ShapeType};
use serde_json::{Number, Value};
use service_common::json_wire_values::{coercion_message, json_value_class, member_by_wire_name};

/// The first value in `body` whose JSON type cannot be read into the modelled target type
/// of the member it is bound to, as the sentence naming it, or `None` when the body is one
/// `input_schema` can be read from.
///
/// An absent or empty body is `None`: a request may carry no body at all, and the members a
/// body would have held are then the validation layer's business.
#[must_use]
pub fn unreadable_value(input_schema: &Schema, body: &[u8]) -> Option<String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    if input_schema
        .members()
        .iter()
        .any(|member| member.http_payload().is_some())
    {
        return None;
    }
    // Not JSON at all names no value, so the codec's own refusal stands.
    let document: Value = serde_json::from_slice(body).ok()?;
    if !json_type_fits(input_schema.shape_type(), &document) {
        // The body of a modelled operation IS the input structure, so a body that is not
        // an object is a value that could not be read as one.
        return Some(coercion_message(
            json_value_class(&document),
            input_schema.shape_type(),
        ));
    }
    let Value::Object(members) = &document else {
        return None;
    };
    members
        .iter()
        // A key naming no member is ignored, exactly as the codec ignores it, and a member
        // read from the URI or a header carries no value in the body.
        .filter_map(|(key, held)| Some((member_by_wire_name(input_schema, key)?, held)))
        .filter(|(member, _)| !is_bound_to_request(member))
        .find_map(|(member, held)| {
            (!json_type_fits(member.shape_type(), held))
                .then(|| coercion_message(json_value_class(held), member.shape_type()))
        })
}

/// Whether a member is read from the request itself rather than from the body, so the body
/// carries no value for it.
fn is_bound_to_request(member: &Schema) -> bool {
    member.http_label().is_some()
        || member.http_query().is_some()
        || member.http_query_params().is_some()
        || member.http_header().is_some()
        || member.http_prefix_headers().is_some()
}

/// Whether a JSON value's type is one the modelled `target` type can be read from.
///
/// The classes are the Smithy ones: a number reads into a numeric shape, a string into a
/// string-shaped one (an enumerated type IS a string shape in these schemas, and a `blob`'s
/// base64 text is a string on the wire), an array into a list, an object into a map, a
/// structure or a union, and anything at all into a `document`. A timestamp reads from
/// either a number or a string, because `@timestampFormat` decides which. An integral shape
/// reads only a whole number its width holds: a fraction, or a value past that width, is no
/// more readable than a string. JSON `null` is absence rather than a type error — an absent
/// required member is the validation layer's refusal, not the codec's.
fn json_type_fits(target: ShapeType, value: &Value) -> bool {
    if matches!(target, ShapeType::Document) {
        return true;
    }
    match value {
        Value::Null => true,
        Value::Bool(_) => matches!(target, ShapeType::Boolean),
        Value::Number(number) => match target {
            ShapeType::Byte => fits_integral(number, i8::MIN.into(), i8::MAX.into()),
            ShapeType::Short => fits_integral(number, i16::MIN.into(), i16::MAX.into()),
            ShapeType::Integer => fits_integral(number, i32::MIN.into(), i32::MAX.into()),
            ShapeType::Long | ShapeType::BigInteger => fits_integral(number, i64::MIN, i64::MAX),
            ShapeType::Float
            | ShapeType::Double
            | ShapeType::BigDecimal
            | ShapeType::Timestamp => true,
            _ => false,
        },
        Value::String(_) => matches!(
            target,
            ShapeType::String
                | ShapeType::Blob
                | ShapeType::Timestamp
                | ShapeType::BigInteger
                | ShapeType::BigDecimal
        ),
        Value::Array(_) => matches!(target, ShapeType::List),
        Value::Object(_) => matches!(
            target,
            ShapeType::Map | ShapeType::Structure | ShapeType::Union
        ),
    }
}

/// Whether `number` is a whole number the inclusive range `min..=max` holds — the condition
/// an integral shape can be read from a JSON number under.
fn fits_integral(number: &Number, min: i64, max: i64) -> bool {
    number
        .as_i64()
        .is_some_and(|value| (min..=max).contains(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// RULE: a body value is readable exactly when its JSON class is one the modelled
    /// target type can be read from. The rows span every class against the target kinds
    /// this model binds, including the width of an integral shape.
    #[test]
    fn a_value_is_readable_only_from_the_classes_its_target_admits() {
        // (case, the value, the modelled target, readable?)
        let rows: &[(&str, Value, ShapeType, bool)] = &[
            ("a string into a string", json!("ada"), ShapeType::String, true),
            ("a number into a string", json!(17), ShapeType::Integer, true),
            ("a number into a string member", json!(17), ShapeType::String, false),
            ("a flag into a string member", json!(true), ShapeType::String, false),
            ("an object into a string member", json!({}), ShapeType::String, false),
            ("an array into a string member", json!([1]), ShapeType::String, false),
            ("a string into an integer", json!("90"), ShapeType::Integer, false),
            ("a fraction into an integer", json!(1.5), ShapeType::Integer, false),
            (
                "a number past an integer's width",
                json!(i64::from(i32::MAX) + 1),
                ShapeType::Integer,
                false,
            ),
            ("a flag into a boolean", json!(false), ShapeType::Boolean, true),
            ("an array into a list", json!(["a"]), ShapeType::List, true),
            ("an object into a list", json!({"a": 1}), ShapeType::List, false),
            ("an object into a map", json!({"a": "b"}), ShapeType::Map, true),
            ("an array into a map", json!(["a"]), ShapeType::Map, false),
            ("an object into a structure", json!({}), ShapeType::Structure, true),
            ("a number into a timestamp", json!(1), ShapeType::Timestamp, true),
            ("a string into a timestamp", json!("2026-01-01"), ShapeType::Timestamp, true),
            ("anything into a document", json!([1, "two"]), ShapeType::Document, true),
            ("null into any target", json!(null), ShapeType::Integer, true),
        ];
        for (case, value, target, expected) in rows {
            let actual = json_type_fits(*target, value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected json_type_fits({target:?}, {value}) == {expected}, \
                 got {actual}"
            );
        }
    }
}
