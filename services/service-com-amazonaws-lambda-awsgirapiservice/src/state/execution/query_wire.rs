// SPDX-License-Identifier: CC0-1.0

//! Binding one AWS SDK call to the AWS QUERY wire, and reading that wire's answer back.
//!
//! Some AWS APIs are published over TWO wires. A service whose model declares an AWS JSON
//! protocol and carries the `awsQueryCompatible` modifier
//! (`smithy_query(query="service_info")`) kept the form-encoded `Action` surface it moved off,
//! because the SDK generations released before that move speak it: such a client sends a
//! `POST` of an `application/x-www-form-urlencoded` body whose `Action` field names the
//! operation and whose other fields carry the input, and reads an XML document back
//! (`<{Operation}Response><{Operation}Result>…`, `<ErrorResponse><Error><Code>…` for a
//! refusal). WHICH wire a run's calls travel on is the SDK the runtime it happens in bundles
//! ([`super::aws_api::BundledSdk`]); what that wire LOOKS like is this module.
//!
//! Nothing here is a per-operation table. The called service's own Smithy model — the same
//! file that service's emulator crate is generated from, embedded by [`super::aws_api`] — says
//! what every value is called on this wire and how a collection is laid out in it: a member
//! travels under `@xmlName` where it carries one and under its own name otherwise, a list
//! marked `@xmlFlattened` travels as `Name.N` and one that is not as `Name.member.N`, and a map
//! as `Name.N.<key>`/`Name.N.<value>` or `Name.entry.N.<key>`/`…`, with the key and value
//! elements named by the map's own members. Reading the answer is the same walk in reverse, so
//! what the guest reads back are the output shape's own member names — the SAME document the
//! JSON wire hands it — whatever the XML called each one.
//!
//! An operation the model does not hold, an input member it does not declare, or a value that
//! cannot be laid out is reported as a call that was NOT MADE, naming it, rather than sent with
//! a field guessed at or dropped: a dropped member would make a call that did something other
//! than what the handler asked for.
//!
//! What bounds these walks is the documents they walk. The input document is already capped in
//! size by the seam that carries it ([`super::aws_api::MAX_REQUEST_BYTES`]) and the answer by
//! [`super::aws_api::MAX_RESPONSE_BYTES`], and neither walk descends past [`MAX_DEPTH`]. The
//! JSON reading and writing are `serde_json`'s, the percent-encoding `percent-encoding`'s, and
//! the answer's XML is read into its elements by this tree's ONE reading of such a document
//! ([`::service_common::xml_document`]), with its own ceilings; the only thing written here is
//! which value belongs where, which is what the model states.

use ::std::collections::BTreeMap;
use ::std::sync::OnceLock;

use ::serde_json::{Map, Value};
use ::service_common::xml_document::{read as read_document, Element};

use super::aws_api;
use super::rest_json::{encoded, member_trait, target, Model};

/// The trait that says what a value is CALLED on this wire, where it is not called by its own
/// member name.
const XML_NAME: &str = "smithy.api#xmlName";

/// The trait that says a collection travels WITHOUT its wrapper element.
const XML_FLATTENED: &str = "smithy.api#xmlFlattened";

/// The trait a service's error shape carries the LEGACY code of the query protocol it moved
/// off in, and the field of it that holds that code.
const QUERY_ERROR: &str = "aws.protocols#awsQueryError";
const QUERY_ERROR_CODE: &str = "code";

/// The trait that makes a structure an ERROR shape.
const ERROR: &str = "smithy.api#error";

/// The element a list's elements travel under when the list is not flattened, and the one a
/// map's entries travel under: the Smithy defaults, used for a collection whose own members
/// name none.
const LIST_ENTRY: &str = "member";
const MAP_ENTRY: &str = "entry";

/// The elements a map entry's key and value travel under, for a map whose key and value
/// members carry no name of their own: the Smithy defaults.
const MAP_KEY: &str = "key";
const MAP_VALUE: &str = "value";

/// The elements this wire's own envelope carries: the result of a served call, and the error of
/// a refused one.
const RESULT_SUFFIX: &str = "Result";
const ERROR_ELEMENT: &str = "Error";
const CODE_ELEMENT: &str = "Code";
const MESSAGE_ELEMENT: &str = "Message";

/// How deep either walk over a MODEL descends.
///
/// A HOST bound, far below what any modelled shape nests to: the input document is the guest's
/// and the shape tree is the called service's, so how far a walk over either recurses is
/// neither's to decide. What bounds the answer DOCUMENT itself — how deep and how many
/// elements it may hold — is the shared reader's ([`::service_common::xml_document`]).
const MAX_DEPTH: usize = 32;

/// A value the model types as nothing, for the recursion steps that have no member traits to
/// read — a list's elements, which carry the element shape's layout and not a member's.
const NO_MEMBER: Value = Value::Null;

/// The parsed model of the service `token` names, parsed once per process per service.
///
/// One cell per entry of the reachable-services table ([`aws_api::APIS`]), exactly as every
/// other question answered out of a called service's model is held
/// ([`super::node_sdk_exports`]): a token no entry answers to parses nothing.
fn model(token: &str) -> Option<&'static Model> {
    static CACHE: OnceLock<Vec<OnceLock<Model>>> = OnceLock::new();
    let (at, source) = aws_api::held_model(token)?;
    let cache = CACHE.get_or_init(|| aws_api::APIS.iter().map(|_| OnceLock::new()).collect());
    let cell = cache.get(at)?;
    Some(cell.get_or_init(|| Model::parse_in_its_own_namespace(source)))
}

/// The name a value travels under on this wire: `@xmlName` where the member carries one, and
/// the name the caller holds for it otherwise.
fn wire_name<'a>(name: &'a str, member: &'a Value) -> &'a str {
    member_trait(member, XML_NAME)
        .and_then(Value::as_str)
        .unwrap_or(name)
}

/// Whether a collection travels WITHOUT its wrapper element: the trait may sit on the MEMBER
/// that carries the collection or on the collection shape itself, and either states the same
/// layout.
fn flattened(model: &Model, shape: &str, member: &Value) -> bool {
    member_trait(member, XML_FLATTENED).is_some()
        || model
            .shape(shape)
            .is_some_and(|shape| member_trait(shape, XML_FLATTENED).is_some())
}

/// Bind `operation` of the service `token` names, carrying `input` as the operation's input
/// document keyed by its own member names, to the form-encoded body this wire carries.
pub fn bound(token: &str, operation: &str, input: &Value) -> Result<Vec<u8>, String> {
    let model = model(token).ok_or_else(|| format!("no model of {token} is held here"))?;
    let shape = model
        .operation(operation)
        .ok_or_else(|| format!("{operation} is not an operation of this service"))?;
    let input_shape = shape
        .get("input")
        .and_then(|input| input.get("target"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut fields: Vec<(String, String)> = Vec::new();
    write_structure(model, &input_shape, input, None, 0, &mut fields)?;
    // The operation and the API version the request names: an AWS Query request names the
    // operation in its `Action` field and the version of the API it is for beside it, which is
    // the version the service's own model declares.
    let mut body = format!("Action={}", encoded(operation));
    if let Some(version) = api_version(model) {
        body.push_str(&format!("&Version={}", encoded(version)));
    }
    for (name, value) in fields {
        body.push('&');
        body.push_str(&encoded(&name));
        body.push('=');
        body.push_str(&encoded(&value));
    }
    Ok(body.into_bytes())
}

/// Lay the members of one structure out as fields of the form, under `prefix` for a structure
/// that is itself a member's value.
fn write_structure(
    model: &Model,
    shape: &str,
    value: &Value,
    prefix: Option<&str>,
    depth: usize,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("the input document nests deeper than one request may carry".to_string());
    }
    let carried = value.as_object().ok_or_else(|| {
        format!(
            "the value of {} is not a structure",
            prefix.unwrap_or("the input")
        )
    })?;
    let members = model.members(shape);
    // A member the operation does not declare is NOT dropped: the call would then do something
    // other than what the handler asked for, so it is reported as a call that was not made.
    let undeclared: Vec<&str> = carried
        .keys()
        .filter(|name| !members.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    if !undeclared.is_empty() {
        return Err(format!(
            "{} declares no {} member",
            shape.rsplit('#').next().unwrap_or(shape),
            undeclared.join(", no ")
        ));
    }
    for (name, member) in members {
        let Some(one) = carried.get(name).filter(|one| !one.is_null()) else {
            continue;
        };
        let wire = wire_name(name, member);
        let key = match prefix {
            Some(prefix) => format!("{prefix}.{wire}"),
            None => wire.to_string(),
        };
        write_value(model, target(member), member, one, &key, depth + 1, out)?;
    }
    Ok(())
}

/// Lay ONE value out as the field, or the run of fields, the wire carries it in.
fn write_value(
    model: &Model,
    shape: &str,
    member: &Value,
    value: &Value,
    key: &str,
    depth: usize,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("the input document nests deeper than one request may carry".to_string());
    }
    match model.kind(shape) {
        "structure" | "union" => {
            write_structure(model, shape, value, Some(key), depth, out)
        }
        "list" | "set" => {
            let elements = value
                .as_array()
                .ok_or_else(|| format!("the value of {key} is not a list"))?;
            let entry = model.shape(shape).and_then(|shape| shape.get("member"));
            let element_shape = entry.map(target).unwrap_or_default().to_string();
            let base = if flattened(model, shape, member) {
                key.to_string()
            } else {
                format!(
                    "{key}.{}",
                    entry.map_or(LIST_ENTRY, |entry| wire_name(LIST_ENTRY, entry))
                )
            };
            for (at, one) in elements.iter().enumerate() {
                write_value(
                    model,
                    &element_shape,
                    &NO_MEMBER,
                    one,
                    &format!("{base}.{}", at + 1),
                    depth + 1,
                    out,
                )?;
            }
            Ok(())
        }
        "map" => {
            let entries = value
                .as_object()
                .ok_or_else(|| format!("the value of {key} is not a map"))?;
            let shape_held = model.shape(shape);
            let key_member = shape_held.and_then(|shape| shape.get("key"));
            let value_member = shape_held.and_then(|shape| shape.get("value"));
            let key_name = key_member.map_or(MAP_KEY, |member| wire_name(MAP_KEY, member));
            let value_name =
                value_member.map_or(MAP_VALUE, |member| wire_name(MAP_VALUE, member));
            let value_shape = value_member.map(target).unwrap_or_default().to_string();
            let flat = flattened(model, shape, member);
            for (at, (entry_key, entry_value)) in entries.iter().enumerate() {
                let base = if flat {
                    format!("{key}.{}", at + 1)
                } else {
                    format!("{key}.{MAP_ENTRY}.{}", at + 1)
                };
                out.push((format!("{base}.{key_name}"), entry_key.clone()));
                write_value(
                    model,
                    &value_shape,
                    value_member.unwrap_or(&NO_MEMBER),
                    entry_value,
                    &format!("{base}.{value_name}"),
                    depth + 1,
                    out,
                )?;
            }
            Ok(())
        }
        kind => {
            out.push((key.to_string(), scalar_text(kind, value, key)?));
            Ok(())
        }
    }
}

/// One scalar as the text this wire carries it as.
///
/// A blob arrives from the SDK the guest calls as the base64 of its bytes
/// ([`super::aws_sdk_guest`]), which is the text this wire carries one as, so it travels as it
/// is. A TIMESTAMP travels as the date-time text the query protocols carry one in — their own
/// default form, where a JSON protocol carries the seconds since the epoch — so a value the
/// guest handed over as those seconds is stated in that form.
fn scalar_text(kind: &str, value: &Value, key: &str) -> Result<String, String> {
    match (kind, value) {
        ("timestamp", Value::Number(seconds)) => seconds
            .as_f64()
            .map(::aws_smithy_types::DateTime::from_secs_f64)
            .and_then(|instant| {
                instant
                    .fmt(::aws_smithy_types::date_time::Format::DateTime)
                    .ok()
            })
            .ok_or_else(|| format!("the value of {key} is not an instant")),
        (_, Value::String(text)) => Ok(text.clone()),
        (_, Value::Number(number)) => Ok(number.to_string()),
        (_, Value::Bool(held)) => Ok(held.to_string()),
        (_, _) => Err(format!("the value of {key} is not a {kind}")),
    }
}

/// The output document of one answered call, keyed by the output shape's OWN member names —
/// the same document the JSON wire of the same API hands the guest, so what an SDK reads back
/// does not depend on the wire its call travelled.
///
/// An answer this walk cannot read — an operation the model does not hold, a document that is
/// not this wire's envelope — is answered as the empty document rather than as text no SDK can
/// read: the call WAS served, and nothing of the answer is invented for it.
pub fn read(token: &str, operation: &str, body: &str) -> String {
    let empty = || "{}".to_string();
    let Some(model) = model(token) else {
        return empty();
    };
    let Some(shape) = model.operation(operation) else {
        return empty();
    };
    let output_shape = shape
        .get("output")
        .and_then(|output| output.get("target"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if model.members(output_shape).is_empty() {
        return empty();
    }
    let Ok(root) = read_document(body.as_bytes()) else {
        return empty();
    };
    // The members sit inside this wire's own result element; a document that carries them at
    // its root is read there, so an envelope without it is read the same way.
    let result = format!("{operation}{RESULT_SUFFIX}");
    let carried = root.child(&result).unwrap_or(&root);
    Value::Object(structure_of(model, output_shape, carried, 0)).to_string()
}

/// What a REFUSAL this wire carries states: the error CODE it names the class by, and the
/// message it words it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The `<Code>` element: the LEGACY code of the query protocol, which is the code an SDK
    /// of this generation reports a refusal under.
    pub code: String,
    /// The `<Message>` element, as the service worded it.
    pub message: String,
    /// Every other element the error carries, which are the members of the error SHAPE the
    /// service declares for the class.
    pub members: BTreeMap<String, String>,
}

/// The refusal an answer of this wire carries, or `None` for a body that is not one.
///
/// This protocol's error envelope is `<ErrorResponse><Error>…</Error>…`, so the error is the
/// root's own `Error` element — and a document that IS that element is read the same way.
pub fn refusal(body: &str) -> Option<Refused> {
    let root = read_document(body.as_bytes()).ok()?;
    let error = match root.name == ERROR_ELEMENT {
        true => &root,
        false => root.child(ERROR_ELEMENT)?,
    };
    let mut members: BTreeMap<String, String> = BTreeMap::new();
    let mut code = String::new();
    let mut message = String::new();
    for child in &error.children {
        match child.name.as_str() {
            CODE_ELEMENT => code = child.text.clone(),
            MESSAGE_ELEMENT => message = child.text.clone(),
            _ => {
                members.insert(child.name.clone(), child.text.clone());
            }
        }
    }
    (!code.is_empty()).then_some(Refused {
        code,
        message,
        members,
    })
}

/// The error SHAPE of the service `token` names whose class is named by the legacy code
/// `code` — the shape an SDK of this generation raises the refusal as.
///
/// Read off the model: the shape is the one whose `awsQueryError` trait carries that code.
/// `None` where the service declares no such shape, in which case the code is all the answer
/// states about the class.
pub fn error_shape(token: &str, code: &str) -> Option<String> {
    error_named_by_query_code(model(token)?, code).map(ToString::to_string)
}

/// The API VERSION a request on this wire names: the version the model's own service shape
/// declares.
fn api_version(model: &Model) -> Option<&str> {
    model.shapes().values().find_map(|shape| {
        (shape.get("type").and_then(Value::as_str) == Some("service"))
            .then(|| shape.get("version").and_then(Value::as_str))
            .flatten()
    })
}

/// The local name of the model's error shape whose class this wire names by `code` — the shape
/// whose `awsQueryError` trait carries it.
fn error_named_by_query_code<'a>(model: &'a Model, code: &str) -> Option<&'a str> {
    model.shapes().iter().find_map(|(id, shape)| {
        let named = member_trait(shape, QUERY_ERROR)
            .and_then(|error| error.get(QUERY_ERROR_CODE))
            .and_then(Value::as_str)
            == Some(code);
        (named && member_trait(shape, ERROR).is_some())
            .then(|| id.rsplit('#').next())
            .flatten()
    })
}

/// The members of one structure, read out of the element that carries them.
fn structure_of(model: &Model, shape: &str, element: &Element, depth: usize) -> Map<String, Value> {
    let mut read = Map::new();
    if depth > MAX_DEPTH {
        return read;
    }
    for (name, member) in model.members(shape) {
        let wire = wire_name(name, member);
        let member_shape = target(member);
        match model.kind(member_shape) {
            "list" | "set" => {
                let elements = list_elements(model, member_shape, member, element, wire);
                if elements.is_empty() {
                    continue;
                }
                let entry_shape = model
                    .shape(member_shape)
                    .and_then(|shape| shape.get("member"))
                    .map(target)
                    .unwrap_or_default()
                    .to_string();
                read.insert(
                    name.clone(),
                    Value::Array(
                        elements
                            .into_iter()
                            .map(|one| value_of(model, &entry_shape, one, depth + 1))
                            .collect(),
                    ),
                );
            }
            "map" => {
                let entries = map_entries(model, member_shape, member, element, wire);
                if entries.is_empty() {
                    continue;
                }
                read.insert(
                    name.clone(),
                    Value::Object(map_of(model, member_shape, entries, depth + 1)),
                );
            }
            _ => {
                if let Some(child) = element.child(wire) {
                    read.insert(
                        name.clone(),
                        value_of(model, member_shape, child, depth + 1),
                    );
                }
            }
        }
    }
    read
}

/// The elements of one list, wherever its layout puts them: directly under the element that
/// carries the member for a flattened list, and inside the member's own element for one that is
/// not.
fn list_elements<'a>(
    model: &Model,
    shape: &str,
    member: &Value,
    element: &'a Element,
    wire: &str,
) -> Vec<&'a Element> {
    if flattened(model, shape, member) {
        return named(element, wire);
    }
    let Some(wrapper) = element.child(wire) else {
        return Vec::new();
    };
    let entry = model
        .shape(shape)
        .and_then(|shape| shape.get("member"))
        .map_or(LIST_ENTRY.to_string(), |entry| {
            wire_name(LIST_ENTRY, entry).to_string()
        });
    named(wrapper, &entry)
}

/// Every element of `element` called `name`, in document order.
///
/// The shared reader's own iterator over them ties the NAME's lifetime to the element's
/// ([`Element::each`]), which a name this walk builds from the model does not share, so the
/// run is collected here instead.
fn named<'a>(element: &'a Element, name: &str) -> Vec<&'a Element> {
    element
        .children
        .iter()
        .filter(|child| child.name == name)
        .collect()
}

/// The entries of one map, the same way.
fn map_entries<'a>(
    model: &Model,
    shape: &str,
    member: &Value,
    element: &'a Element,
    wire: &str,
) -> Vec<&'a Element> {
    if flattened(model, shape, member) {
        return named(element, wire);
    }
    let Some(wrapper) = element.child(wire) else {
        return Vec::new();
    };
    wrapper.each(MAP_ENTRY).collect()
}

/// One map, read out of its entries: each entry's key element names the entry and its value
/// element carries the value.
fn map_of(
    model: &Model,
    shape: &str,
    entries: Vec<&Element>,
    depth: usize,
) -> Map<String, Value> {
    let held = model.shape(shape);
    let key_name = held
        .and_then(|shape| shape.get("key"))
        .map_or(MAP_KEY.to_string(), |member| {
            wire_name(MAP_KEY, member).to_string()
        });
    let value_member = held.and_then(|shape| shape.get("value"));
    let value_name = value_member.map_or(MAP_VALUE.to_string(), |member| {
        wire_name(MAP_VALUE, member).to_string()
    });
    let value_shape = value_member.map(target).unwrap_or_default().to_string();
    let mut read = Map::new();
    for entry in entries {
        let Some(key) = entry.child(&key_name) else {
            continue;
        };
        let Some(value) = entry.child(&value_name) else {
            continue;
        };
        read.insert(
            key.text.clone(),
            value_of(model, &value_shape, value, depth + 1),
        );
    }
    read
}

/// ONE value, read as the type the model gives its shape.
///
/// A number, a boolean and an instant are read as what they are rather than as the text the
/// document carried them as, because that is the form the SDK the guest calls hands a caller
/// one in — the same form the JSON wire of the same API carries it in, so an answer does not
/// change shape with the wire it travelled.
fn value_of(model: &Model, shape: &str, element: &Element, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    match model.kind(shape) {
        "structure" | "union" => Value::Object(structure_of(model, shape, element, depth + 1)),
        "list" | "set" => {
            let entry = model.shape(shape).and_then(|shape| shape.get("member"));
            let entry_name = entry.map_or(LIST_ENTRY.to_string(), |entry| {
                wire_name(LIST_ENTRY, entry).to_string()
            });
            let entry_shape = entry.map(target).unwrap_or_default().to_string();
            Value::Array(
                named(element, &entry_name)
                    .into_iter()
                    .map(|one| value_of(model, &entry_shape, one, depth + 1))
                    .collect(),
            )
        }
        "map" => Value::Object(map_of(
            model,
            shape,
            element.each(MAP_ENTRY).collect(),
            depth + 1,
        )),
        "integer" | "long" | "short" | "byte" => element
            .text
            .trim()
            .parse::<i64>()
            .map_or_else(|_| Value::String(element.text.clone()), Value::from),
        "float" | "double" => element
            .text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(|| Value::String(element.text.clone()), Value::Number),
        "boolean" => match element.text.trim() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(element.text.clone()),
        },
        "timestamp" => ::aws_smithy_types::DateTime::from_str(
            element.text.trim(),
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
        .and_then(|instant| ::serde_json::Number::from_f64(instant.as_secs_f64()))
        .map_or_else(|| Value::String(element.text.clone()), Value::Number),
        _ => Value::String(element.text.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny model standing for the shapes this wire lays out: one operation per class of
    /// layout the protocol has.
    const MODEL: &str = r#"{
      "smithy": "2.0",
      "shapes": {
        "probe#Probe": {
          "type": "service",
          "version": "2012-11-05",
          "operations": [{ "target": "probe#Send" }]
        },
        "probe#Send": {
          "type": "operation",
          "input": { "target": "probe#SendRequest" },
          "output": { "target": "probe#SendResult" },
          "errors": [{ "target": "probe#Absent" }]
        },
        "probe#SendRequest": {
          "type": "structure",
          "members": {
            "Url": { "target": "smithy.api#String" },
            "Delay": { "target": "smithy.api#Integer" },
            "Names": {
              "target": "probe#Names",
              "traits": {
                "smithy.api#xmlName": "Name",
                "smithy.api#xmlFlattened": {}
              }
            },
            "Wrapped": { "target": "probe#Names" },
            "Attributes": {
              "target": "probe#Attributes",
              "traits": {
                "smithy.api#xmlName": "Attribute",
                "smithy.api#xmlFlattened": {}
              }
            },
            "Entries": {
              "target": "probe#Entries",
              "traits": {
                "smithy.api#xmlName": "Entry",
                "smithy.api#xmlFlattened": {}
              }
            }
          }
        },
        "probe#SendResult": {
          "type": "structure",
          "members": {
            "Id": { "target": "smithy.api#String" },
            "Count": { "target": "smithy.api#Integer" },
            "Ready": { "target": "smithy.api#Boolean" },
            "Messages": {
              "target": "probe#Entries",
              "traits": {
                "smithy.api#xmlName": "Message",
                "smithy.api#xmlFlattened": {}
              }
            },
            "Attributes": {
              "target": "probe#Attributes",
              "traits": {
                "smithy.api#xmlName": "Attribute",
                "smithy.api#xmlFlattened": {}
              }
            }
          }
        },
        "probe#Names": { "type": "list", "member": { "target": "smithy.api#String" } },
        "probe#Entries": { "type": "list", "member": { "target": "probe#Entry" } },
        "probe#Entry": {
          "type": "structure",
          "members": {
            "Body": { "target": "smithy.api#String" },
            "Size": { "target": "smithy.api#Integer" }
          }
        },
        "probe#Attributes": {
          "type": "map",
          "key": { "target": "smithy.api#String", "traits": { "smithy.api#xmlName": "Name" } },
          "value": { "target": "smithy.api#String", "traits": { "smithy.api#xmlName": "Value" } }
        },
        "probe#Absent": {
          "type": "structure",
          "members": { "message": { "target": "smithy.api#String" } },
          "traits": {
            "smithy.api#error": "client",
            "aws.protocols#awsQueryError": { "code": "Probe.NoSuchThing", "httpResponseCode": 400 }
          }
        }
      }
    }"#;

    /// The parsed probe model.
    fn probe() -> Model {
        Model::parse_in_its_own_namespace(MODEL)
    }

    /// The fields of one laid-out request, as `name=value` pairs in the order written.
    fn fields(input: &Value) -> Vec<String> {
        let model = probe();
        let mut out: Vec<(String, String)> = Vec::new();
        write_structure(&model, "probe#SendRequest", input, None, 0, &mut out)
            .expect("the probe input lays out");
        out.into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect()
    }

    #[test]
    fn every_class_of_value_is_laid_out_where_the_model_says() {
        // (case, the input a client sent, the fields the wire must carry)
        let rows: Vec<(&str, Value, Vec<&str>)> = vec![
            (
                "a scalar and a number",
                ::serde_json::json!({ "Url": "https://queue/one", "Delay": 10 }),
                vec!["Delay=10", "Url=https://queue/one"],
            ),
            (
                "a flattened list",
                ::serde_json::json!({ "Names": ["All", "Policy"] }),
                vec!["Name.1=All", "Name.2=Policy"],
            ),
            (
                "a list that is not flattened",
                ::serde_json::json!({ "Wrapped": ["one"] }),
                vec!["Wrapped.member.1=one"],
            ),
            (
                "a flattened map whose key and value are named",
                ::serde_json::json!({ "Attributes": { "VisibilityTimeout": "30" } }),
                vec!["Attribute.1.Name=VisibilityTimeout", "Attribute.1.Value=30"],
            ),
            (
                "a flattened list of structures",
                ::serde_json::json!({ "Entries": [{ "Body": "hello", "Size": 5 }] }),
                vec!["Entry.1.Body=hello", "Entry.1.Size=5"],
            ),
        ];

        for (case, input, expected) in rows {
            let laid_out = fields(&input);
            let expected: Vec<String> = expected.into_iter().map(ToString::to_string).collect();
            let mut sorted = laid_out.clone();
            sorted.sort();
            let mut wanted = expected.clone();
            wanted.sort();
            assert_eq!(
                sorted, wanted,
                "case {case}: expected the fields {wanted:?}, got {laid_out:?}"
            );
        }
    }

    #[test]
    fn a_member_the_operation_does_not_declare_is_no_call_at_all() {
        let input = ::serde_json::json!({ "Url": "https://queue/one", "Nonsense": "x" });
        let model = probe();
        let mut out: Vec<(String, String)> = Vec::new();
        let refused = write_structure(&model, "probe#SendRequest", &input, None, 0, &mut out);
        assert!(
            refused
                .as_ref()
                .err()
                .is_some_and(|detail| detail.contains("Nonsense")),
            "case an undeclared member: expected the layout to name it, got {refused:?}"
        );
    }

    #[test]
    fn the_answers_members_are_read_back_under_their_own_names() {
        let body = "<SendResponse><SendResult><Id>m-1</Id><Count>2</Count><Ready>true</Ready>\
                    <Message><Body>a &amp; b</Body><Size>5</Size></Message>\
                    <Message><Body>second</Body></Message>\
                    <Attribute><Name>SentTimestamp</Name><Value>17</Value></Attribute>\
                    </SendResult><ResponseMetadata><RequestId>r-1</RequestId></ResponseMetadata>\
                    </SendResponse>";
        let model = probe();
        let root = read_document(body.as_bytes()).expect("the probe answer parses");
        let result = root
            .child("SendResult")
            .expect("the answer carries its result element");
        let read = Value::Object(structure_of(&model, "probe#SendResult", result, 0));
        assert_eq!(
            read,
            ::serde_json::json!({
                "Id": "m-1",
                "Count": 2,
                "Ready": true,
                "Messages": [{ "Body": "a & b", "Size": 5 }, { "Body": "second" }],
                "Attributes": { "SentTimestamp": "17" },
            }),
            "case the probe answer: expected the output shape's own members, got {read}"
        );
    }

    #[test]
    fn a_refusal_of_this_wire_states_its_code_and_its_sentence() {
        let body = "<ErrorResponse><Error><Type>Sender</Type><Code>Probe.NoSuchThing</Code>\
                    <Message>The specified thing does not exist.</Message></Error>\
                    <RequestId>r-2</RequestId></ErrorResponse>";
        let refused = refusal(body).expect("the probe refusal parses");
        assert_eq!(
            (refused.code.as_str(), refused.message.as_str()),
            (
                "Probe.NoSuchThing",
                "The specified thing does not exist."
            ),
            "case the probe refusal: expected the code and the sentence the document carries, \
             got {refused:?}"
        );
        assert_eq!(
            error_named_by_query_code(&probe(), "Probe.NoSuchThing"),
            Some("Absent"),
            "case the probe refusal: the model names the shape the code belongs to"
        );
    }

    #[test]
    fn an_answer_this_wire_did_not_word_carries_no_refusal() {
        // (case, the text an answer carried)
        let rows = [
            ("text that is no document at all", "not xml at all <"),
            ("an element left open", "<ErrorResponse><Error>"),
            ("a document of another shape", "<SendResponse><SendResult/></SendResponse>"),
            (
                "an envelope naming no code",
                "<ErrorResponse><Error><Type>Sender</Type></Error></ErrorResponse>",
            ),
        ];
        for (case, body) in rows {
            assert!(
                refusal(body).is_none(),
                "case {case}: expected no refusal at all, got {:?}",
                refusal(body)
            );
        }
    }
}
