// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sfn-awsstepfunctions` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## Every served call is a management event
//!
//! The routing table this service answers with is its AUDITED one ([`management_events`]),
//! so every call it serves is recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it. The recording is a property of SERVING a call rather than of
//! any one operation, so no handler of this service carries one.
//!
//! ## Every served call consumes the capacity of its API
//!
//! For the same reason, every call this service serves is counted into the `AWS/States`
//! series of the API it named ([`api_capacity`]), which is also where the observations
//! that call queued are handed over to the stores CloudWatch and CloudWatch Logs answer
//! from.
//!
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header this
//! protocol reports that id in (`x-amzn-RequestId`) — on a success exactly as on a
//! failure. It is a property of the RESPONSE, not of any modelled shape: no output and
//! no error shape of this model declares it, so no handler can set it, and the
//! framework's `awsJson1_0` renderers do not write it.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for
//! every emulated service, and this file only chooses the report this protocol takes
//! (the header alone — `awsJson1_0` has no envelope element for the id) and wires it at
//! the two layers every response of this service passes through:
//!
//! * [`identity_authorization::IamAuthorized`], the wrapper
//!   [`Service::override_operation`] hands the router in place of each modelled operation:
//!   it takes the IAM decision in front of the operation and serves everything it admits
//!   through `service_common`'s own `RequestIdHeaderOperation`, which covers a success, a
//!   modelled error, a constraint refusal raised while that operation was being served,
//!   and its own authorization refusal;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal.
//!
//! Nothing else about a response changes: the status, the other headers and every byte
//! of the body stay as the renderer wrote them.

use service_common::query_error_envelope::RequestIdEnvelopeDefinition;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether IAM lets the CALLER make the request at all: the `iam:PassRole` step a
/// control-plane request that names a `roleArn` takes, and the identity-based decision
/// every call of this service is subject to — both read from the documents in force on
/// the principal the request signed as.
pub(crate) mod authorization;

/// WHO a signed request speaks for, and what the account's IAM store holds about it —
/// the lookups both of those questions are decided from.
pub(crate) mod request_caller;

/// What one SERVED call costs the API it named: the capacity it consumed, and the
/// hand-over of what that call left for the stores that report it.
///
/// Public: the series a call is counted in are read back by this crate's tests.
pub mod api_capacity;

/// This front door's AUDIT leg: every call it serves is recorded as a management event of
/// `states.amazonaws.com` in the account's event history, where CloudTrail's
/// `LookupEvents` reads it.
///
/// Public: its `eventSource` and its per-operation reporting-vs-mutating catalog are
/// checked by this crate's tests.
pub mod management_events;

/// This front door's DATA leg: the calls on the CONTENTS of a state machine, which
/// CloudTrail records as DATA events and delivers only into the bucket of a trail whose
/// event selectors admit them.
///
/// Public: its catalog of the data-event operations and the resource type they name is
/// checked by this crate's tests.
pub mod data_events;

/// Where that identity-based decision is TAKEN: in front of every modelled operation,
/// through [`identity_authorization::IamAuthorized`].
///
/// Public: its catalog of the member each operation addresses its resource by is checked
/// against the Smithy model by this crate's tests.
pub mod identity_authorization;

/// How this service words the input-constraint refusals the framework hands
/// back — the one formatter every operation's boundary validation reaches.
/// How this service words a boundary refusal, and which members it states no value
/// for.
///
/// Reached through `override_refusal` below for a refusal the framework hands over,
/// and through `crate::state`'s one handler-side constraint message for a modelled
/// constraint the centralized walk cannot enforce — both word their clause through
/// the one body there.
pub(crate) mod validation;

/// How this service serializes a response body's NESTED structures — the SWF
/// type discriminator every awsJson recording of this API shows on them.
/// Public: its catalog of discriminated shapes is checked against the Smithy
/// model by this crate's tests.
pub mod wire;

/// The `com-amazonaws-sfn-awsstepfunctions` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers — and routing
    /// through this service's AUDITED table (see [`management_events`]), so every call it
    /// serves is recorded as a management event. Identity, the protocol and the operations
    /// are the generated entry's untouched.
    entry: RequestIdEnvelopeDefinition<
        ::service_common::audited_operation::AuditedDefinition<crate::ServiceDefinition<Eng>>,
    >,
    /// This service's wire protocol, built ONCE here because
    /// `override_protocol` hands out a borrow of it rather than a new one per
    /// request.
    protocol: wire::SwfJsonProtocol,
    /// Serves every modelled operation to a caller IAM allows to perform it, so each
    /// response an operation produced names the request it answers and no request IAM
    /// refuses reaches a handler at all. Held, not built per request:
    /// `override_operation` hands back a borrow of it.
    authorized: identity_authorization::IamAuthorized<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(
                ::service_common::audited_operation::AuditedDefinition::new(
                    crate::ServiceDefinition::new(Arc::clone(&engine)),
                    // Every operation of this service, each wrapped in the shared
                    // management-event recorder and built from this same shared engine
                    // (see [`management_events`]), so each call this service serves is
                    // recorded as a management event of `states.amazonaws.com` — the one
                    // table the identity gate below resolves every admitted request
                    // through, so no request reaches a path that records nothing.
                    management_events::audited_table(&engine),
                ),
            ),
            protocol: wire::SwfJsonProtocol::new(),
            authorized: identity_authorization::IamAuthorized::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what
        // is handed back is the shared `RequestIdEnvelopeDefinition`, which reads
        // identity, protocol and the routing table straight off that entry and adds
        // the request-id header to the three error renderers (see the module docs).
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // NOT a re-route: the operation served is the one this service's own routing
        // table names for the request, reached through the shared
        // `RequestIdHeaderOperation` — the one place every operation's response passes,
        // so no operation can answer without naming the request it answered — and behind
        // the IAM decision [`identity_authorization`] takes in front of every one of
        // them, so no handler can be reached by a caller whose policies do not allow the
        // action it asked for. A request the table does not answer declines here exactly
        // as it would there, leaving the router's routing-error path (which names the
        // request itself, through the definition wrapper) unchanged.
        self.entry
            .routes(req)
            .then_some(&self.authorized as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        //
        // Step Functions answers the standard AWS validation envelope, with the
        // rejected value echoed as the member's MODELLED TYPE renders it — a
        // fractional member with a fractional part, an integral one without. See
        // [`validation`] for the whole rule. Code and message are replaced
        // together, and the code stays the modelled `ValidationException` (HTTP
        // 400) a client branches on; only the value's rendering changes.
        validation::refused(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model-derived protocol is `awsJson1_0`, and so is this one: it
        // delegates routing, request deserialization and the error envelope to
        // the framework's own `awsJson1_0`, and differs only in the success
        // body, where a nested structure of AWS's SWF base model carries the
        // `__type` discriminator the recordings of this API show on it. See
        // [`wire`] for the rule and the shapes it covers.
        Some(&self.protocol)
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
