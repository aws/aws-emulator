// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-freetier-awsfreetierservice` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself; this
//!   service returns the shared wrapper that serves the routing table's own
//!   operation and re-announces its answer's media type (see "The media type this
//!   service announces" below).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …); this service keeps that rendering word for word and re-announces its
//!   media type.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering; this service keeps that rendering and re-announces its media type.
//! - `override_protocol` — the codec this service's own responses are serialized
//!   in. The default returns `None`, keeping the model-derived protocol; this
//!   service overrides it (see "The media type this service announces" below).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The media type this service announces
//!
//! The model declares the `awsJson1_0` protocol trait, but every recorded response
//! of this service — HTTP 200 and HTTP 400 alike — announces
//! `application/x-amz-json-1.1`, and the media type a client reads is what it keys
//! content negotiation and protocol assertion off. The rule, the one function that
//! applies it and the operation-seam wrapper that reaches the refusal tail all live
//! in [`service_common::response_media_type`];
//! [`RESPONSE_MEDIA_TYPE`](announced_media_type::RESPONSE_MEDIA_TYPE) is this
//! service's pair, and the three seams it is applied at are:
//!
//! - `override_protocol` serves the awsJson `1.1` codec, which is what the
//!   centralized walk serializes this service's own responses with — every success
//!   and every error a handler raises. The JSON framing is byte-identical to `1.0`,
//!   so a request sent with either media type still reads, and only the announced
//!   media type changes;
//! - `override_operation` hands back the shared
//!   [`ResponseMediaTypeOperation`](service_common::response_media_type::ResponseMediaTypeOperation),
//!   which serves the modelled operation the generated routing table names and
//!   re-announces what that walk answered — reaching the framework's refusal tail
//!   INSIDE the walk (an absent `@required` member, a value outside a declared
//!   bound), which `override_protocol` deliberately does not reach;
//! - `override_routing_error` and `override_framework_error` re-announce the other
//!   two framework-rendered responses, keeping their status, code and body word for
//!   word.
//!
//! One response is still announced by the framework alone, because no author seam
//! reaches it: the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything
//! (`vendor/smithy-server-runtime/src/multi_service_router.rs`).
//!
//! ## The decision taken in front of every operation
//!
//! IAM's authorization of the action. AWS judges it at the IAM layer, ahead of the service
//! reading the request's own input and ahead of it reading or writing any record, so the
//! decision is taken by the operation wrapper below ([`StatedPerOperation`]) rather than
//! inside a handler: the input is read in FRONT of every handler — here, to state a
//! constraint refusal in this service's words, and again by the centralized walk that
//! builds the handler's input — so a gate inside a handler could not be first. Which
//! operations are judged is the model's question and is answered in one place
//! ([`authorization::judges`]): the ones whose model declares `AccessDeniedException`.
//!
//! ## This service's other edited seam: how a broken constraint is stated
//!
//! The framework refuses a request that breaks a constraint the model declares BEFORE
//! any handler runs, and lets the service choose the words. Two parts of the sentence
//! AWS states cannot be produced from the violations alone, which is all
//! `override_refusal` is handed:
//!
//! * the value a clause echoes. The constraint walk stores it ELIDED (a bounded prefix
//!   plus a byte-count marker AWS never emits), because a clause interpolates whatever
//!   is stored and a caller controls the length; the submitted bytes are only
//!   recoverable from the built input.
//! * the value a clause states at a LIST member when one of its ELEMENTS failed. AWS
//!   reports such a failure at the list and echoes the whole list; the violation carries
//!   the one element.
//!
//! [`StatedPerOperation`] is where both come from: every modelled operation is served
//! wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//! violations against that operation's input schema — echoes restored — together with
//! the request's own lists, and [`validation`] turns them into the sentence. It
//! re-announces the media type too, so the wrapper that existed for the refusal tail
//! alone ([`ResponseMediaTypeOperation`]) is the fallback for a routable request no
//! wrapper covers rather than the first choice. `override_refusal` states a refusal that
//! reaches the centralized walk anyway through the SAME module, so the wording lives in
//! ONE place — it just has only the violations to state from.
//!
//! The refusals a HANDLER words — a filter whose root-operator arity is wrong, an
//! activity id the catalog does not hold, a `nextToken` this service did not issue — are
//! not here: they are the operations' own, stated in the envelope
//! [`crate::state::stated_validation`] writes so this service's own validation layer
//! words every one of them the same way.

pub mod announced_media_type;
/// The per-request IAM decision the operations whose model declares
/// `AccessDeniedException` are served behind: taken in front of the request's own input,
/// and so in front of the constraint walk and the handler alike (see the module header).
pub mod authorization;
/// Reached by [`crate::state`] as well: a bound a HANDLER enforces on top of the model's
/// is stated in the same words as one the walk found (see that module's rule 7).
pub(crate) mod validation;

use announced_media_type::RESPONSE_MEDIA_TYPE;
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use service_common::response_media_type::ResponseMediaTypeOperation;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

/// The `com-amazonaws-freetier-awsfreetierservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec this service answers in (awsJson `1.1`; see the module docs).
    /// Built once here because [`Service::override_protocol`] hands back a borrow
    /// of it.
    protocol: aws_smithy_schema_server::rpc::HttpRpcServerProtocol<
        aws_smithy_schema_server::json::JsonCodec,
    >,
    /// The wrapper every routable request is served through, so a refusal raised
    /// inside the request walk announces this service's media type too. Held in a
    /// field, built once, because [`Service::override_operation`] hands back a
    /// borrow of it. The FALLBACK: a request whose operation [`Self::stated`] covers
    /// is served through that wrapper instead, which announces the same media type and
    /// states the refusal in this service's words as well.
    media_type: ResponseMediaTypeOperation,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words
    /// it. Keyed by the routing key the protocol resolves — the key the generated table
    /// uses — and built from the SAME shared engine, so a wrapped operation reads and
    /// writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
///
/// The wrapper is handed the shared engine as well as the operation built from it, because
/// the IAM decision it takes in front of the request reads the caller's identity and its
/// policies through that one store ([`authorization`]).
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                    ::std::sync::Arc::clone(&$engine),
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
        // EVERY modelled operation, one per file in `src/ops/`, because the rule is the
        // service's and not one operation's: an operation whose input declares no
        // constraint simply never has a violation to state, which the wrapper reads off
        // that operation's own input schema. An operation added later is added here as
        // well, or it answers a constraint refusal in the framework's wording rather
        // than this service's; `tests` below fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            get_account_activity::GetAccountActivity,
            get_account_plan_state::GetAccountPlanState,
            get_free_tier_usage::GetFreeTierUsage,
            list_account_activities::ListAccountActivities,
            upgrade_account_plan::UpgradeAccountPlan,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: aws_smithy_schema_server::rpc::AwsJson11(),
            media_type: ResponseMediaTypeOperation::new(RESPONSE_MEDIA_TYPE),
            stated,
        }
    }
}

/// One modelled operation, served behind the decision AWS takes in front of a handler —
/// IAM's authorization of the action — and so that a refused request is stated the way this
/// service states it (the value as submitted, and a list-element failure reported at its
/// list) and the answer announces this service's media type.
///
/// The IAM decision is taken HERE rather than inside each handler because of the order the
/// two steps happen in: AWS judges the action at the IAM layer before the request's own
/// input is read, and the input IS read in front of every handler — by this wrapper, to
/// state a constraint refusal in this service's words, and again by the centralized walk
/// that builds the handler's input. A gate inside a handler therefore cannot be first,
/// whatever the handler does, so a caller whose identity-based policies allow nothing would
/// be told about the request it sent instead of being refused the action. Standing the gate
/// in front of both readings is what makes the modelled `AccessDeniedException` the answer
/// to such a caller whatever its input carries; which operations it judges is the model's
/// own question and is answered in one place ([`authorization::judges`]).
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to
/// the generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle
/// -> serialize, telemetry and every modelled error. The violations come from the
/// framework's own machinery ([`stated_refusal`]), so this cannot refuse a request the
/// framework's own walk would accept, nor accept one it would refuse. The cost is that a
/// request this wrapper does not refuse is read twice, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation. The structurally right place for the collecting half is the frozen crate —
/// handing a handler, or `override_refusal`, the input it already built — which
/// `services/common/src/validation.rs` records as the core seam this re-read exists only
/// until.
struct StatedPerOperation<O, Eng> {
    operation: O,
    /// The shared engine, so the principal's policies are read from the one store every
    /// service of this emulator shares — the same engine `operation` holds.
    engine: Arc<Eng>,
}

impl<O, Eng> StatedPerOperation<O, Eng> {
    /// Wrap one operation, already constructed from the shared engine, beside that engine.
    fn new(operation: O, engine: Arc<Eng>) -> Self {
        Self { operation, engine }
    }
}

/// This service's answer to every modelled constraint `req` breaks, for the operation
/// `O`: the violations the framework's own walk finds, and the wording [`validation`]
/// gives them. `None` when the request breaks none — and for a failure that is NOT a
/// refusal (a body the codec cannot read at all, a member of the wrong type), which the
/// framework's own walk answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_lists_quoting_up_to`: one read of
/// the request yields the framework's own violations (the protocol's deserializer for an
/// absent `@required` member, `constraints::validate` over the built input) with each value
/// echo restored to the bytes the caller sent, plus the request's own lists — the value a
/// clause stated at a LIST member needs and no violation carries. Nothing is re-checked
/// here, and no second copy of that walk lives in this crate.
///
/// How LONG a value it restores is this service's own, and is stated from the model
/// ([`validation::LARGEST_QUOTED_VALUE_BYTES`]) rather than left to the shared default,
/// because this model bounds the member every refusal of this service is CARRIED in
/// (rule 9); what the clauses quote together is bounded by [`validation`] as it words them.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists_quoting_up_to::<O>(
            protocol,
            req,
            validation::LARGEST_QUOTED_VALUE_BYTES,
        );
    let stated = validation::refusal(&violations, || lists)?;
    Some((violations, stated))
}

impl<O, Eng> ErasedOperation for StatedPerOperation<O, Eng>
where
    O: Operation + ErasedOperation,
    Eng: Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // IAM FIRST, before a single value of the request's own input is read: the
            // action is this operation's own name under this service's prefix, and a
            // caller no identity-based policy allows is refused it and told nothing
            // about the request it sent — not that it broke a modelled constraint, and
            // not anything about the account's plan data. An operation whose model
            // declares no 403 is not judged (`authorization::judges`).
            if let Some(denial) = authorization::refusal(&*self.engine, ctx, O::NAME) {
                return RESPONSE_MEDIA_TYPE
                    .announce(authorization::refusal_response(svc, req, &denial));
            }
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return RESPONSE_MEDIA_TYPE.announce(svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                ));
            }
            // Every other answer of a routed request — a success, a modelled failure, a
            // refusal the walk raises that this wrapper found nothing to state — is
            // re-announced here, which is what [`ResponseMediaTypeOperation`] does for a
            // request no wrapper covers.
            RESPONSE_MEDIA_TYPE.announce(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
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
        // Every request the generated routing table can serve is served through a
        // wrapper that delegates to that same table entry and then re-announces the
        // media type of what it answered — reaching the refusal the framework renders
        // inside the request walk, which `override_protocol` does not (see the module
        // header). The wrapper is [`StatedPerOperation`] where this service holds one
        // for the operation (it states a constraint refusal in this service's words as
        // well), and the shared media-type wrapper otherwise. The routing stays the
        // model's: a request whose routing key names no modelled operation is declined
        // here, so the framework answers it as the unroutable request it is — through
        // `override_routing_error` below, which re-announces that answer too.
        if !service_common::response_media_type::routes(&self.entry, req) {
            return None;
        }
        let stated = self
            .entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.stated.get(key));
        Some(stated.unwrap_or(&self.media_type as &dyn ErasedOperation))
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. It is handed the violations ALONE — no request — so it can neither
        // echo the value the caller submitted (the walk stores it elided) nor state the
        // list a failed element belongs to; [`StatedPerOperation`] answers a modelled
        // operation's refusals before this walk is reached, with both of those in hand.
        // A refusal that gets here anyway is still stated by this service, through the
        // SAME module, so the wording cannot differ between the two paths — it just has
        // only the violations to state from.
        //
        // An authentication refusal (HTTP 403) is not an input-constraint failure and
        // keeps the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => {
                validation::refusal(violations, std::collections::HashMap::new)
            }
            _ => None,
        }
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response this SERVICE produces — a success and a modelled failure
        // its handler raises — is serialized by the protocol returned here, so this
        // is the one place the media type it announces is decided. See the module
        // docs: every recorded response of this service carries the awsJson `1.1`
        // media type, which is what a client keys its parsing off. A refusal the
        // framework renders in front of a handler is not reached from here; the
        // three seams the module header names re-announce those.
        Some(&self.protocol)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. The framework's protocol-correct default for
        // this service's envelope is kept word for word, status, code and body
        // alike; only the media type is re-announced, so an unroutable request is
        // answered under the same media type as a routed one (see the module
        // header).
        Some(RESPONSE_MEDIA_TYPE.announce(self.entry.routing_error_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The entry's protocol-correct rendering is kept word
        // for word; only the media type is re-announced (see the module header).
        //
        // The error is borrowed and `ProtocolError` is neither `Clone` nor
        // reconstructable in general, so the default renderer is reached by
        // rebuilding the error. The match is TOTAL over what this seam is ever
        // handed, not a partial application of the rule: the router raises it from
        // exactly ONE call site, with exactly one variant —
        // `ProtocolError::Malformed(..)`, from the `aws-chunked` decode it runs
        // before naming an operation. `ProtocolError` is an open enum this crate
        // does not own, so the remaining arm exists for a variant a future
        // framework may route here: it declines to `None`, keeping the framework's
        // own rendering of an error this service cannot rebuild rather than losing
        // what that error said.
        match error {
            aws_smithy_schema_server::ProtocolError::Malformed(message) => {
                let rebuilt =
                    aws_smithy_schema_server::ProtocolError::Malformed(message.clone());
                // Rendered through the framework's own renderer for this service's
                // protocol envelope rather than through the entry: this seam is handed
                // the error alone, and the entry's renderer resolves the envelope from
                // the REQUEST it is given. The model declares one protocol family
                // (awsJson), and the media type this service announces is re-stamped on
                // the rendering below anyway.
                Some(RESPONSE_MEDIA_TYPE.announce(
                    smithy_server_runtime::protocol_error_response(
                        smithy_server_runtime::PreDispatchProtocol::Json {
                            content_type: RESPONSE_MEDIA_TYPE.implied(),
                            errortype_header: false,
                        },
                        rebuilt,
                    ),
                ))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: no non-test build needs it.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-freetier-awsfreetierservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a constraint refusal in the framework's wording — and a key spelled
    /// differently from the generated table's fails here instead of silently never being
    /// reached by `override_operation`.
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
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert_eq!(
            operations.len(),
            5,
            "case the model's surface: expected this service's 5 operations, got \
             {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it \
                 answers a constraint refusal in the framework's wording instead of this \
                 service's"
            );
        }
    }
}
