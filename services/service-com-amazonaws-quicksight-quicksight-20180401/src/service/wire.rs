// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` answers report the request id of a FAILED call.
//!
//! RULE: EVERY failure of this service reports the request id of the call in the
//! `x-amzn-RequestId` response header — this module's half of the rule, and the one place
//! every refusal of every operation states it. Whether the error DOCUMENT states that same
//! id beside the members the class fills depends on the surface the call was addressed to,
//! and is owned by [`super::error_document_request_id`], at the seam that can see the
//! request. Every exception shape of this model DECLARES a `RequestId` member
//! (`smithy_query(query="shape",
//! shape="ResourceNotFoundException,ResourceExistsException,LimitExceededException",
//! model_path="services/service-com-amazonaws-quicksight-quicksight-20180401/smithy-model/com-amazonaws-quicksight-quicksight-20180401.json")`),
//! and it is OPTIONAL in every one of them, so a refusal stating it and a refusal leaving
//! it out are both documents the model allows. A SUCCESS is the other half of the contract
//! and is untouched here: an output shape declares `RequestId` as a body member and an
//! answer that succeeds states it there.
//!
//! The framework's generic serializer writes the document — every member the handler set,
//! the request id among them — and knows nothing of the header, and `vendor/` is read-only,
//! so this half of the rule lives at the seam a service owns over the answers it
//! writes: [`RequestIdMirroredRestJson`] is the protocol `service.rs` hands the dispatch
//! sequence through [`smithy_server_runtime::Service::override_protocol`], so EVERY modelled
//! failure of EVERY operation goes out the same way and no handler, shape declaration or
//! error constructor mentions the rendering at all. The id a handler allocated is TAKEN OFF
//! the document as it is written and put in the header instead, never re-minted here, so the
//! header can only ever report the id allocated for this very call; an error shape that
//! declares no such member is rendered exactly as the framework writes it and reports no id
//! anywhere.
//!
//! Everything else is the framework's own `restJson1` protocol, delegated to unchanged:
//! routing, request deserialization, success bodies, the status, `x-amzn-errortype` and
//! every member of the error document. One class of failure is not reachable from this seam
//! and is left as the framework writes it: a refusal rendered BEFORE a handler runs (an
//! absent required member, a value outside a modelled bound, an unauthenticated caller),
//! whose synthetic shape has no request-id member in the first place.

use std::cell::RefCell;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The response header AWS reports the request id of a FAILED call in, and the place EVERY
/// such answer of this service reports it.
///
/// `pub(super)` because the document half of the rule restates the id this header reports
/// rather than minting a second one ([`super::error_document_request_id`]).
pub(super) const REQUEST_ID_HEADER: &str = "x-amzn-RequestId";

/// The member this model's exception shapes declare the request id under — the one the
/// document half of the rule states it in too.
pub(super) const REQUEST_ID_MEMBER: &str = "RequestId";

/// The error shape's request-id member, or `None` for a shape that declares none.
///
/// Only a plain string member of the BODY qualifies: a member the model binds to a header,
/// to the query string or to the payload is not part of the JSON object this rule is about,
/// and one already bound to a header is already reported where the rule wants it.
fn request_id_member(error_schema: &Schema) -> Option<&Schema> {
    error_schema.members().iter().copied().find(|member| {
        member
            .member_name()
            .is_some_and(|name| name == REQUEST_ID_MEMBER)
            && matches!(member.shape_type(), ShapeType::String)
            && member.http_header().is_none()
            && member.http_payload().is_none()
            && member.http_query().is_none()
    })
}

/// An error structure serialized as the framework writes it except for the request-id
/// member, whose value is noted in `observed` and left out of the document, so the response
/// header is where the id is reported.
struct RequestIdObserved<'a> {
    inner: &'a dyn SerializableStruct,
    request_id: &'a Schema,
    observed: &'a RefCell<Option<String>>,
}

impl SerializableStruct for RequestIdObserved<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        let mut observing = Observing {
            inner: &mut *serializer,
            member_name: self.request_id.member_name(),
            observed: self.observed,
        };
        self.inner.serialize_members(&mut observing)
    }
}

/// The serializer an error's members are written through: every write is the wrapped
/// serializer's, unchanged — the write of the request-id member is noted and WITHHELD, so
/// the value is reported in the response header rather than in the document.
struct Observing<'a> {
    inner: &'a mut dyn ShapeSerializer,
    member_name: Option<&'a str>,
    observed: &'a RefCell<Option<String>>,
}

impl Observing<'_> {
    /// Whether `schema` is the member whose value the header reports.
    fn observes(&self, schema: &Schema) -> bool {
        self.member_name.is_some() && schema.member_name() == self.member_name
    }
}

impl ShapeSerializer for Observing<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, value)
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, write_entries)
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if self.observes(schema) {
            // The id the handler allocated for this call: noted here, and NOT passed to the
            // wrapped serializer, so the document comes out as the class's own members and
            // the id is reported in the response header instead.
            *self.observed.borrow_mut() = Some(value.to_string());
            return Ok(());
        }
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}

/// The `restJson1` codec that reports a failure's request id — the one the handler set on
/// the member the model declares — in the `x-amzn-RequestId` response header rather than
/// from the document it was written on: the model's own protocol in every other respect.
///
/// The document the id is taken off is what the header then reports, so a surface whose
/// refusals DO state the id in their document restates this header's value
/// ([`super::error_document_request_id`]) and the two can never name different ids.
#[derive(Debug)]
pub struct RequestIdMirroredRestJson {
    inner: RestJsonServerProtocol,
}

impl RequestIdMirroredRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of it and
    /// must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated service definition's own protocol (whose table is the
    /// model's); an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for RequestIdMirroredRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for RequestIdMirroredRestJson {
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
        // A SUCCESS is the framework's own, byte for byte: an output shape declares its
        // `RequestId` as a body member and the answer carries it there.
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
        let Some(request_id) = request_id_member(error_schema) else {
            // A shape with no request-id member reports none in its header either.
            return self
                .inner
                .serialize_error(error, error_schema, error_code, http_status);
        };
        let observed: RefCell<Option<String>> = RefCell::new(None);
        let mut response = self.inner.serialize_error(
            &RequestIdObserved {
                inner: error,
                request_id,
                observed: &observed,
            },
            error_schema,
            error_code,
            http_status,
        )?;
        // Dropped rather than fatal if the HTTP layer cannot carry the value: `try_insert`
        // is the fallible half of the insert the framework itself uses on the error path,
        // where a panic would become a 500 an SDK retries. A value no header can carry is
        // not reported rather than turning a modelled refusal into a server failure.
        if let Some(id) = observed.into_inner() {
            let _ = response.headers_mut().try_insert(REQUEST_ID_HEADER, id);
        }
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
