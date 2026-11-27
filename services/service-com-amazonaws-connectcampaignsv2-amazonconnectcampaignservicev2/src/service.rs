// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2` service: how it claims requests and how it words the
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
//! ## The media type this service announces
//!
//! The model declares the `restJson1` protocol trait, whose generic default response
//! media type is `application/json`, while the recorded responses of this service's own
//! front door — success and refusal alike, across both of its planes — announce
//! `application/x-amz-json-1.1`: that front end labels its JSON bodies with that media type
//! whatever the modelled protocol is, and the media type a client reads is what it keys its
//! parsing and content negotiation off. The rule, the one function that applies it and the
//! operation-seam wrapper that reaches the refusal tail with it all live in
//! [`service_common::response_media_type`]; [`RESPONSE_MEDIA_TYPE`] is this service's
//! own pair, and the seams it is applied at are `override_operation` (every routed
//! request, including the refusal the framework renders inside the request walk),
//! `override_routing_error` and `override_framework_error`.
//!
//! This service's wire shows a SECOND announcer in front of that front door, and then which
//! layer answered decides the value: the refusals of its TAG operations are recorded under
//! the protocol's own `application/json`, and they are recognisable on the wire by the error
//! CODE they carry — `BadRequestException` and `NotFoundException`
//! ([`crate::state::TAG_BAD_REQUEST_CODE`], [`crate::state::TAG_NOT_FOUND_CODE`]), neither
//! of which is the name of any shape this service's model declares, where every answer of
//! the modelled front door carries a modelled shape's name. So those refusals keep the media
//! type their own producer stamped and everything else is re-announced
//! ([`announced_by_the_modelled_front_door`]). The sibling ConnectCampaigns v1 API is served
//! by the same front door and the same tag tier, and splits its wire at the same place.
//!
//! One response is announced by the framework alone, because no author seam reaches it:
//! the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything.
//!
//! ## The form a timestamp travels in
//!
//! Every timestamp member of this model targets one shape, `TimeStamp`, which carries
//! `@timestampFormat("date-time")` (`smithy_query(query="shape", shape="TimeStamp")`), so an
//! instant travels as an RFC 3339 STRING in both directions rather than as the
//! epoch-seconds number `restJson1` defaults to. The generated member schemas drop that
//! format, so the codec is taken from the workspace's shared
//! [`service_common::date_time_json::DateTimeRestJson`], which supplies it at the protocol
//! seam and is the model's own `restJson1` in every other respect. It never overrides a
//! member that DOES declare a format, so the three shapes whose schemas this crate writes
//! out by hand — `Schedule` and the two outbound-request batch inputs — keep answering and
//! parsing exactly as before.

use service_common::response_media_type::{ResponseMediaType, ResponseMediaTypeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

pub mod validation;

/// The media type the `restJson1` protocol trait this service's model declares implies,
/// which the framework's renderers stamp on every response — the one value
/// [`RESPONSE_MEDIA_TYPE`] replaces.
const REST_JSON_MEDIA_TYPE: &str = "application/json";

/// The pair this service's responses are re-announced across: the media type its
/// model's protocol trait implies, and the one its recorded wire announces (see the
/// module header). Named once, so every seam that re-announces reads the same two
/// values.
pub const RESPONSE_MEDIA_TYPE: ResponseMediaType = ResponseMediaType::new(
    REST_JSON_MEDIA_TYPE,
    service_common::response_media_type::AWS_JSON_1_1,
);

/// The wire error codes this service's TAG tier answers with, neither of which is the name
/// of a shape this service's model declares ([`crate::state::TAG_BAD_REQUEST_CODE`] and
/// [`crate::state::TAG_NOT_FOUND_CODE`]) — which is what tells that tier's refusals apart
/// from every answer of the modelled front door on the wire a client reads.
const TAG_TIER_ERROR_CODES: [&str; 2] = [
    crate::state::TAG_BAD_REQUEST_CODE,
    crate::state::TAG_NOT_FOUND_CODE,
];

/// Whether `resp` is one of the responses THIS service's front door produced — the
/// [`service_common::response_media_type::AnnouncedResponses`] predicate the media type pair
/// above belongs to.
///
/// Everything but a refusal the TAG tier answered is the front door's: its successes, the
/// refusals its handlers raise in the model's own shapes, and the refusals the framework
/// renders inside the walk, all of which the recorded wire announces under the awsJson 1.1
/// media type. The tag tier's own refusals — the client errors carrying one of
/// [`TAG_TIER_ERROR_CODES`], read off the response a client receives by the shared
/// [`service_common::response_media_type::client_error_naming_one_of`] — are recorded under
/// the protocol-implied value, so they are handed back exactly as their producer wrote them.
fn announced_by_the_modelled_front_door(resp: &aws_smithy_runtime_api::http::Response) -> bool {
    !service_common::response_media_type::client_error_naming_one_of(resp, &TAG_TIER_ERROR_CODES)
}

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality. That path conjunct is what separates two services sharing one
/// SigV4 signing name — ConnectCampaignsV2 and ConnectCampaigns v1 both sign as
/// `connect-campaigns`, so the key alone matches both and the router rejects every
/// such request as `500 AmbiguousService` instead of serving either one — and their
/// modelled path spaces are disjoint: every V2 route lives under `v2` here, while v1
/// publishes `campaigns`, `campaigns-state`, `campaigns-summary`, `connect-instance`
/// and `tags` there.
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

/// The `com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper every routable request is served through, so a refusal raised
    /// inside the request walk announces this service's media type too. Held in a
    /// field, built once, because [`Service::override_operation`] hands back a borrow
    /// of it.
    media_type: ResponseMediaTypeOperation,
    /// This service's wire protocol: its model's `restJson1` with every timestamp in the
    /// `date-time` form that model declares (see the module header). Held in a field,
    /// built once, because [`Service::override_protocol`] hands back a borrow of it.
    protocol: ::service_common::date_time_json::DateTimeRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            media_type: ResponseMediaTypeOperation::announcing(
                RESPONSE_MEDIA_TYPE,
                announced_by_the_modelled_front_door,
            ),
            protocol: ::service_common::date_time_json::DateTimeRestJson::new(),
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]:
        // this service's signing name (`connect-campaigns`) is shared with the
        // ConnectCampaigns v1 API, so the claim is the route key AND the
        // model-derived first URI path segment (`v2` for every V2 route). Matching
        // on the signing name alone would have both services claim every
        // `connect-campaigns`-signed request, which the router answers with 500
        // `AmbiguousService` instead of serving either one.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request the generated routing table can serve is served through the
        // shared media-type wrapper, which delegates to that same table entry and then
        // re-announces the media type of what it answered — reaching the refusal the
        // framework renders inside the request walk — for the responses this service's own
        // front door produced ([`announced_by_the_modelled_front_door`]); a refusal of the
        // TAG tier keeps the media type its own producer stamped (see the module header).
        // The routing stays the model's: a request whose routing key names no modelled
        // operation is declined here, so the framework answers it as the unroutable
        // request it is — through `override_routing_error` below, which re-announces that
        // answer too.
        service_common::response_media_type::routes(&self.entry, req)
            .then_some(&self.media_type as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. This service DOES word those refusals itself — the pattern it
        // states, the instance id it withholds, the way it reports a list element and
        // the JSON document one operation nests the clause in — and the wording lives
        // once in [`validation`], where a handler weighing a leaf the boundary walk
        // cannot reach reads it too, so the two layers refuse a value with the one
        // sentence. The code goes with it: this service's own `ValidationException`,
        // which every operation of the model declares.
        validation::refusal(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated routing
        // table resolved it. The framework's protocol-correct default for this
        // service's envelope is kept word for word, status, code and body alike; only
        // the media type is re-announced, so an unroutable request is answered under
        // the same media type as a routed one (see the module header).
        Some(RESPONSE_MEDIA_TYPE.announce(self.entry.routing_error_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The entry's protocol-correct rendering is kept word for
        // word; only the media type is re-announced (see the module header). The whole
        // body is the shared rule's: that seam is handed the error ALONE, with no
        // request to resolve an envelope from, and the rebuild-and-render that answer
        // takes lives once in
        // [`service_common::response_media_type::reannounced_framework_error`].
        service_common::response_media_type::reannounced_framework_error(
            RESPONSE_MEDIA_TYPE,
            error,
        )
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every timestamp member of this model is a `date-time` one, which the generated
        // member schemas do not say, so the codec that reads and writes a body is the
        // shared one that supplies the declared format (see the module header). Nothing
        // else about the wire changes: the routing, the envelopes and the media type above
        // are the ones this service already answered with.
        Some(&self.protocol)
    }
}
