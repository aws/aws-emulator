// SPDX-License-Identifier: CC0-1.0

//! WHERE in a request's life the `Scope` precondition is judged: ahead of every
//! refusal the request boundary would otherwise answer first.
//!
//! # The rule
//!
//! A request that names a `Scope` to RESOLVE something in names the namespace WAF has
//! to act in, and WAF judges whether the endpoint the request reached serves that
//! namespace ([`crate::service::requests::scope_served_here`]) BEFORE it reports any
//! other member's modelled constraint and before it resolves anything the request
//! names. So a caller at an endpoint that cannot act in the namespace it asked for is
//! told its scope is wrong whatever else the request carries, and the refusal class
//! does not depend on whether some unrelated member happened to be well formed.
//!
//! What this gate does NOT take over is whether the value IS a scope: that is a
//! modelled constraint on the `Scope` member, which the request boundary refuses
//! before any handler and states in the request validator's own envelope. The gate
//! takes WAF's own precondition and nothing else.
//!
//! # Why a gate rather than the service's refusal seam
//!
//! The framework's per-request sequence is deserialize -> validate -> handle, so a
//! value outside a modelled `@length`, `@pattern` or `@range` is refused before any
//! handler runs, and the only seam over that refusal
//! ([`smithy_server_runtime::Service::override_refusal`]) is handed the violations
//! ALONE — no request, no operation — so it can neither read the scope the request
//! named nor answer a modelled shape: what travels on it is a code and a message
//! ([`smithy_server_runtime::Refused`]), and `Field`/`Parameter`/`Reason` cannot.
//! A wrapper, unlike that seam, is handed the request, so it can take WAF's own
//! precondition first and answer it the way the operation's error channel does: the
//! modelled `WAFInvalidParameterException`, serialized by this service's own protocol.
//!
//! Nothing else is re-implemented, and nothing is moved out of the handlers: the input
//! is read by the framework's own deserializer, the judgement is the same function
//! every handler calls, and a request this gate does not refuse is handed to the
//! operation it wraps, whose [`ErasedOperation::run`] owns deserialize -> validate ->
//! handle -> serialize, telemetry and every modelled error — including the same scope
//! refusal, for a handler reached any other way. The cost is that such a request is
//! read twice, once here and once in the operation's own sequence, because the
//! framework hands no already-built input to an operation.
//!
//! # What is NOT gated
//!
//! * An operation that MINTS a resource into a namespace rather than resolving one in
//!   it (`CreateWebACL`): the scope names where the new resource lands, not an endpoint
//!   the caller must reach ([`crate::service::requests::validate_minted_scope`]).
//! * An operation whose target is named by an ARN that carries its own namespace
//!   (`GetTopPathStatisticsByTraffic`): there the ARN is judged against the requested
//!   scope first, so its own handler orders those two refusals.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Service,
};

use crate::ops::types::{WAFInvalidParameterException, SCHEMA_WAF_INVALID_PARAMETER_EXCEPTION};

/// The HTTP status `WAFInvalidParameterException` is answered with
/// (`smithy_query(query="operation",operation="UpdateWebACL")`: `errorKind` client,
/// `httpError` 400) — restated here because serializing a modelled error takes its
/// status as an argument.
const INVALID_PARAMETER_STATUS: u16 = 400;

/// The wire error code (`__type`) of that shape, as every operation's error channel
/// renders it.
const INVALID_PARAMETER_CODE: &str = "WAFInvalidParameterException";

/// How one operation's input names the `Scope` it asks about: `None` for an input that
/// may leave it out (a web ACL addressed by its ARN alone), in which case the request
/// names no namespace for this gate to judge.
pub type ScopeOf<Input> = fn(&Input) -> Option<&str>;

/// One operation, served so the `Scope` its request names is judged before the request
/// boundary refuses anything else about it.
///
/// Generic over what it wraps, which is the operation as the service otherwise serves
/// it: `crate::service` wires it around the operation built from the shared engine, so
/// the store a request does reach is the one store.
pub struct ScopeGate<Inner: Operation> {
    operation: Inner,
    /// Reads the `Scope` member off this operation's own input, which only the service
    /// wiring the gate can name.
    scope_of: ScopeOf<Inner::Input>,
}

impl<Inner: Operation> ScopeGate<Inner> {
    /// Gate `operation` — already constructed from the shared engine — reading the
    /// scope it asks about with `scope_of`.
    pub fn over(operation: Inner, scope_of: ScopeOf<Inner::Input>) -> Self {
        Self {
            operation,
            scope_of,
        }
    }

    /// This gate's answer to `req`, or `None` when the request names a namespace this
    /// endpoint can act in — in which case the operation behind it is served.
    ///
    /// `None` as well for a request the framework's own deserializer cannot read at
    /// all: there is no scope to judge, so the refusal is left to the walk behind the
    /// gate, which answers it in its own envelope.
    ///
    /// The service definition is read, used and dropped inside this one function:
    /// `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the caller's
    /// await would make that future non-`Send`.
    fn refusal(
        &self,
        svc: &dyn Service,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Option<RtResponse> {
        let protocol = svc.definition().protocol();
        let mut deserializer = protocol.deserialize_input(req, Inner::INPUT_SCHEMA).ok()?;
        let read = <Inner::Input as DeserializableStruct>::deserialize(&mut *deserializer);
        drop(deserializer);
        let input = read.ok()?;
        let scope = (self.scope_of)(&input)?;
        let refusal = crate::service::requests::scope_served_here(scope, ctx.parts()).err()?;
        Some(self.answer(svc, req, &refusal))
    }

    /// The scope refusal on the wire: the modelled shape, its schema, its wire code and
    /// its modelled status — the SAME four facts the framework's own error tail renders
    /// a modelled failure from, so this is the answer the handler behind the gate gives
    /// for the same request.
    fn answer(
        &self,
        svc: &dyn Service,
        req: &RtRequest,
        refusal: &WAFInvalidParameterException,
    ) -> RtResponse {
        // The record the operation's own sequence would have made: this answers ahead
        // of `ErasedOperation::run`, which is where the framework classifies an
        // outcome. Nothing the caller sent is logged — which operation refused, and
        // which class it refused with, are what a reader needs.
        ::tracing::info!(
            operation = Inner::NAME,
            outcome = INVALID_PARAMETER_CODE,
            status = INVALID_PARAMETER_STATUS,
            "refused a request naming a scope this endpoint does not serve"
        );
        let protocol = svc.definition().protocol();
        match protocol.serialize_error(
            refusal,
            &SCHEMA_WAF_INVALID_PARAMETER_EXCEPTION,
            INVALID_PARAMETER_CODE,
            INVALID_PARAMETER_STATUS,
        ) {
            Ok(response) => response,
            Err(unserializable) => svc
                .definition()
                .framework_error_response(unserializable, req),
        }
    }
}

impl<Inner: Operation + ErasedOperation> ErasedOperation for ScopeGate<Inner> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(response) = self.refusal(svc, req, ctx) {
                return response;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}
