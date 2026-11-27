// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-connectcampaigns-amazonconnectcampaignservice` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. EDITED HERE: every
//!   routed request is served through the media-type wrapper below, and an operation whose
//!   input declares a COLLECTION member through one more wrapper in front of it, so a
//!   clause about such a member quotes the collection the request carried
//!   ([`submitted_collection`]).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. It delegates to the shared AWS renderer
//!   ([`service_common::validation_message::aws_validation_refusal_withholding`]), so a
//!   constraint refusal of any operation of this service reads as AWS words it: the
//!   framework's envelope, member paths and requirements, with a `@pattern` quoted without
//!   its whole-value anchors and a clause that states a value only where that value is the
//!   COLLECTION the caller submitted. See the inline note on that method.
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
//! The model declares the `restJson1` protocol trait, whose generic default response media
//! type is `application/json`, while the recorded responses of this service's own front door
//! — success and refusal alike, across every one of its planes — announce
//! `application/x-amz-json-1.1`: that front end labels its JSON bodies with that media type
//! whatever the modelled protocol is, and the media type a client reads is what it keys its
//! parsing and content negotiation off. The rule, the one function that applies it and the
//! operation-seam wrapper that reaches the refusal tail with it all live in
//! [`service_common::response_media_type`]; [`RESPONSE_MEDIA_TYPE`] is this service's own
//! pair, and the seams it is applied at are `override_operation` (every routed request,
//! including the refusal the framework renders inside the request walk),
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
//! ([`announced_by_the_modelled_front_door`]). The sibling ConnectCampaignsV2 API is served
//! by the same front door and the same tag tier, and splits its wire at the same place.
//!
//! One response is announced by the framework alone, because no author seam reaches it:
//! the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything.

mod submitted_collection;

use crate::ops::get_campaign_state_batch::GetCampaignStateBatch;
use crate::ops::put_dial_request_batch::PutDialRequestBatch;
use crate::ops::untag_resource::UntagResource;
use service_common::response_media_type::{ResponseMediaType, ResponseMediaTypeOperation};
use service_common::validation::SubmittedEcho;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service, Violation};
use std::collections::HashSet;
use std::sync::Arc;

/// Every violation of one refusal as this service states it, paired with the paths whose
/// clause QUOTES a value: the collection members whose value half was restored to the
/// collection the caller submitted.
///
/// `restorations` hands each violation as it will be stated beside the value half the
/// framework's own walk wrote for it, so a clause that now carries the submitted collection
/// is told from one that still carries the walk's summary of it by that comparison alone —
/// which is what keeps the two layers that restore a collection ([`stated_refusal`] and
/// [`submitted_collection`]) wording one refusal the same way.
fn restated<I>(restorations: I) -> (Vec<Violation>, HashSet<String>)
where
    I: IntoIterator<Item = (Violation, Option<String>)>,
{
    let mut stated: Vec<Violation> = Vec::new();
    let mut quoted: HashSet<String> = HashSet::new();
    for (violation, written_by_the_walk) in restorations {
        if violation.value != written_by_the_walk {
            quoted.insert(violation.path.clone());
        }
        stated.push(violation);
    }
    (stated, quoted)
}

/// `violations` stated the way this service states a constraint refusal, with the clauses
/// at `quoted` keeping the value half they carry and every other one losing it.
///
/// The envelope, the member paths and every requirement are the shared AWS renderer's
/// ([`service_common::validation_message::aws_validation_refusal_withholding`]), so a
/// constraint refusal of any operation of this service reads as AWS words it, with a
/// `@pattern` quoted without the anchors a whole-value pattern wraps it in.
///
/// What this service decides is the VALUE half of each clause, and it states that half
/// only where the value is a COLLECTION the caller submitted: a scalar member's value is
/// never echoed back — neither its own bytes nor the framework's elision of them — so the
/// clause names the member and the requirement and nothing else. A member that was ABSENT
/// keeps the framework's `Value null`, which is the one thing that half reports about a
/// value nobody sent, and a collection the refusal carries only a COUNT of has that count
/// dropped too, since a count is not a value a caller sent.
fn stated(violations: Vec<Violation>, quoted: &HashSet<String>) -> Option<Refused> {
    // The member paths whose clause states no value at all: every one the framework wrote
    // a value half for that is not the collection the caller submitted.
    let withheld: HashSet<String> = violations
        .iter()
        .filter(|violation| !quoted.contains(&violation.path))
        .filter(|violation| violation.value.is_some() || violation.sensitive)
        .map(|violation| violation.path.clone())
        .collect();
    service_common::validation_message::aws_validation_refusal_withholding(
        &Refusal::Invalid(violations),
        |path| withheld.contains(path),
    )
}

/// `refusal` stated the way this service states a constraint refusal, or `None` for a
/// refusal that is not one.
///
/// The wording is [`stated`]'s. The one collection this layer can quote is the EMPTY one:
/// it is handed the violations and not the request they were found in, so the only
/// collection a clause here can state is the one the walk's own summary determines
/// completely ([`service_common::validation_message::quoting_an_empty_submitted_collection`],
/// the shared rule for that layer). A request served through [`submitted_collection`] has
/// its collections restored from the request itself before this is reached.
fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let (violations, quoted) = restated(violations.iter().map(|violation| {
        let written_by_the_walk = violation.value.clone();
        (
            service_common::validation_message::quoting_an_empty_submitted_collection(
                violation.clone(),
            ),
            written_by_the_walk,
        )
    }));
    stated(violations, &quoted)
}

/// `refusing` stated with every COLLECTION clause quoting the collection the caller
/// submitted, and the violations the answer is rendered from — or `None` when no clause of
/// this refusal states one, which leaves the request to the walk behind
/// [`submitted_collection`].
///
/// The wording is [`stated_refusal`]'s, so which layer refused a request is not something
/// a caller can tell; what this entry point adds is the VALUE half of a clause about a
/// collection, restored from the echo the request itself was read for by the one shared rule
/// for it ([`service_common::validation_message::quoting_the_submitted_collection`]).
fn stated_refusal_quoting_collections(
    refusing: &[(Violation, SubmittedEcho)],
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, quoted) = restated(refusing.iter().map(|(violation, echo)| {
        let written_by_the_walk = violation.value.clone();
        (
            service_common::validation_message::quoting_the_submitted_collection(
                violation.clone(),
                echo.clone(),
            ),
            written_by_the_walk,
        )
    }));
    if quoted.is_empty() {
        return None;
    }
    let refused = stated(violations.clone(), &quoted)?;
    Some((violations, refused))
}

/// The media type the `restJson1` protocol trait this service's model declares implies,
/// which the framework's renderers stamp on every response — the one value
/// [`RESPONSE_MEDIA_TYPE`] replaces.
const REST_JSON_MEDIA_TYPE: &str = "application/json";

/// The pair this service's responses are re-announced across: the media type its model's
/// protocol trait implies, and the one its recorded wire announces (see the module header).
/// Named once, so every seam that re-announces reads the same two values.
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
/// SigV4 signing name — ConnectCampaigns v1 and ConnectCampaignsV2 both sign as
/// `connect-campaigns`, so the key alone matches both and the router rejects every
/// such request as `500 AmbiguousService` instead of serving either one — and their
/// modelled path spaces are disjoint: v1 publishes `campaigns`, `campaigns-state`,
/// `campaigns-summary`, `connect-instance` and `tags` here, while every V2 route
/// lives under `v2` there.
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

/// The `com-amazonaws-connectcampaigns-amazonconnectcampaignservice` emulated service: the generated [`ServiceDefinition`] plus
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
    /// The operations whose input declares a COLLECTION member a caller can take outside
    /// its modelled bounds, each served through the wrapper that quotes the collection the
    /// request carried in the clause about it ([`submitted_collection`]). Held in fields for
    /// the same reason [`Self::media_type`] is, and each delegating to that same wrapper for
    /// every request it words no refusal for.
    campaign_ids: CollectionStated<GetCampaignStateBatch<Eng>>,
    dial_requests: CollectionStated<PutDialRequestBatch<Eng>>,
    tag_keys: CollectionStated<UntagResource<Eng>>,
}

/// One operation served through the collection-quoting wrapper, as the fields above hold it.
type CollectionStated<O> = submitted_collection::StatingTheSubmittedCollection<O>;

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let announced = || {
            ResponseMediaTypeOperation::announcing(
                RESPONSE_MEDIA_TYPE,
                announced_by_the_modelled_front_door,
            )
        };
        Self {
            entry: crate::ServiceDefinition::new(engine),
            media_type: announced(),
            campaign_ids: CollectionStated::new(announced()),
            dial_requests: CollectionStated::new(announced()),
            tag_keys: CollectionStated::new(announced()),
        }
    }

    /// The wrapper that words a refusal of the operation `routing_key` names by quoting the
    /// collection the request carried, or `None` for an operation whose input declares no
    /// collection member a caller can take outside its bounds.
    ///
    /// Keyed on each operation's own modelled name
    /// ([`submitted_collection::StatingTheSubmittedCollection::serves`]), so the mounting is
    /// the model's and not a path this file spells.
    fn stating_the_submitted_collection(&self, routing_key: &str) -> Option<&dyn ErasedOperation> {
        if self.campaign_ids.serves(routing_key) {
            return Some(&self.campaign_ids);
        }
        if self.dial_requests.serves(routing_key) {
            return Some(&self.dial_requests);
        }
        if self.tag_keys.serves(routing_key) {
            return Some(&self.tag_keys);
        }
        None
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
        // ConnectCampaignsV2 API, so the claim is the route key AND the
        // model-derived first URI path segment (v1's unprefixed route space).
        // Matching on the signing name alone would have both services claim every
        // `connect-campaigns`-signed request, which the router answers with 500
        // `AmbiguousService` instead of serving either one.
        can_handle_parts(parts)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) so this service can
        // state it the way the service it emulates states it, which no model records.
        //
        // The whole wording is delegated to [`stated_refusal`], which delegates in turn to
        // the shared AWS renderer: the envelope, the member paths and every requirement
        // stay the framework's, and the renderings AWS does differently are applied to
        // every constrained member of every operation of this service alike — a
        // `@pattern` requirement quoted without the anchors a whole-value pattern wraps it
        // in, and a clause that states a value only where that value is the COLLECTION the
        // caller submitted. A refusal that is not a constraint violation — an
        // unauthenticated caller — keeps the framework's code and wording.
        stated_refusal(refusal)
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
        //
        // An operation whose input declares a COLLECTION member is served through one more
        // wrapper in front of that one ([`submitted_collection`]), so a clause about such a
        // member quotes the collection the request carried — which takes the request, and
        // this is the last seam that still holds it. It delegates to the media-type wrapper
        // for every request it words no refusal for, so the announcing above is unchanged.
        if !service_common::response_media_type::routes(&self.entry, req) {
            return None;
        }
        submitted_collection::routing_key(&self.entry, req)
            .and_then(|key| self.stating_the_submitted_collection(key))
            .or(Some(&self.media_type as _))
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
}
