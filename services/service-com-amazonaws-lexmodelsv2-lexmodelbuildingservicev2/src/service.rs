// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2` service: how it claims requests and how it words the
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
//!   correctly. Taken here for the media type this service announces (see below):
//!   the routing stays the model's and only the response header is re-announced.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. Taken here to re-announce the media type of
//!   the entry's own protocol-correct rendering, which is otherwise unchanged.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). Taken here for the same one reason: the media type it announces.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The media type this service announces
//!
//! The model declares the `restJson1` protocol trait, whose generic default response
//! media type is `application/json`, while every recorded response of this service —
//! success and refusal alike, on every status — announces `application/x-amz-json-1.1`:
//! its model-building front end labels its JSON bodies with that media type whatever the
//! modelled protocol is, and the media type a client reads is what it keys its parsing
//! and content negotiation off. The rule, the one function that applies it and the
//! operation-seam wrapper that reaches the refusal tail with it all live in
//! [`service_common::response_media_type`]; [`RESPONSE_MEDIA_TYPE`] is this service's own
//! pair, and the seams it is applied at are `override_operation` (every routed request,
//! including the refusal the framework renders inside the request walk),
//! `override_routing_error` and `override_framework_error`.
//!
//! One response is announced by the framework alone, because no author seam reaches it:
//! the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything.
//!
//! ## The export archive plane
//!
//! `DescribeExport` hands out `downloadUrl`, "a pre-signed S3 URL that points to the bot or
//! bot locale archive"; a URL this service announces is a promise that a caller can follow
//! it, so it is pre-signed against the base URL this emulator is reached on and carries the
//! archive's own object path (`crate::state::export_archive_path`), a path space no modelled
//! operation binds. Two seams carry it, the same pair any emulated download plane uses:
//! [`Service::can_handle`] also claims a request addressed to that path space, and
//! [`Service::override_operation`] serves it through [`export_archive::ExportArchiveRoute`].

use service_common::response_media_type::{ResponseMediaType, ResponseMediaTypeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

pub(crate) mod export_archive;

/// `refusal` with the clauses of each refused member in the order this service states
/// them, or `None` when it is not a constraint refusal.
///
/// This front door reports the EXPRESSION a value has to match before that member's other
/// requirements, where the framework's walk collects a member's violations in the order
/// its constraints are checked (a length before a pattern). Both halves of that order —
/// the members in the order the walk found them, and a member's pattern first — are
/// [`service_common::validation_message::in_pattern_first_order`], the tree's one
/// statement of it; only the owned `Refusal` the seam hands back is rebuilt here.
fn in_pattern_first_order(refusal: &Refusal) -> Option<Refusal> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let ordered = service_common::validation_message::in_pattern_first_order(violations)
        .into_iter()
        .cloned()
        .collect();
    Some(Refusal::Invalid(ordered))
}

/// How this service quotes a `@pattern` requirement back: the expression exactly as the
/// model declares it, anchors included.
///
/// The shared renderer's own choice drops the `^`/`$` a whole-value pattern is written
/// with, which is what most AWS front doors state; this one quotes the declared
/// expression, so the rendering is passed in rather than defaulted.
fn declared_pattern(pattern: &str) -> String {
    pattern.to_string()
}

/// The media type the `restJson1` protocol trait this service's model declares implies,
/// which the framework's renderers stamp on every response — the one value
/// [`RESPONSE_MEDIA_TYPE`] replaces.
const REST_JSON_MEDIA_TYPE: &str = "application/json";

/// The pair this service's responses are re-announced across: the media type its model's
/// protocol trait implies, and the one its recorded wire announces (see the module
/// header). Named once, so every seam that re-announces reads the same two values.
pub const RESPONSE_MEDIA_TYPE: ResponseMediaType = ResponseMediaType::new(
    REST_JSON_MEDIA_TYPE,
    service_common::response_media_type::AWS_JSON_1_1,
);

/// The `com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper every routable request is served through, so a refusal raised inside
    /// the request walk announces this service's media type too. Held in a field, built
    /// once, because [`Service::override_operation`] hands back a borrow of it.
    media_type: ResponseMediaTypeOperation,
    /// Serves the download of an export's archive at the pre-signed URL `DescribeExport`
    /// announced for it; [`Service::override_operation`] hands it that request (see
    /// [`export_archive`]).
    export_archive: export_archive::ExportArchiveRoute<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            media_type: ResponseMediaTypeOperation::new(RESPONSE_MEDIA_TYPE),
            export_archive: export_archive::ExportArchiveRoute::new(engine),
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
        // This service signs as `lex` — the signing name the WHOLE Lex family shares
        // (this V2 model-building control plane, the V2 runtime, and the V1 model-building
        // and runtime APIs), while AWS separates them by endpoint host (`models-v2-lex`
        // here). The router keys on the SigV4 credential scope first, so all four resolve
        // to `lex`: claiming on that name alone makes every one of them claim every Lex
        // request and the router answers `500 AmbiguousService` — an error code no AWS
        // model declares — instead of serving any of them.
        //
        // The claim is therefore this service's own modelled wire surface, through the one
        // rule `service_common::lex` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split): the
        // first URI path segments this `restJson1` model binds, matched by EXACT segment
        // equality, and — on a segment a sibling publishes too — the modelled route
        // (method + URI pattern) its own model binds.
        service_common::lex::claims_request(parts, self.definition(), crate::ROUTE_PATH_SEGMENTS)
            // And a request addressed to the path space the archive of an export is served
            // from, which no modelled operation of any Lex-family model binds: without
            // this the router answers a URL `DescribeExport` handed out as a request naming
            // no operation. See the module docs.
            || self.export_archive.claims_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The download of an export's archive, which is not an API call at all: it is the
        // artifact `DescribeExport` hands out a pre-signed URL for, served by the route
        // that owns it (see [`export_archive`]). Claimed first, because such a request
        // names no modelled operation for the routing table below to resolve.
        if self.export_archive.claims(req).is_some() {
            return Some(&self.export_archive as &dyn ErasedOperation);
        }
        // Every request the generated routing table can serve is served through the
        // shared media-type wrapper, which delegates to that same table entry and then
        // re-announces the media type of what it answered — reaching the refusal the
        // framework renders inside the request walk (see the module header). The routing
        // stays the model's: a request whose routing key names no modelled operation is
        // declined here, so the framework answers it as the unroutable request it is —
        // through `override_routing_error` below, which re-announces that answer too.
        service_common::response_media_type::routes(&self.entry, req)
            .then_some(&self.media_type as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) so this service can state
        // it the way the service it emulates states it, which no model records.
        //
        // The whole wording is delegated to the shared AWS renderer rather than written
        // here: the envelope, the member paths and every requirement stay the
        // framework's. Three things are this service's own, and are applied to every
        // constrained member of every operation of it alike:
        //
        // * the VALUE half of a clause — this front door names the member and the
        //   requirement and states no value at all, whatever the member and whatever it
        //   carried, so nothing is echoed back;
        // * a `@pattern` requirement quoted as the model declares it, anchors included
        //   ([`declared_pattern`]);
        // * the order a member's clauses are stated in ([`in_pattern_first_order`]).
        let ordered = in_pattern_first_order(refusal);
        service_common::validation_message::aws_validation_refusal_withholding_with(
            ordered.as_ref().unwrap_or(refusal),
            declared_pattern,
            |_| true,
        )
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated routing
        // table resolved it. The framework's protocol-correct default for this service's
        // envelope is kept word for word, status, code and body alike; only the media
        // type is re-announced, so an unroutable request is answered under the same
        // media type as a routed one (see the module header).
        Some(RESPONSE_MEDIA_TYPE.announce(self.entry.routing_error_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The entry's protocol-correct rendering is kept word for
        // word; only the media type is re-announced (see the module header).
        //
        // The error is borrowed and `ProtocolError` is neither `Clone` nor
        // reconstructable in general, so the default renderer is reached by rebuilding
        // the one variant the router ever raises here — `Malformed`, from the
        // `aws-chunked` decode it runs before naming an operation. `ProtocolError` is an
        // open enum this crate does not own, so the remaining arm declines to `None`,
        // keeping the framework's own rendering of an error this service cannot rebuild
        // rather than losing what that error said.
        match error {
            aws_smithy_schema_server::ProtocolError::Malformed(message) => {
                let rebuilt = aws_smithy_schema_server::ProtocolError::Malformed(message.clone());
                // Rendered through the framework's own renderer for this service's
                // protocol envelope rather than through the entry: this seam is handed
                // the error alone, and the entry's renderer resolves the envelope from
                // the REQUEST it is given. The model declares restJson1, and the media
                // type this service announces is re-stamped on the rendering anyway.
                Some(RESPONSE_MEDIA_TYPE.announce(
                    smithy_server_runtime::protocol_error_response(
                        smithy_server_runtime::PreDispatchProtocol::Json {
                            content_type: REST_JSON_MEDIA_TYPE,
                            errortype_header: true,
                        },
                        rebuilt,
                    ),
                ))
            }
            _ => None,
        }
    }
}
