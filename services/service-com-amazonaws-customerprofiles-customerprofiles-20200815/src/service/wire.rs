// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` ERROR document reports a modelled exception's `Message`
//! member.
//!
//! RULE: an error document this service answers CARRIES its error shape's `Message` member —
//! the sentence the refusal words, or JSON `null` for a refusal that words none — so a caller
//! can tell "this refusal states no message" apart from "the member was not reported". The
//! two are different answers on the wire and different answers to an SDK: a JSON `null`
//! reaches a client as a message that is absent (`None`), while a key the document never
//! carried reaches it as the empty string the SDK substitutes for a key it did not find.
//!
//! Every exception this model declares carries exactly one optional member, `Message`
//! (`smithy_query(query="shape", shape="ResourceNotFoundException,BadRequestException")`),
//! and whether a refusal words a sentence is a property of the OPERATION that raises it:
//! the domain-scoped reads word an absent domain with a sentence naming it, while the
//! domain-configuration update (`PUT /domains/{DomainName}`) refuses an unknown domain with
//! the shape alone. The framework's generic serializer writes an absent optional member by
//! writing NOTHING, which is right for a success body and drops the key out of an error
//! document, so an operation that states no message could not be told from one whose message
//! went missing.
//!
//! [`MessageReportedRestJson`] is the protocol `service.rs` hands the dispatch sequence
//! through [`smithy_server_runtime::Service::override_protocol`], so EVERY modelled failure
//! of EVERY operation is reported the same way and no handler, shape or error constructor
//! mentions the rendering at all. Everything else — routing, request deserialization, success
//! bodies, the status, `__type`, `x-amzn-errortype` and an error whose message IS worded — is
//! the framework's own `restJson1` protocol, delegated to unchanged.
//!
//! A refusal rendered before any handler ran (a value outside a modelled bound, an
//! authentication failure) does not come through this seam, which is correct: that message is
//! built from the request and is never absent (see [`super::modelled_refusal`]).

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

/// The spellings an error document's message member can take: the one this model declares
/// for every exception it carries (`Message`), and the lowercase one other AWS JSON error
/// documents use for the same member, accepted so a shape that ever declares it is reported
/// under the same rule.
const MESSAGE_MEMBER_NAMES: &[&str] = &["Message", "message"];

/// The name of `error_schema`'s message member, or `None` for a shape that declares none.
///
/// Only a plain string member of the BODY qualifies: a member bound to a header, to the
/// query string or to the payload is not part of the JSON object this rule is about, and a
/// `null` written for it would say something else entirely.
fn message_member_name(error_schema: &Schema) -> Option<&'static str> {
    error_schema.members().iter().copied().find_map(|member| {
        let name = MESSAGE_MEMBER_NAMES
            .iter()
            .copied()
            .find(|name| member.member_name() == Some(name))?;
        (matches!(member.shape_type(), ShapeType::String)
            && member.http_header().is_none()
            && member.http_payload().is_none()
            && member.http_query().is_none())
        .then_some(name)
    })
}

/// `body` with `member` reported as JSON `null`, or `None` when there is nothing to report:
/// the body is not a JSON object, or it already carries the member — whatever value the
/// refusal worded, an empty string included, which is a WORDED message and not this case.
fn reporting_null_message(body: &[u8], member: &str) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    if object.contains_key(member) {
        return None;
    }
    object.insert(member.to_string(), Value::Null);
    serde_json::to_vec(&document).ok()
}

/// This service's wire protocol: `restJson1` exactly as the model declares it, with an error
/// document that always reports the error shape's `Message` member (see the module docs).
#[derive(Debug)]
pub struct MessageReportedRestJson {
    inner: RestJsonServerProtocol,
}

impl MessageReportedRestJson {
    /// Build the protocol once, at service construction: `override_protocol` hands back a
    /// borrow of it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated `ServiceDefinition`'s own protocol (whose table is the model's);
    /// an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error` — none of which consults
    /// routes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for MessageReportedRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for MessageReportedRestJson {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_output(output, output_schema, operation_name, success_status)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        let mut response = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        // A shape with no message member has nothing to report, and a body the renderer did
        // not hold in memory is left exactly as it wrote it.
        let Some(member) = message_member_name(error_schema) else {
            return Ok(response);
        };
        let Some(reported) = response
            .body()
            .bytes()
            .and_then(|bytes| reporting_null_message(bytes, member))
        else {
            return Ok(response);
        };
        *response.body_mut() = SdkBody::from(reported);
        Ok(response)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE over the bodies the renderer can hand this seam: a worded message survives
    /// verbatim — the empty string included, which is a worded message and not "none" — and
    /// a body that carries no member at all gains it as an explicit null.
    #[test]
    fn a_worded_message_survives_and_an_unworded_one_is_reported_as_null() {
        // (case, the body the renderer wrote, the value the member must read as after it)
        let table: [(&str, &str, Option<Value>); 5] = [
            (
                "no member at all",
                r#"{"__type":"ResourceNotFoundException"}"#,
                Some(Value::Null),
            ),
            (
                "a worded message",
                r#"{"__type":"ResourceNotFoundException","Message":"Domain d is not found"}"#,
                None,
            ),
            (
                "a worded empty message",
                r#"{"__type":"BadRequestException","Message":""}"#,
                None,
            ),
            (
                "the member already null",
                r#"{"__type":"ResourceNotFoundException","Message":null}"#,
                None,
            ),
            ("not a JSON object", r#"["ResourceNotFoundException"]"#, None),
        ];

        for (case, body, expected) in table {
            let reported = reporting_null_message(body.as_bytes(), "Message");
            match expected {
                None => assert!(
                    reported.is_none(),
                    "case {case:?}: expected the body to be left as it was, got {:?}",
                    reported.map(String::from_utf8)
                ),
                Some(value) => {
                    let bytes =
                        reported.unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
                    let document: Value = serde_json::from_slice(&bytes)
                        .unwrap_or_else(|error| panic!("case {case:?}: not JSON ({error})"));
                    assert_eq!(
                        document.get("Message"),
                        Some(&value),
                        "case {case:?}: expected the member to read as {value:?}, got {document}"
                    );
                }
            }
        }
    }

    /// The member is found under either spelling, and only for a plain string member of the
    /// body: the shapes this model declares all spell it `Message`.
    #[test]
    fn the_message_member_is_the_shapes_own_prose_member() {
        use crate::ops::types::{SCHEMA_BAD_REQUEST_EXCEPTION, SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION};

        for (case, schema) in [
            ("ResourceNotFoundException", &SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION),
            ("BadRequestException", &SCHEMA_BAD_REQUEST_EXCEPTION),
        ] {
            assert_eq!(
                message_member_name(schema),
                Some("Message"),
                "case {case:?}: expected the modelled prose member to be found"
            );
        }
    }
}
