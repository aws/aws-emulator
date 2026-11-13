// SPDX-License-Identifier: CC0-1.0

//! @op GetLogObject
//!
//! `POST / -> 200` (`X-Amz-Target: Logs_20140328.GetLogObject`). Resolves the
//! `logObjectPointer` the service issued for a LARGE LOG OBJECT and streams that object's
//! bytes back as the modelled `GetLogObjectResponseStream`, a `vnd.amazon.eventstream`
//! body; a pointer the service did not issue is refused.
//!
//! This operation is hand-authored rather than generated: its output member `fieldStream`
//! targets a `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim refuses the
//! request. The routing seam is `src/service.rs`, the shared framing and request reader
//! are `service_common::event_stream`, and the request shape is in
//! `src/ops/types/get_log_object_types.rs`.
//!
//! # What a pointer addresses, and why none resolves here
//!
//! A large log object (LLO) is made at INGESTION, and by one ingestion path only: the
//! model documents this operation as fetching "the content of large portions of log events
//! that have been ingested through the PutOpenTelemetryLogs API", where "when log events
//! contain fields that would cause the total event size to exceed 1MB […] the excess
//! portions are stored as Large Log Objects (LLOs) and these fields are processed
//! separately and LLO reference system fields (in the format `@ptr.$[path.to.field]`) are
//! added". The VALUE of such a reference field is the `logObjectPointer` this operation
//! takes, so a caller only ever round-trips a pointer the service minted for an object it
//! holds.
//!
//! This service's model binds no such ingestion operation — `PutOpenTelemetryLogs` is not
//! among the 118 operations bound to `com.amazonaws.cloudwatchlogs#Logs_20140328`
//! (`smithy_query(query="list_operations")`) — and the one ingestion operation it does
//! bind refuses an event larger than the ceiling instead of splitting its fields ("Each
//! log event can be no larger than 1 MB", `crate::state::MAX_LOG_EVENT_BYTES`, enforced by
//! `crate::ops::put_log_events`). So this store holds no large log object, mints no LLO
//! reference field into the rows `GetQueryResults` reports, and therefore has issued no
//! `logObjectPointer`: every pointer a caller can present — one it built itself, one that
//! has expired, one this service minted for another purpose such as the `@ptr` log RECORD
//! pointer of a query row ([`crate::state::insights::log_record_pointer`]) — is a pointer
//! this service did not issue, and is refused with the operation's modelled
//! `InvalidParameterException` at HTTP 400.
//!
//! That refusal is the whole of what a client can observe of this operation here, and it
//! is the same answer AWS gives the same requests. Should an LLO-producing ingestion path
//! ever be bound to this service, the objects it stores are what this handler resolves a
//! pointer against before framing the `fields` event; nothing is synthesised in the
//! meantime, because a log object nothing ingested does not exist.
//!
//! # Why the identity decision does not come first
//!
//! The resource this call addresses is named by nothing in the request except the pointer:
//! the request carries no log group and no log stream, so WHICH resource is reached is
//! whatever the pointer decodes to. A pointer the service did not issue decodes to no
//! resource, so there is nothing for an identity decision to be about, and the request is
//! refused for what is wrong with IT. That is why the authorization gate in front of every
//! operation of this service defers to this one
//! (`crate::service::identity_authorization::DECIDED_BY_THE_OPERATION`): a caller holding no
//! permissions presenting a pointer this service did not issue is answered the same
//! `InvalidParameterException` as any other caller presenting it.
//!
//! Nothing here mutates the store: resolving a pointer reports an object, so a request
//! leaves every log group, stream and event exactly as a later reader finds them.
//!
//! ## Input
//!
//! * `logObjectPointer` (LogObjectPointer, len 1..512) **required** — the pointer the
//!   service issued for the object to stream, read out of an `@ptr.$[…]` reference field
//!   of the log event that carries it.
//! * `unmask` (Unmask, default `false`) — whether the object's data protection masking is
//!   lifted, as on this service's other event reads (`GetLogEvents$unmask`,
//!   `GetLogRecord$unmask`). It selects the FORM of a resolved object's bytes, so it
//!   cannot make an unresolvable pointer resolve.
//!
//! ## Output
//!
//! * `fieldStream` (GetLogObjectResponseStream, `@streaming`) — the body: one `fields`
//!   event per part of the object, then the stream ends. The union's other member,
//!   `InternalStreamingException`, is the in-band report of a server-side failure
//!   mid-stream, so it belongs to no answer this store produces.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — a parameter is specified incorrectly: a
//!   value outside a modelled constraint, or a `logObjectPointer` this service did not
//!   issue.
//! * `AccessDeniedException`, `InvalidOperationException`, `LimitExceededException`,
//!   `ResourceNotFoundException` (HTTP 400) — the operation's remaining modelled errors;
//!   no condition of this store raises them.

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use service_common::event_stream::{error_response, read_request, RequestRefusal};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it, and every one of them is a client error at HTTP 400.
#[smithy_macros::operation_error("com.amazonaws.cloudwatchlogs#GetLogObject")]
#[derive(Debug)]
pub enum GetLogObjectErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidOperationException(InvalidOperationException),
    InvalidParameterException(InvalidParameterException),
    LimitExceededException(LimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// The refusal CloudWatch Logs answers a `logObjectPointer` it cannot resolve to an object
/// it holds — a pointer a client built itself, one past its validity, or one that addresses
/// something other than a large log object.
///
/// Fixed text for the class: it states what was wrong with the value and carries NOTHING of
/// the value itself, like every other refusal this service words for a caller-supplied token,
/// so a pointer a client may have logged is not reflected back into a 400 body.
const POINTER_NOT_ISSUED_MESSAGE: &str = "Invalid log object pointer format.";

/// The `GetLogObject` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds one of
/// these and `service.rs` holds another wrapped in an `EventStreamRoute`; both reach the
/// same state, because the state lives in the shared engine, not on the operation.
pub struct GetLogObject<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> GetLogObject<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> GetLogObject<Eng> {
    /// The whole request: read it against the model, then answer for the pointer it
    /// carries.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, GetLogObjectErrorModeled> {
        let input = read_input(protocol, req)?;

        // The pointer is resolved against the large log objects this store holds, and it
        // holds none: no operation bound to this service ingests a log event whose fields
        // are split into one (see "What a pointer addresses, and why none resolves here").
        // So the pointer was not issued here whatever it spells and whatever `unmask`
        // asks of the object it would address, and the request is refused rather than
        // answered with bytes nothing stored.
        let _ = (&input.log_object_pointer, input.unmask, &self.engine);
        Err(invalid(POINTER_NOT_ISSUED_MESSAGE.to_string()))
    }
}

/// Read the request into the modelled input shape and check it against the model.
///
/// Driven through the service's own protocol by the shared reader, so the body codec is
/// the protocol's and the rules checked are the model's; the refusal is worded by
/// [`crate::service::constraint_envelope`] rather than by the framework's default
/// envelope, so a bad request to this hand-authored operation reads exactly as the
/// framework's boundary words the same violation for this service's generated operations.
fn read_input(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> Result<GetLogObjectInput, GetLogObjectErrorModeled> {
    read_request::<GetLogObjectInput>(protocol, req, &GetLogObjectInput::SCHEMA).map_err(
        |refusal| {
            let stated = match &refusal {
                RequestRefusal::Invalid(violations) => crate::service::constraint_envelope(violations),
                RequestRefusal::Unreadable(_) => None,
            };
            invalid(stated.unwrap_or_else(|| refusal.stated()))
        },
    )
}

/// "A parameter is specified incorrectly" — this operation's refusal for a request it will
/// not accept.
fn invalid(message: String) -> GetLogObjectErrorModeled {
    GetLogObjectErrorModeled::InvalidParameterException(InvalidParameterException {
        message: Some(message),
    })
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for GetLogObject<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused read is diagnosable. No request
            // payload — the pointer included — and no credential reaches it.
            tracing::debug!(
                operation = "GetLogObject",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}
