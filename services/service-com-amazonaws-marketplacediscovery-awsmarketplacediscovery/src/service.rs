// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplacediscovery-awsmarketplacediscovery` service: how it claims requests and how it words the
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
//!   generated routing table. This service returns the SAME operation the generated
//!   table names, wrapped in [`StatedPerOperation`] (see below).
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
//! ## This service's edited seams
//!
//! * [`authorization`] — the per-request IAM decision every operation takes before it reads
//!   any catalog record: `AccessDeniedException` is a service-common error of this model, so
//!   every operation has to be able to answer it.
//! * [`validation`] — the WORDS of a constraint refusal. Two parts of the sentence AWS
//!   states cannot be produced from the violations alone, which is all `override_refusal` is
//!   handed:
//!   - the value a clause echoes. The constraint walk stores it ELIDED (a bounded prefix
//!     plus a byte-count marker AWS never emits), because a clause interpolates whatever is
//!     stored and a caller controls the length; the submitted bytes are only recoverable
//!     from the built input.
//!   - the value a clause states at a COLLECTION member when one of its ELEMENTS failed.
//!     AWS reports such a failure at the collection and echoes the whole collection; the
//!     violation carries the one element.
//!
//!   [`StatedPerOperation`] is where both come from: every modelled operation is served
//!   wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//!   violations against that operation's input schema — echoes restored — together with the
//!   request's own lists, and [`validation`] turns them into the sentence. `override_refusal`
//!   states a refusal that reaches the centralized walk anyway through the SAME module, so
//!   the wording lives in ONE place — it just has only the violations to state from.
//! * [`saas_fulfillment_page`] — the one address this service serves that no modelled
//!   operation describes: a buyer-facing page a SaaS fulfillment option publishes in
//!   `fulfillmentUrl` / `launchUrl`, claimed in `can_handle` and answered from
//!   `override_routing_error`, because such an address is opened by a browser rather than
//!   called by an SDK.
//! * [`catalog_media`] — the same for the media a CATALOG RECORD publishes: the logo
//!   thumbnail image a listing or product announces in `logoThumbnailUrl`, the mark of a
//!   certification its trust center names (`TrustCenterCertificationAttribute$logoUrl`), and
//!   the reviews page a review source keeps for a listing
//!   (`ReviewSourceSummary$sourceUrl`).

/// The per-request IAM decision every operation of this service takes before it reads any
/// catalog record: `AccessDeniedException` is a service-common error of this model.
pub mod authorization;
/// The routes that answer at the addresses a catalog record publishes its media at: a
/// listing's or product's logo thumbnail image, and the reviews page of a review source.
pub mod catalog_media;
/// The route that answers at the buyer-facing addresses a SaaS fulfillment option
/// publishes, which no modelled operation describes.
pub mod saas_fulfillment_page;
/// The one way this service answers a fetch of an address it published itself, shared by
/// every plane that publishes one.
mod served_document;
/// How a request the framework's walk refused is WORDED.
pub(crate) mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The `com-amazonaws-marketplacediscovery-awsmarketplacediscovery` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The shared engine, for the addresses this service serves that no modelled
    /// operation describes ([`saas_fulfillment_page`], [`catalog_media`]): those answers are
    /// taken outside the operation walk, so they reach the store themselves.
    engine: Arc<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words it.
    /// Keyed by the routing key the protocol resolves — the key the generated table uses —
    /// and built from the SAME shared engine, so a wrapped operation reads the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! stated_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut stated = OperationRoutingTable::new();
        // EVERY modelled operation (ten of them, one per file in `src/ops/`), because the
        // rule is the SERVICE's and not one operation's: an operation whose input declares
        // no constrained collection simply never has such a violation to state, which the
        // wrapper reads off that operation's own input schema. An operation added later is
        // added here as well, or it answers a refusal without the request's own values in
        // hand; the test below fails when one is missing.
        stated_operations!(
            stated,
            engine,
            get_listing::GetListing,
            get_offer::GetOffer,
            get_offer_set::GetOfferSet,
            get_offer_terms::GetOfferTerms,
            get_product::GetProduct,
            list_fulfillment_options::ListFulfillmentOptions,
            list_listing_attributes::ListListingAttributes,
            list_purchase_options::ListPurchaseOptions,
            search_facets::SearchFacets,
            search_listings::SearchListings,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            stated,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it — the value as submitted, and an element failure reported at its collection.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this
/// wrapper does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation. The
/// structurally right place for the collecting half is the frozen crate — handing a handler,
/// or `override_refusal`, the input it already built — which
/// `services/common/src/validation.rs` records as the core seam this re-read exists only
/// until.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// This service's answer to every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk finds and the wording [`validation`] gives them.
/// `None` when the request breaks none — and for a failure that is NOT a refusal (a body the
/// codec cannot read at all, a member of the wrong type), which the framework's own walk
/// answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_lists`: one read of the request
/// yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input) with each value echo
/// restored to the bytes the caller sent, plus the request's own lists — the value a clause
/// states at a collection member and no violation carries. Nothing is re-checked here, and
/// no second copy of that walk lives in this crate: both halves are bounded where those
/// bounds are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        ::service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    let stated = validation::refusal(&violations, &lists)?;
    Some((violations, stated))
}

impl<O> ErasedOperation for StatedPerOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                );
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
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
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Reporting, Deployment,
        // Agreement, Entitlement, Metering), each serving its own endpoint
        // (`discovery-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind — every operation here is `POST /2026-02-05/<operation>`
        // (`OPERATIONS.md`) — which is what `claims_request` matches against the
        // generated `ROUTE_PATH_SEGMENTS` when a service declares no RPC protocol.
        if service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        ) {
            return true;
        }
        // And a fetch of a buyer-facing page a SaaS fulfillment option publishes: the
        // registration and launch addresses this service hands back in `fulfillmentUrl` and
        // `launchUrl` are opened by whatever browser, probe or test holds them —
        // anonymously, with no credential scope naming this service, only the path this
        // service itself minted under the plane segment that is this service's alone. That
        // path is a CAPABILITY rather than a shape: it carries the authenticator this
        // process minted for the option it names (`crate::state`), checked before any
        // record is read, so an address a caller assembled out of the plane's layout and a
        // record id is claimed by nobody. See [`saas_fulfillment_page`]: without this the
        // router answers `404 UnknownService` for an address this service published.
        if saas_fulfillment_page::claimed(parts) {
            return true;
        }
        // And a fetch of the media a catalog record publishes: the logo thumbnail image a
        // listing or product announces, and the reviews page a review source keeps for a
        // listing. Both are addresses this service minted under a plane segment of its own,
        // both carry the authenticator that makes the path a capability, and both are opened
        // anonymously, exactly as the pages above are.
        catalog_media::claimed(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`StatedPerOperation`]
        // so a refused request is worded with the request's own values in hand. A key the
        // model does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // It is handed the violations ALONE — no request — so it can neither echo a long
        // value as the caller submitted it (the walk stores that elided) nor state the whole
        // collection a failed element belongs to; [`StatedPerOperation`] answers a modelled
        // operation's refusals before this walk is reached, with both of those in hand. A
        // refusal that gets here anyway is still stated by this service, through the SAME
        // module, so the wording cannot differ between the two paths — it just has only the
        // violations to state from.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => validation::refusal(violations, &HashMap::new()),
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
        //
        // The addresses this service serves that no modelled operation describes: a
        // buyer-facing page of a stored SaaS fulfillment option, which `fulfillmentUrl` and
        // `launchUrl` publish for a buyer to follow, and the media a catalog record publishes
        // — its logo thumbnail image and the reviews page of a review source. They are
        // answered here because the router consults this seam before it authenticates
        // anything, which is how AWS serves them: unauthenticated, and as an image or a web
        // page rather than a modelled API call. A fetch carries no caller identity, so what
        // stands in for the read that handed the address out is the address itself: each path
        // carries the authenticator this process minted for the record it names
        // (`crate::state`), which both routes verify before they reach the store, so only an
        // address this service published resolves to a record at all.
        saas_fulfillment_page::answer(&*self.engine, req)
            .or_else(|| catalog_media::answer(&*self.engine, req))
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

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `stated_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: no non-test build needs it.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `stated_operations!` fails here instead of silently
    /// answering a constraint refusal without the request's own values in hand — and a key
    /// spelled differently from the generated table's fails here instead of silently never
    /// being reached by `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_this_services_refusal_wording() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.split('#').nth(1))
            .collect();
        assert!(
            !operations.is_empty(),
            "case setup: the model must declare operations, got {}",
            shapes.len()
        );
        let service = EmulatedService::new(Arc::new(::engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation:?}: every modelled operation must be served behind the \
                 wrapper that states this service's refusals"
            );
        }
    }
}
