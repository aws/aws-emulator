// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-trustedadvisor-trustedadvisor` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. The routing key and the handler stay the
//!   generated table's; each operation is served through [`InModelledEnvelope`], which
//!   states a request refused for breaking a modelled constraint in this service's own
//!   `ValidationException`, worded the way AWS words it. See "Why this file validates
//!   input" below.
//! - `override_refusal` — EDITED HERE. A refusal the framework still renders (one with
//!   no modelled operation to read the request against) is worded from the same
//!   violations through the same [`validation::refusal_message`], so the two refusal
//!   paths cannot disagree.
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
//! ## Why this file validates input
//!
//! `com.amazonaws.trustedadvisor#ValidationException` (`@error("client")`,
//! `@httpError(400)`, one `@required` member `message`) is a COMMON error of every
//! operation of this service: the 400 a caller gets for a value that breaks a rule the
//! model declares is this shape, and the sentence it carries is what an SDK surfaces.
//!
//! That sentence is BUILT FROM THE REQUEST — `<N> validation error(s) detected: ` over
//! one clause per failing member — so it is part of the contract rather than prose, and
//! the framework's own clause renderer diverges from AWS in four places: it prints a
//! `@pattern` with the `^`/`$` anchors a model writes, it lists an enum's value set in
//! the model's declaration order, it echoes the value of a member the model marks
//! `@sensitive`, and it states a collection as its `<n> items` count rather than as the
//! collection the caller sent. The last of those cannot be fixed from
//! [`Service::override_refusal`] at all: that seam is handed the violations alone, never
//! the request they were collected from, so the collection is no longer reachable.
//!
//! [`InModelledEnvelope`] closes that gap. It asks the framework's OWN boundary walk what
//! the request violates — read the request into the operation's input shape, then run the
//! model's constraint traits over what was read
//! (`aws_smithy_schema_server::constraints::validate`, the same call the centralized
//! sequence makes) — and states those violations through this service's modelled
//! `ValidationException`. The rules enforced are therefore exactly the model's; only the
//! WORDS and the envelope are this service's, and they live in [`validation`]. A request
//! the model accepts is delegated untouched to the generated operation, which stays the
//! only thing that serves it: each handler, its state and its 501 are exactly as before.

// `pub(crate)` because this module owns the WORDS of every input-validation refusal of
// this service, including the clause a handler builds for a condition the model's
// constraint traits cannot express: the clause and envelope builders are reached from
// `crate::ops`, while everything that reads a `Violation` stays `pub(super)` to this file.
pub(crate) mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use validation::{refusal_message, rendered_collections, RenderedCollections};

/// The wire error code (`__type` / `x-amzn-errortype`) of this service's
/// input-validation refusal: the shape the model lists as a common error of every
/// operation for "Exception that the request failed to satisfy service constraints", and
/// the same name a handler-returned `ValidationException` carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status that shape's own `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The `com-amazonaws-trustedadvisor-trustedadvisor` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before the handler runs is answered in
    /// this service's own `ValidationException`. Keyed by the routing key the protocol
    /// resolves — the key the generated table uses — and built from the SAME shared
    /// engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
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
    ///
    /// EVERY operation the model declares is wrapped: a constraint is a property of the
    /// INPUT SHAPE and `ValidationException` is a common error of all 12 operations, so
    /// the wrapper goes in front of all of them — implemented or still scaffolded. A stub
    /// answers 501 for a request it could serve and the modelled 400 for one no operation
    /// could, which is the order the real service evaluates them in.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            batch_update_recommendation_resource_exclusion::BatchUpdateRecommendationResourceExclusion,
            get_organization_recommendation::GetOrganizationRecommendation,
            get_recommendation::GetRecommendation,
            list_checks::ListChecks,
            list_organization_recommendation_accounts::ListOrganizationRecommendationAccounts,
            list_organization_recommendation_resources::ListOrganizationRecommendationResources,
            list_organization_recommendations::ListOrganizationRecommendations,
            list_recommendation_resources::ListRecommendationResources,
            list_recommendations::ListRecommendations,
            list_recommendations_for_resource::ListRecommendationsForResource,
            update_organization_recommendation_lifecycle::UpdateOrganizationRecommendationLifecycle,
            update_recommendation_lifecycle::UpdateRecommendationLifecycle,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated the way this service
/// states them.
///
/// The framework's pre-dispatch renderer gets the status (400) and the error code
/// (`ValidationException`) right; what it cannot get right is the sentence, for the four
/// reasons the module docs state — and for one of them (a collection stated as its
/// element count) it cannot, because the seam over it never sees the request.
///
/// So this wrapper collects the refusal itself — the very two classes the framework's own
/// walk collects, from the framework's own deserializer and constraint walk — and renders
/// it through the model's `ValidationException` schema. The envelope is built from the
/// structured [`Violation`]s, never from a rendered response. Nothing else is
/// re-implemented: a request this service does not refuse is handed to the generated
/// `ErasedOperation::run`, which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
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
            let started = std::time::Instant::now();
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            if let Some(refusal) = modelled_refusal::<O>(protocol, req) {
                record_refusal::<O>(ctx, started.elapsed());
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this refusal's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as the
/// framework gates it.
fn record_refusal<O: Operation>(ctx: &OperationContext<'_>, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), O::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            VALIDATION_EXCEPTION_STATUS,
            VALIDATION_EXCEPTION,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

/// How this service refuses the request `O` cannot be served from, or `None` when it has
/// nothing of its own to say — in which case the request goes on to the generated
/// operation and the framework answers as it always did (including for a body the codec
/// cannot read at all, which is a serialization failure rather than a refusal).
///
/// This is the boundary walk the framework runs, in the framework's order and with the
/// framework's own code: read the request into the operation's input shape, then run
/// constraint-trait validation over what was read. Only the WORDS and the envelope are
/// this service's.
///
/// The violations are then put through TWO readers of the SAME built input, neither of
/// which checks anything:
///
/// * [`rendered_collections`] renders each collection member the way a clause states it,
///   which the framework's walk stores as an element count;
/// * [`validation::with_submitted_values`] puts back the value a clause echoes, which the
///   framework's walk stores shortened past 256 bytes (a prefix plus `... [<n> bytes]`,
///   text AWS never emits). A clause states the value the member HELD, so it is read off
///   the request rather than off that rendering — the members this service models reach
///   2048 and 10000 bytes, well past the cap, so a value inside its modelled maximum
///   can break a `@pattern` and still be too long for it.
fn modelled_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse> {
    let input = {
        let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
        match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
            Ok(input) => input,
            // A member the shape requires is absent. It is found while READING the
            // request rather than by walking what was read, but to a caller it is the
            // same refusal — so it is stated the same way, naming the member and the
            // rule it broke. Nothing was read, so there is no collection to render.
            //
            // ONE clause: the codec stops at the first required member it misses, so a
            // request missing two is stated as missing one, and a request missing one
            // while also breaking a constraint states only the absence. That
            // short-circuit is the frozen crate's (`DeserializableStruct::deserialize`
            // returns on the first `MissingMember`), not a choice here; the
            // constraint path below does report every violation it collects.
            Err(SerdeError::MissingMember { member_name }) => {
                let absent = Violation {
                    // The validating walk spells a member's path in lowerCamel; the codec
                    // hands over the Smithy name, so it is cased here to the one spelling
                    // the clause builder uses.
                    path: constraints::path::member(&member_name),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                };
                let message = refusal_message(&[absent], &RenderedCollections::new())?;
                return validation_exception(protocol, message);
            }
            // Any other codec failure is not a refusal this service rewords.
            Err(_) => return None,
        }
    };
    let violations = constraints::validate(&input, O::INPUT_SCHEMA);
    if violations.is_empty() {
        return None;
    }
    let collections = rendered_collections(&input, O::INPUT_SCHEMA);
    let violations =
        validation::with_submitted_values(violations, &input, O::INPUT_SCHEMA, &collections);
    let message = refusal_message(&violations, &collections)?;
    validation_exception(protocol, message)
}

/// This service's refusal carrying `message`: HTTP 400 `ValidationException`, serialized
/// through that shape's own schema so the body carries the modelled `message` member, the
/// `__type` discriminator and the `x-amzn-errortype` header restJson1 resolves an error
/// SHAPE from — exactly as a handler-returned `ValidationException` does.
///
/// `None` when the error cannot be serialized, which leaves the request to the framework
/// rather than answering a 400 with no reason at all.
fn validation_exception(protocol: &dyn ServerHttpProtocol, message: String) -> Option<RtResponse> {
    protocol
        .serialize_error(
            &crate::ops::types::ValidationException { message },
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in `InModelledEnvelope`
        // so a request refused before the handler runs is stated the way this service
        // states it. A key the model does not name resolves to `None` here exactly as it
        // does in the generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam words any refusal the FRAMEWORK still renders. Wording
        // it here from the same violations keeps the two paths from disagreeing: a value
        // outside a declared constraint is this service's `ValidationException` either
        // way, with the same sentence.
        //
        // Two things this seam cannot do are why the wrapper exists at all: the framework
        // hands over the violations alone, not the request they were collected from, so
        // an elided value echo cannot be restored and a collection cannot be rendered —
        // hence the empty collection table here.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure, and the request never reached an operation.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: refusal_message(violations, &RenderedCollections::new())?,
            }),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has no name of its
            // own for keeps the framework's wording.
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
