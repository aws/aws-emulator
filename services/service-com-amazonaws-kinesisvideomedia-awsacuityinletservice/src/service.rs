// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesisvideomedia-awsacuityinletservice` service: how it claims requests and how it words the
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
//! ## The ingest plane
//!
//! This service's model binds one route, `POST /getMedia`. Beside it, a Kinesis video
//! stream has a PRODUCER half — `POST /putMedia`, served at the write data endpoint
//! `GetDataEndpoint(APIName=PUT_MEDIA)` hands a producer — which no model binds at all,
//! so the generated routing table can name no request to it. Two seams carry it, and
//! [`put_media`] owns what it answers:
//!
//! - [`Service::can_handle`] also claims a `POST` whose first path segment is
//!   `putMedia` ([`put_media::claims_parts`]), under the same routing key the modelled
//!   route is claimed under. Without that claim the emulator answers
//!   `404 UnknownService` for the endpoint `GetDataEndpoint` handed out.
//! - [`Service::override_operation`] serves it as an [`ErasedOperation`]
//!   ([`put_media::PutMediaIngest`]) which declares SigV4, so the emulator verifies the
//!   producer's signature before any media is taken — but only at the ADDRESS the write
//!   plane is announced at. At any other address of this emulator the path is bound to no
//!   operation, and [`Service::override_routing_error`] answers it as the unrouted path it
//!   is, with nothing ingested.
//!
//! The Matroska container a producer submits is read by [`matroska`].
//!
//! ## Who may reach either plane
//!
//! Both routes are authorized before they read or write anything, against the
//! identity-based policies IAM holds for the principal the request's credential names
//! ([`authorization`]) — the step AWS takes at the IAM layer ahead of the operation, and
//! the reason a caller a policy denies is served no media and ingests none.
//!
//! ## Where the modelled read route is served
//!
//! The modelled `POST /getMedia` belongs to the READ plane and is bound on the address
//! `GetDataEndpoint` announces for it, exactly as the ingest route is bound on the write
//! plane's ([`read_plane`]). A reader that sent it to this emulator's control-plane front
//! door, or to the write plane's address, reached an endpoint that binds no operation for
//! that path, so [`Service::override_operation`] names this family's front-door answer for
//! an unroutable request instead of the operation — nothing is resolved and nothing is
//! read.

pub mod authorization;
pub mod matroska;
pub mod put_media;
pub mod read_plane;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::service::unknown_operation;

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// routing key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality.
///
/// That path conjunct is what separates services sharing one SigV4 signing name. FIVE
/// emulated services sign as `kinesisvideo` and are addressed by that one endpoint
/// prefix — this media endpoint, the Kinesis Video control plane, Archived Media,
/// Signaling Channels and WebRTC Storage — so the routing key alone matches all five
/// and the router, which refuses rather than guesses, answers `500 AmbiguousService`
/// for ALL of them: a code no AWS model carries. Their modelled path spaces are
/// DISJOINT at the first segment (`getMedia` here; `createStream`/`listTagsForStream`/…
/// in the control plane; `getClip`/`getHLSStreamingSessionURL`/`listFragments` in
/// Archived Media; `v1` in Signaling Channels; `joinStorageSession` in WebRTC
/// Storage), which is exactly the case this framework discriminator exists for.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
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

/// The `com-amazonaws-kinesisvideomedia-awsacuityinletservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves the ingest plane — `POST /putMedia`, the producer half of a stream. Held
    /// on the service because [`Service::override_operation`] hands the router a BORROW
    /// of one of its own operations, and it owns the shared engine so a fragment it
    /// takes lands in the same store `GetMedia` reads.
    ingest: put_media::PutMediaIngest<Eng>,
    /// The shared engine, held beside the entry so the seams below can read this store —
    /// which is where the addresses the control plane announced for this stream's planes
    /// live ([`read_plane`]).
    engine: Arc<Eng>,
    /// What a modelled read call that reached an address which does not serve the read
    /// plane is answered with. Stateless, and held rather than built per request because
    /// the seam answers a reference borrowed from `&self`.
    unrouted_on_other_address: unknown_operation::UnknownOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            ingest: put_media::PutMediaIngest::new(Arc::clone(&engine)),
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            unrouted_on_other_address: unknown_operation::UnknownOperation,
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]: this
        // service's signing name (`kinesisvideo`) is shared with the Kinesis Video
        // control plane, Archived Media, Signaling Channels and WebRTC Storage, so the
        // claim is the routing key AND the model-derived first URI path segment.
        // Matching on the signing name alone would have five services claim every
        // `kinesisvideo` request, which the router answers with 500 `AmbiguousService`
        // instead of serving any of them.
        if can_handle_parts(parts) {
            return true;
        }
        // And the INGEST plane beside it: `POST /putMedia` is this service's producer
        // half (see the module docs), a route no Kinesis Video model binds and none of
        // the four peers claims, so without this the emulator answers
        // `404 UnknownService` for the write data endpoint it hands producers.
        smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
            && put_media::claims_parts(parts)
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
        // `POST /putMedia` is that case: the ingest plane carries no `X-Amz-Target` and
        // no modelled URI names it, so it is named here — which is also what has the
        // router verify the producer's SigV4 signature before the operation runs.
        //
        // It is named only for a request that reached the ADDRESS the ingest plane is
        // announced at ([`put_media::ingest_served_at`]): a producer that sent it to this
        // emulator's control-plane front door, or to the read plane's address, reached an
        // endpoint that binds no operation for the path, so no operation is named for it
        // and `override_routing_error` below answers it as the unrouted path it is.
        if put_media::claims_request(req) && self.ingest.serves_the_addressed_plane(req) {
            return Some(&self.ingest as &dyn ErasedOperation);
        }
        // And the modelled read route is bound on the READ plane's own address: a call for
        // it that reached the control-plane front door or the write plane's address
        // reached an endpoint that binds no operation for its path, so the front door's
        // answer to an unroutable request is named for it instead of the operation, and no
        // operation of this service runs (see [`read_plane`]).
        if read_plane::claims_request(req) && !read_plane::read_plane_served_at(&*self.engine, req)
        {
            return Some(&self.unrouted_on_other_address as &dyn ErasedOperation);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording.
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
        //
        // The ingest path is worded here: an address that does not serve the ingest
        // plane binds no operation for `/putMedia`, which an endpoint answers as an
        // unknown operation rather than as this service's own 501
        // ([`put_media::unrouted_address_response`]). Nothing is read from the request,
        // so nothing is ingested.
        if put_media::claims_request(req) {
            return Some(put_media::unrouted_address_response());
        }
        // And every other request this service claimed and could name no modelled
        // operation for — one of its routes under a method its model does not bind — is
        // the same unroutable request, answered in the same envelope. The framework's
        // `rest` default (`501 NotImplemented`) is a status no endpoint of this family
        // answers.
        Some(unknown_operation::unknown_operation_response())
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
