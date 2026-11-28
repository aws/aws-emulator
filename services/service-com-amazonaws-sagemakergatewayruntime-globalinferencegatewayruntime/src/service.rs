// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime` service: how it claims requests and how it words the
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
//! # The two seams this service edits
//!
//! * `can_handle` — this service signs as `sagemaker`, a signing name it SHARES with
//!   every other SageMaker-family service while serving its own endpoint
//!   (`gateway-runtime.sagemaker`), so the claim is narrowed to its own modelled wire
//!   surface by the one rule `service-common` states for a shared signing name.
//! * `override_operation` — `InvokeGlobalInferenceGatewayWithResponseStream` is served
//!   from here, because the generated routing table's entry for it declines: the scaffold
//!   generator emits no event-stream framing for a `@streaming` output.
//!
//! How this service READS an invocation request and WORDS a refusal of one is the same
//! for both routes, so it lives in one place, [`invocation_gate`].

/// The content-type gate, the body read, the registry resolution and the refusal wording
/// both operations of this service share.
pub mod invocation_gate;

/// The media type the buffered completion payload is announced under.
pub mod payload_media_type;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The first URI path segment every route of this model binds
/// (`POST /global-inference-gateway/{GatewayId}/invocations` and
/// `.../invocations-response-stream`), which is this service's whole wire surface and so
/// what tells its traffic from that of the SageMaker-family services sharing its signing
/// name. The generated `crate::ROUTE_PATH_SEGMENTS` carries the same value, read from the
/// model; this constant is what the hand-authored streaming route matches its own path
/// against.
pub const GATEWAY_PATH_SEGMENT: &str = "global-inference-gateway";

/// The `com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The buffered operation, served through a layer that announces its payload under the
    /// media type the model declares for it ([`payload_media_type`]). Built from the SAME
    /// shared engine as the generated entry, so both read and write the one store.
    buffered: payload_media_type::AnnouncedAsModelled<
        crate::ops::invoke_global_inference_gateway::InvokeGlobalInferenceGateway<Eng>,
    >,
    /// The hand-authored `InvokeGlobalInferenceGatewayWithResponseStream` implementation,
    /// wrapped in the shared `EventStreamRoute` adapter: the generated routing table holds
    /// a declining entry for that operation, so this one is served through
    /// [`EmulatedService::override_operation`], which the router consults first. Built from
    /// the SAME shared engine, so both routes read and write the one store.
    response_stream: service_common::event_stream::EventStreamRoute<
        crate::ops::invoke_global_inference_gateway_with_response_stream::ResponseStreamInvocation<
            Eng,
        >,
    >,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            buffered: payload_media_type::AnnouncedAsModelled::new(
                crate::ops::invoke_global_inference_gateway::InvokeGlobalInferenceGateway::new(
                    Arc::clone(&engine),
                ),
            ),
            response_stream: service_common::event_stream::EventStreamRoute(
                crate::ops::invoke_global_inference_gateway_with_response_stream::ResponseStreamInvocation::new(engine),
            ),
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
        // This service signs as `sagemaker` — a signing name it SHARES with every other
        // SageMaker-family service (the control plane, the endpoint runtimes, A2I, Edge,
        // FeatureStore, Metrics, the inference router and the job runtimes) — while
        // serving its own endpoint (`gateway-runtime.sagemaker`). The router resolves the
        // routing key from the SigV4 credential scope first, so every one of them sees the
        // key `sagemaker`: claiming on the signing name ALONE claims the siblings' traffic
        // too, and the router — which refuses rather than guesses — answers
        // `500 AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the one
        // rule `service-common` states for every service under a shared signing name. This
        // model is `restJson1` and declares no RPC protocol: its surface is the URI path
        // space its two operations bind — the `/global-inference-gateway/…` space, which no
        // sibling binds — and that is what `claims_request` matches against the generated
        // `ROUTE_PATH_SEGMENTS`.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // `InvokeGlobalInferenceGatewayWithResponseStream` is served from here because the
        // generated routing table's entry for it declines: the scaffold generator does not
        // emit the event-stream framing a `@streaming` output needs. Every other operation
        // resolves through the generated table.
        use crate::ops::invoke_global_inference_gateway_with_response_stream::ResponseStreamInvocation;
        if ResponseStreamInvocation::<Eng>::claims(req) {
            return Some(&self.response_stream);
        }
        // NOT a re-route for the buffered operation: the key is the one the generated
        // table's own protocol resolves, and the operation it names is the one served —
        // standing behind the layer that announces its payload under the media type the
        // model declares for it. A key the model does not name resolves to `None` here
        // exactly as it does in the generated table, so the router's routing-error path is
        // unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        if key == "InvokeGlobalInferenceGateway" {
            return Some(&self.buffered);
        }
        None
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
        let _ = refusal;
        None
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
