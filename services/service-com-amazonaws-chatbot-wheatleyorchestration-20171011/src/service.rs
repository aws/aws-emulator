// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-chatbot-wheatleyorchestration-20171011` service: how it claims requests and how it words the
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
//! own behaviour, so an unedited service still routes and answers correctly
//! (`override_protocol` is a sixth, which this service takes — see [`wire`]):
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// How this service's error document carries a modelled exception's `Message` member: the
/// sixth seam, `override_protocol`, which this service takes for that one thing the generic
/// renderer cannot express.
pub mod wire;

/// Whether IAM lets a request perform the action it asks for: the authorization step the
/// operations that act on a custom action run before they read or write the store.
pub mod authorization;

/// The resource-scoped authorization decision `DisassociateFromConfiguration` takes on the
/// `Resource` it addresses, ahead of the modelled constraints on that member.
pub mod resource_authorization;

/// The error code AWS Chatbot answers a request whose input does not satisfy a modeled
/// constraint with.
///
/// `InvalidRequestException` is this service's own client-input error — `@error("client")`
/// `@httpError(400)`, documented "Your request input doesn't meet the constraints required
/// by AWS Chatbot" — and the service model declares NO `ValidationException` at all, so the
/// framework's synthetic `smithy.framework#ValidationException` names a shape no chatbot
/// client (or code generator) can know. The distinction is client-visible: an SDK unmarshals
/// a MODELED error into its typed exception, so the code a client branches on has to be this
/// one.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The message that refusal carries.
///
/// It is constant: the request is refused before it is dispatched to an operation, so the
/// answer names neither the member nor the constraint that failed. The status (400) and
/// [`INVALID_REQUEST_EXCEPTION`] are the contract; this sentence is the text AWS Chatbot was
/// recorded answering for such a refusal, identical on every operation, and not a value a
/// client may rely on.
const INVALID_REQUEST_BODY_MESSAGE: &str = "Invalid request body";

/// The `com-amazonaws-chatbot-wheatleyorchestration-20171011` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol: the model's own `restJson1`, with the error document
    /// of [`wire`]. Built once here because [`Service::override_protocol`] hands back a
    /// borrow of it.
    protocol: wire::MessageAlwaysPresentRestJson,
    /// `DisassociateFromConfiguration`'s resource-scoped authorization decision, taken ahead
    /// of the modelled constraints on the member it is about; see
    /// [`resource_authorization`].
    resource_gate: resource_authorization::ResourceAuthorizationGate<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        use resource_authorization::ResourceAuthorizationGate;
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            protocol: wire::MessageAlwaysPresentRestJson::new(),
            resource_gate: ResourceAuthorizationGate::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
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
        // ONE case needs it: `DisassociateFromConfiguration`, whose resource-scoped
        // authorization decision is recorded ahead of the modelled constraints on the member
        // it is about — see [`resource_authorization`] for why that cannot be done from
        // inside `handle`, and why the LINK is not gated the same way. The operation name
        // comes from the model, not from a literal, and a request naming any other operation
        // is left to the generated routing table.
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        (self.resource_gate.operation_name() == routing_key)
            .then_some(&self.resource_gate as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // AWS Chatbot answers EVERY such refusal with its own modeled client-input
        // error, `InvalidRequestException` (400) — "Your request input doesn't meet the
        // constraints required by AWS Chatbot" — whichever operation was addressed and
        // whichever constraint (pattern, range, length, required) failed. One decision
        // for every operation and every constraint kind, because the request is refused
        // before it is dispatched to an operation.
        //
        // The status stays with the protocol (400, which is also
        // `InvalidRequestException`'s `@httpError`), and both the code and the message
        // are replaced together: a message without its matching code is worse than
        // leaving both alone.
        //
        // An authentication refusal is NOT an input-validation failure — it keeps the
        // framework's SigV4 wording and 403 — and `Refusal` is `#[non_exhaustive]`, so a
        // refusal class this crate has not seen keeps the default wording too.
        match refusal {
            Refusal::Invalid(_) => Some(Refused {
                code: INVALID_REQUEST_EXCEPTION.to_string(),
                message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
            }),
            _ => None,
        }
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restJson1 codec, with the ONE difference the generic renderer
        // cannot express: a modelled exception's `Message` member is present in the error
        // document even where the error class states no message, as JSON `null` rather than
        // dropped (see `wire`). Borrowed from the field built in `new`, never constructed
        // per request.
        Some(&self.protocol)
    }
}
