// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-resiliencehubv2-ngrhservicecore` service: how it claims requests and how it words the
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

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::no_content_responses::without_content;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The wire error code this service refuses an invalid request with — the client-input
/// error its model declares for every one of its operations (`ValidationException`,
/// `@error("client")`, `@httpError(400)`), which is also the code the framework's own
/// default names for such a refusal. Restated beside the message because
/// [`Service::override_refusal`] replaces code and message together.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// Whether the request these `parts` came from matches this service's SigV4 signing name
/// AND the URI path space its own model publishes (`crate::ROUTE_PATH_SEGMENTS`, derived
/// from the model's `smithy.api#http` URIs at build time — every operation here is bound
/// under the single literal first segment `v2`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here rather than re-implemented: the request's route key
/// must match this service's signing name AND its first URI path segment must be one of
/// the segments passed, by exact segment equality. The shim is the same one
/// `appconfig`, `appconfigdata`, `ivs` and `ivsrealtime` use, and it keeps this crate's
/// dependency set — and so the workspace lock file every concurrent change rewrites —
/// untouched.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_own_path(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-resiliencehubv2-ngrhservicecore` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The protocol this service answers modelled requests in: the model's own `restJson1`,
/// with the one framing correction `service-common`'s no-content module states — an answer
/// at HTTP 204 carries no body, where the serializer renders the empty JSON object for an
/// output structure that declares no member. `TagResource` is such an operation.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that carries
        // no content is emptied of the body rendered for it.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries its error envelope at a 4xx or
        // 5xx, so nothing here is emptied; the same call is made on both paths rather than
        // leaving one of them to answer differently.
        Ok(without_content(response))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here: the model's own, with the one framing correction above —
        // an answer at a no-content status carries no body, which `TagResource`'s
        // members-less output structure would otherwise be rendered as `{}` for.
        Some(self)
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `resiliencehub` — a signing name it SHARES with the V1
        // Resilience Hub endpoint (`services/service-com-amazonaws-resiliencehub-\
        // awsresiliencehub`, `aws.auth#sigv4` name `resiliencehub`, the same
        // `endpointPrefix`). The router resolves a request's routing key from the SigV4
        // credential scope first, so both services see the same key, and claiming on the
        // signing name ALONE makes BOTH claim every request — which the router refuses as
        // `500 AmbiguousService` (in an awsJson envelope, which no restJson1 client can
        // parse) instead of serving either.
        //
        // AWS separates the two by API version in the URI path, which is what this claim
        // uses: this model is `restJson1` with every one of its 73 operations bound under
        // the single literal first segment `v2` (`smithy_query(query="digest")`), so the
        // generated `ROUTE_PATH_SEGMENTS` is `["v2"]` and the claim is the route key AND
        // one of this model's own first path segments, leaving the V1 path space
        // (`/list-apps`, `/create-resiliency-policy`, …) to V1 alone.
        claims_route_key_and_own_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // This service does not state a constraint violation in the common AWS framework
        // form that counts the errors and names the member: a value the model's own
        // regular expression for the member it was sent for does not admit is reported as
        // a bracketed constraint report, one entry per violated member, naming the
        // expression and the value — whichever operation and whichever member carried it.
        // The report is the shared one (`service_common::validation_message`), which the V1
        // endpoint of this same service states too, so the two cannot drift. It answers
        // `None` for a refusal holding a violation of any OTHER modelled constraint, so
        // those keep the framework's wording rather than reading half in one voice and
        // half in the other.
        //
        // The CODE is restated with the message, because a message without its matching
        // code is worse than leaving both alone.
        let Refusal::Invalid(violations) = refusal else {
            // An authentication refusal is the framework's 403 to word, not ours.
            return None;
        };
        let message = service_common::validation_message::pattern_constraint_report(violations)?;
        Some(Refused {
            code: VALIDATION_EXCEPTION_CODE.to_string(),
            message,
        })
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
