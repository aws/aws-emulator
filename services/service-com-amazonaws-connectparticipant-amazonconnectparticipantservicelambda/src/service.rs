// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda` service: how it claims requests and how it words the
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
//! One data plane is mounted here as well: the pre-signed attachment URLs
//! `StartAttachmentUpload` and `GetAttachment` publish name no modelled operation, so they
//! are claimed by `can_handle` (their path is inside this service's own modelled path
//! space) and named as an operation of this service by [`Service::override_operation`], which
//! the router authenticates like any other. See [`attachment_plane`].
//!
//! A sixth seam is taken as well: `override_protocol`, for the ONE thing about this
//! service's wire the generic renderer cannot express — the body member a modelled failure
//! reports its explanatory text in. See [`wire`].

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

mod attachment_plane;
mod connection_type_refusal;
mod wire;

/// Whether this service serves the request these `parts` came from.
///
/// This service signs as `execute-api` — a signing name it SHARES with the Amazon API
/// Gateway Management API
/// (`services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi`,
/// `aws.auth#sigv4` name `execute-api`), because an Amazon Connect participant endpoint
/// is itself served through API Gateway. The router resolves a request's routing key
/// from the SigV4 credential scope FIRST, so both services see the same key: claiming
/// on the signing name alone makes each claim its peer's traffic, the router finds two
/// claimants and answers `500 AmbiguousService` — a code no AWS model declares — which
/// makes BOTH services unreachable.
///
/// Their modelled URI path spaces are disjoint: every operation of this service is bound
/// under `/participant/…` (`ROUTE_PATH_SEGMENTS` is `["participant"]`), while the
/// management API's are bound under `/@connections/…`. So the claim adds the framework's
/// path-segment conjunct over this service's own generated, model-derived segments.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule and is CALLED here rather than re-implemented. It takes a whole
/// `http::Request` while [`Service::can_handle`] is handed only the request's parts, so
/// the two things it reads — the URI and the headers — are re-presented to it as a
/// bodiless request. It reads neither the body nor the method, so nothing it decides on
/// is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
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

/// The `com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The attachment plane, over the SAME shared engine the generated entry threads into
    /// every operation: a request to a published attachment URL names no modelled operation,
    /// so it is served by this one instead, and it must read and write the one store the
    /// operations use rather than a second copy.
    attachments: attachment_plane::AttachmentPlane<Eng>,
    /// This service's wire protocol: the model's own `restJson1`, with the error document
    /// of [`wire`]. Built once here because [`Service::override_protocol`] hands back a
    /// borrow and must never build one per request.
    protocol: wire::ParticipantRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            attachments: attachment_plane::AttachmentPlane::new(engine),
            protocol: wire::ParticipantRestJson::new(),
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
        // The signing name `execute-api` is SHARED with the API Gateway Management API,
        // so the claim is the signing name AND this service's own modelled path space.
        // See [`can_handle_parts`] for why, and for who owns the rule.
        //
        // That claim also covers a request to an ATTACHMENT URL this service published,
        // because the published path is inside this service's own modelled path space
        // (`crate::state::ATTACHMENT_PATH_PREFIX`); `override_operation` serves it.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` lets the generated routing table resolve it. `req` is the whole
        // request, so a decision that depends on the body is reachable here.
        //
        // A request to an ATTACHMENT URL this service published is exactly that case by
        // construction — the path is not in the model — so this is where the documented
        // upload and download of an attachment are served. The PATH decides, read by the one
        // reader of that form (`crate::state::parse_attachment_path`), so the URL this
        // service hands out and the request it serves back cannot disagree about which
        // attachment is named. Naming an OPERATION for it is also what puts the request
        // through the router's own authentication, which is what holds a published URL to the
        // credential it carries. See `attachment_plane`.
        crate::state::parse_attachment_path(::service_common::request_path(req.uri()))
            .map(|_| &self.attachments as &dyn ErasedOperation)
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
        // The one refusal this service states itself is a request for no connection type:
        // it answers that with one sentence of its own rather than a report of the
        // modelled bound. See [`connection_type_refusal`].
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        connection_type_refusal::missing_connection_type(violations).then(|| Refused {
            code: connection_type_refusal::VALIDATION_EXCEPTION.to_string(),
            message: connection_type_refusal::MISSING_CONNECTION_TYPE.to_string(),
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A request to a published attachment URL is NOT answered here: `override_operation`
        // above names an operation for it, so the router authenticates it before it is
        // served. The router reaches THIS seam before authentication, so a data plane
        // answered from it would serve stored bytes to a caller the front door would refuse.
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
        // The model's own restJson1 codec, with the one difference the generic renderer
        // cannot express: a modelled failure's explanatory text is reported under the
        // lowercase wire key this service's error documents carry it in, not under the
        // member name the model spells (see [`wire`]). Borrowed from the field built in
        // `new`, never constructed per request.
        //
        // This seam drives the MODELLED sequence only. A refusal the framework renders
        // before a handler runs is built from a synthetic shape that already spells that
        // member lowercase, so both layers of this service answer one logical failure in
        // one member.
        Some(&self.protocol)
    }
}
