// SPDX-License-Identifier: CC0-1.0

//! The HTTP STATUS this service answers a modelled error with.
//!
//! This service's wire protocol is `awsJson1_0`, which reports WHICH error a caller
//! hit in the response BODY — the `__type` key naming the error shape — and not in the
//! status line. Its recorded refusals accordingly carry one status for the whole
//! client-error class: a `ResourceNotFoundException` (`@httpError` 404) and a
//! `ConflictException` (`@httpError` 409) are both answered HTTP 400, with the class
//! read off `__type`. A `@httpError` value on a client error shape of this model is
//! therefore an error CLASSIFICATION, not the status line this service writes.
//!
//! That is a property of the service's wire envelope rather than of any one operation,
//! so it lives in the one protocol the dispatch sequence drives every modelled error
//! through ([`ClientErrorStatusProtocol`], handed to the framework by
//! `service.rs`'s `override_protocol`). No handler and no error shape mentions a status
//! at all: each `src/ops/*.rs` returns its modelled error and this is where the status
//! line is decided.
//!
//! Everything else — routing, request deserialization, every success body and the
//! event-stream path — is the generated, model-derived protocol's, delegated to
//! unchanged.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use smithy_server_runtime::ServerHttpProtocol;
use std::sync::Arc;

/// The HTTP status every CLIENT error of this service is answered with.
///
/// `awsJson1_0` names the error in the body's `__type` rather than in the status line,
/// and the recordings of this service's refusals show HTTP 400 for error shapes whose
/// `@httpError` is 404 and 409 alike. See [`error_status`].
pub const CLIENT_ERROR_STATUS: u16 = 400;

/// The status a modelled error is answered with, given the status its shape declares.
///
/// A client error (`4xx`) is answered with [`CLIENT_ERROR_STATUS`] whatever `@httpError`
/// the shape carries, so a caller reads WHICH refusal it was from the error class in the
/// body — the only place this service reports it. A server error keeps its own status:
/// nothing recorded contradicts it, and a `5xx` is what tells a client the call may be
/// retried as sent.
#[must_use]
pub fn error_status(modelled_status: u16) -> u16 {
    match modelled_status {
        400..=499 => CLIENT_ERROR_STATUS,
        other => other,
    }
}

/// This service's wire protocol: the generated, model-derived `awsJson1_0` codec, with
/// this service's own choice of the status line a modelled error is answered with.
///
/// Holds the generated [`crate::ServiceDefinition`] behind the same `Arc` the service
/// does, so there is exactly one routing table and one codec per mounted service.
pub struct ClientErrorStatusProtocol<Eng: Send + Sync + 'static> {
    entry: Arc<crate::ServiceDefinition<Eng>>,
}

impl<Eng: Send + Sync + 'static> ClientErrorStatusProtocol<Eng> {
    /// Build the protocol once, at service construction: `override_protocol` borrows it
    /// from `&self` and must never build one per request.
    pub fn new(entry: Arc<crate::ServiceDefinition<Eng>>) -> Self {
        Self { entry }
    }

    /// The generated, model-derived protocol this one wraps.
    fn inner(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }
}

impl<Eng: Send + Sync + 'static> std::fmt::Debug for ClientErrorStatusProtocol<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientErrorStatusProtocol")
            .field("protocol_id", self.inner().protocol_id())
            .finish()
    }
}

impl<Eng: Send + Sync + 'static> ServerProtocol for ClientErrorStatusProtocol<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.inner().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.inner().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner().deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner()
            .serialize_output(output, output_schema, operation_name, success_status)
    }

    /// The modelled error in the generated `awsJson1_0` envelope — body, `__type` and
    /// error headers untouched — under the status this service answers its class with.
    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner()
            .serialize_error(error, error_schema, error_code, error_status(http_status))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner()
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a modelled CLIENT error (`4xx`) is answered with this service's one
    /// client-error status whatever `@httpError` its shape declares; a server error
    /// (`5xx`) keeps the status its shape declares.
    ///
    /// The rows span every `@httpError` this model's error shapes carry — 400
    /// (`ValidationException`), 402 (`ServiceQuotaExceededException`), 403
    /// (`AccessDeniedException`), 404 (`ResourceNotFoundException`), 409
    /// (`ConflictException`), 429 (`ThrottlingException`), 500
    /// (`InternalServerException`), 503 (`ServiceUnavailableException`); source:
    /// `smithy_query(query="shape",shape="ValidationException,ServiceQuotaExceededException,
    /// AccessDeniedException,ResourceNotFoundException,ConflictException,ThrottlingException,
    /// InternalServerException,ServiceUnavailableException")` — plus each bound of the
    /// client-error range and the value just past each end.
    #[test]
    fn every_client_error_status_collapses_to_one_and_a_server_error_keeps_its_own() {
        // (case, the status the shape declares, the status answered)
        let rows: &[(&str, u16, u16)] = &[
            ("ValidationException", 400, 400),
            ("ServiceQuotaExceededException", 402, 400),
            ("AccessDeniedException", 403, 400),
            ("ResourceNotFoundException", 404, 400),
            ("ConflictException", 409, 400),
            ("ThrottlingException", 429, 400),
            ("InternalServerException", 500, 500),
            ("ServiceUnavailableException", 503, 503),
            ("the low bound of the client-error range", 400, 400),
            ("the high bound of the client-error range", 499, 400),
            ("just below the client-error range", 399, 399),
            ("just above the client-error range", 500, 500),
        ];

        for (case, declared, expected) in rows {
            let answered = error_status(*declared);
            assert_eq!(
                answered, *expected,
                "case {case:?}: a shape declaring @httpError {declared} must be answered \
                 with HTTP {expected}, got {answered}"
            );
        }
    }
}
