// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iotsitewise-awsiotsitewise` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it for one thing: `InvokeAssistant` answers a
//!   `vnd.amazon.eventstream` body, which the generated shim of a `@streaming`-output
//!   operation declines rather than serializes, so this seam routes it to the
//!   hand-authored handler in [`crate::ops::invoke_assistant`]. Every other request
//!   resolves through the generated routing table.
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
//! A sixth seam, `override_protocol`, is one this service DOES take: the codec every
//! modelled response is rendered through omits a string member whose stored value is
//! unset instead of reporting it as the empty string — see [`unset_string_members`]
//! for which members of this model that covers and where the rule itself lives. That
//! same seam records what the input shape each request is read against declares, which
//! is what lets the refusal below state a requirement the framework's own walk cannot
//! apply and the constraint set a clause at a collection member lists ([`input_shape`]).
//!
//! The WORDS of a constraint refusal are [`validation`]'s, this service's one home
//! for them — stated in the dialect of the ENDPOINT hosting the operation the
//! request named, which is [`endpoints`]'s reading of the model.

pub mod endpoints;
mod input_shape;
mod unapplied_patterns;
mod unset_string_members;
pub mod validation;

use crate::ops::invoke_assistant::InvokeAssistant;
use service_common::event_stream::EventStreamRoute;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error code IoT SiteWise answers a request that breaks one of the model's
/// own rules with.
///
/// `InvalidRequestException` is the client-input error this service declares ("The
/// request isn't valid. This can occur if your request contains malformed JSON or
/// unsupported characters.") and it is on every operation that takes input. The
/// framework's default code, `ValidationException`, names a shape
/// `com.amazonaws.iotsitewise` does not contain at all, so no generated SDK
/// exception class exists for it and a client's `except InvalidRequestException`
/// never fires.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The `com-amazonaws-iotsitewise-awsiotsitewise` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves `InvokeAssistant`, whose output is a `@streaming` event-stream union. The
    /// generated `ErasedOperation` shim of such an operation declines the request (it
    /// cannot drive `serialize_as_event_stream<P>` through an object-safe `run`), so this
    /// copy is held behind an [`EventStreamRoute`] and routed by
    /// [`Service::override_operation`]. It owns the shared engine, so the conversation it
    /// records and the datasets it reads live in the same store every other handler reads.
    invoke_assistant: EventStreamRoute<InvokeAssistant<Eng>>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: unset_string_members::UnsetStringMembersRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            invoke_assistant: EventStreamRoute(InvokeAssistant::new(Arc::clone(&engine))),
            entry: crate::ServiceDefinition::new(engine),
            protocol: unset_string_members::UnsetStringMembersRestJson::new(),
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
        // `InvokeAssistant` answers an event-stream body, which the generated shim for a
        // `@streaming`-output operation cannot serialize — it declines the request — so
        // this seam routes it to the hand-authored handler, which reads its own request
        // and refuses a constraint violation in this service's own words.
        //
        // The request is recognised by the key the model's OWN route table resolves for it
        // (`POST /assistant/invocation`), not by a path written here, so this seam cannot
        // drift from the URI the model binds. Every other request answers `None` and
        // resolves through that same table.
        let key = self.entry.protocol().routing_key(req).ok()?;
        (key == crate::ops::invoke_assistant::INVOKE_ASSISTANT)
            .then_some(&self.invoke_assistant as &dyn ErasedOperation)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every description this model declares targets one `Description` shape,
        // whose `@length` minimum of 1 means the empty string is not a value it can
        // hold — while each read declares its description `@required` even though
        // the member that writes it is optional on every create and update. So a
        // resource created without a description has none to report, and this codec
        // leaves the member out of the response instead of filling it with a string
        // the shape rejects. `ListActionsResponse$nextToken` is the same shape of
        // problem on a pagination token the model marks `@required` and documents as
        // null on the last page. The rule is stated once, over any member whose own
        // `@length` forbids the empty string, in `service_common` and taken here by
        // [`unset_string_members`]; the codec is otherwise the model's own
        // `restJson1`.
        Some(&self.protocol)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // A value outside a declared `@length`/`@range`/`@pattern`, a value outside
        // an enumerated type, or an absent required member: IoT SiteWise states all
        // of these as [`INVALID_REQUEST`], which is both a modelled shape of this
        // service and a 400, the same status the framework renders a refusal with.
        //
        // Both halves of the answer — the code and the AWS request-validation
        // report naming every member and the requirement it broke — are
        // [`validation`]'s, the one home for this service's refusal words, so no
        // operation and no seam can word the same refusal another way. An
        // authentication refusal keeps the framework's 403 wording.
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
