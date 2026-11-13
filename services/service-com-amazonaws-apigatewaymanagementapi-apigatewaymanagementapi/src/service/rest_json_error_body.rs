// SPDX-License-Identifier: CC0-1.0

//! What this service's ERROR body carries on the wire.
//!
//! RULE: this service speaks `restJson1` (`smithy_query(query="service_info")` reports
//! protocols `[restJson1]`), and a `restJson1` failure names its error SHAPE in the
//! `x-amzn-errortype` response header, so the BODY of a failure carries the error shape's own
//! modelled members and nothing else — no shape DISCRIMINATOR member. The `__type` member
//! belongs to the AWS JSON protocol family, which conveys the shape in the body because it
//! has no such header; a `restJson1` client resolves the shape from the header, and reading
//! an unmodelled member out of the body is not something a client generated from this model
//! can do.
//!
//! The consequence for THIS model is sharp: `GoneException` (`@httpError(410)`) and
//! `ForbiddenException` (`@httpError(403)`) declare NO members at all
//! (`smithy_query(query="shape", shape="GoneException,ForbiddenException")`), so there is
//! nothing to serialize into their bodies and a failure of one is the empty JSON document
//! `{}` exactly, with its identity in the header alone. A shape that DOES declare a member
//! carries that member and no discriminator beside it.
//!
//! WHY it matters here: a client that deserializes a 410 against the modelled, memberless
//! `GoneException` sees an unmodelled member; a strict parser rejects it, and anything that
//! round-trips or hashes the error body behaves differently against this emulator than
//! against AWS. It also teaches callers to read the error code out of a body AWS never
//! populates for these errors.
//!
//! WHERE IT IS APPLIED. The framework's shared `restJson1` renderer writes the `__type`
//! member for every service it serves (`vendor/smithy-schema-server/src/rest_json.rs`), and
//! that renderer is not this crate's to change, so the body is corrected at the seams this
//! service owns over the answers it writes:
//!
//! * [`smithy_server_runtime::Service::override_protocol`] hands the dispatch walk
//!   [`ConnectionsRestJson`], which serializes every failure a HANDLER raised;
//! * [`smithy_server_runtime::Service::override_routing_error`] answers a request this
//!   service claimed that names no modelled operation.
//!
//! One class is left as the framework writes it, and cannot be reached from a service seam: a
//! refusal rendered BEFORE a handler runs (an absent required member, a value outside a
//! modelled bound, an unauthenticated caller). The dispatch walk renders those from the
//! protocol's pre-dispatch ENVELOPE — a framework enum describing which protocol family
//! states the failure, not a response this crate is handed — and `override_refusal` chooses
//! only the code and the message inside it. The status, every header including
//! `x-amzn-errortype`, request deserialization and the whole SUCCESS path stay with the
//! framework renderer that owns them.
//!
//! The rewriting itself is `service_common::error_type_locator`'s — the one place in the tree
//! that owns where a rendered JSON failure names its shape — so this module is only this
//! service's protocol wrapper around it.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;

pub use ::service_common::error_type_locator::without_error_shape_in_body as with_served_error_body;

/// This service's wire protocol: `restJson1` exactly as the model declares it, with the error
/// body this service is recorded answering (see the module docs).
#[derive(Debug)]
pub struct ConnectionsRestJson {
    inner: RestJsonServerProtocol,
}

impl ConnectionsRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of it and
    /// must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated `ServiceDefinition`'s own protocol (whose table is the model's);
    /// an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error`.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for ConnectionsRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for ConnectionsRestJson {
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
        // Nothing about a REQUEST changes: it is read by the model's own codec.
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // A SUCCESS is the framework's own, byte for byte.
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
        let resp = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        Ok(with_served_error_body(resp))
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
    use super::with_served_error_body;
    use aws_smithy_types::body::SdkBody;
    use serde_json::Value;

    /// A response carrying `status` and `body`.
    fn response(status: u16, body: &str) -> aws_smithy_runtime_api::http::Response {
        aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// RULE over the bodies the framework renderer writes for THIS service's error shapes:
    /// the discriminator is gone and every modelled member survives with its value. The rows
    /// differ along what the rule generalises over — the error class, how many members the
    /// shape declares, and whether the discriminator carries a namespace. (What the shared
    /// rewriting does in general is pinned in `service_common::error_type_locator`.)
    #[test]
    fn an_error_body_carries_the_shapes_own_members_and_nothing_else() {
        let rows: &[(&str, &str, Value)] = &[
            (
                "the memberless GoneException",
                r#"{"__type":"GoneException"}"#,
                serde_json::json!({}),
            ),
            (
                "the memberless ForbiddenException",
                r#"{"__type":"ForbiddenException"}"#,
                serde_json::json!({}),
            ),
            (
                "the memberless LimitExceededException",
                r#"{"__type":"LimitExceededException"}"#,
                serde_json::json!({}),
            ),
            (
                "a refusal that carries a sentence",
                r#"{"__type":"BadRequestException","message":"Invalid connectionId: x"}"#,
                serde_json::json!({ "message": "Invalid connectionId: x" }),
            ),
            (
                "a discriminator carrying its namespace",
                r#"{"__type":"com.amazonaws.apigatewaymanagementapi#GoneException"}"#,
                serde_json::json!({}),
            ),
        ];

        for (case, rendered, expected) in rows {
            let answered = with_served_error_body(response(410, rendered));
            let body = answered.body().bytes().expect("an in-memory body");
            let document: Value = serde_json::from_slice(body)
                .unwrap_or_else(|e| panic!("case {case:?}: the body must be JSON ({e})"));
            assert_eq!(
                document, *expected,
                "case {case:?}: expected the body {expected}, got {document}"
            );
        }
    }

    /// A SUCCESS body is returned byte-identical, whatever members it carries — the rule is
    /// about a failure's envelope only.
    #[test]
    fn a_success_is_returned_untouched() {
        for (case, status, body) in [
            ("a reported connection", 200u16, r#"{"connectedAt":"x"}"#),
            ("a read", 200, r#"{"__type":"not an error at all"}"#),
        ] {
            let answered = with_served_error_body(response(status, body));
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: a success body must be byte-identical"
            );
        }
    }
}
