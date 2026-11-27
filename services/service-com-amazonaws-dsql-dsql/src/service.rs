// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dsql-dsql` service: how it claims requests and how it words the
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
//!   correctly, or serve a modelled operation WRAPPED. This service wraps every
//!   one of them in [`InModelledEnvelope`], so a request refused before its
//!   handler runs is refused in this service's own modelled `ValidationException`
//!   rather than in the framework's pre-dispatch envelope (see the section below),
//!   and `CreateStream` additionally behind
//!   [`crate::ops::create_stream::PassRoleGate`], because the pass-role
//!   authorization decision on its `targetDefinition` has to be reached before
//!   the framework's model-constraint validation, and this is the only seam
//!   handed the request.
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
//! # How this service refuses a request it never ran
//!
//! `ValidationException` is one of the errors this model binds to EVERY operation,
//! and it declares `message` AND `reason` as `@required`
//! (`smithy_query(query="shape", shape="ValidationException")`), so a request refused
//! for a modelled constraint is answered with that whole body — `reason`, and
//! `fieldList` naming the members that failed — exactly as this service's handler-side
//! refusals are. The framework's pre-dispatch renderer cannot: its only seam,
//! `override_refusal`, carries a code and a sentence, so the body it writes is
//! `message` alone and a `@required` member is missing from it.
//!
//! Every operation is therefore served through [`InModelledEnvelope`], which states
//! that refusal itself — from the framework's own deserializer and constraint walk —
//! before the handler runs. [`validation`] owns the wording and the modelled body, and
//! `override_refusal` words any refusal the framework still renders from the same
//! helpers so the two paths cannot disagree.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The refusal layer: how this service words a request it never ran, and the modelled
/// `ValidationException` body it answers with. Internal to this crate — what a client
/// sees of it is the error on the wire, which the crate's tests assert through the
/// router.
///
/// Visible to the rest of the crate because a MODELLED constraint the framework's
/// constraint walk cannot reach is refused by the handler that owns the member, and
/// such a refusal has to read exactly as the walk's own does
/// ([`validation::fields_that_did_not_validate`]).
pub(crate) mod validation;

/// The `com-amazonaws-dsql-dsql` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before its handler runs is refused
    /// in this service's own modelled `ValidationException` — and `CreateStream`
    /// wrapped once more in [`crate::ops::create_stream::PassRoleGate`], outside that
    /// envelope, so the authorization decision on the target's role is reached ahead
    /// of the model-constraint validation. Keyed by the routing key the protocol
    /// resolves — the key the generated table uses — and built from the SAME shared
    /// engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
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
    pub fn new(engine: Arc<Eng>) -> Self {
        // EVERY operation of the model, because `ValidationException` is a
        // service-COMMON error: the constraint refusal each one owes is the same
        // refusal, so none of them may answer it in a different body. The list is
        // hand-maintained — `OperationRoutingTable` exposes only `add_operation` /
        // `get`, so this crate cannot walk the generated table to wrap whatever it
        // holds — and an operation added to the model and not added here would fall
        // back to the framework's `message`-only envelope;
        // `tests/all/boundary_validation_exception.rs` names every operation the model
        // declares and is what catches the omission.
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            create_cluster::CreateCluster,
            delete_cluster::DeleteCluster,
            delete_cluster_policy::DeleteClusterPolicy,
            delete_stream::DeleteStream,
            get_cluster::GetCluster,
            get_cluster_policy::GetClusterPolicy,
            get_stream::GetStream,
            get_vpc_endpoint_service_name::GetVpcEndpointServiceName,
            list_clusters::ListClusters,
            list_streams::ListStreams,
            list_tags_for_resource::ListTagsForResource,
            put_cluster_policy::PutClusterPolicy,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_cluster::UpdateCluster,
        );
        // `CreateStream` is the one operation with a decision AHEAD of the
        // constraint refusal: being allowed to pass the role its `targetDefinition`
        // names is an authorization decision about the caller and that one member, so
        // the pass-role gate wraps the envelope rather than the other way round — a
        // request naming a role this account cannot pass is 403 whichever other
        // constraint it breaks.
        in_modelled_envelope.add_operation(
            <crate::ops::create_stream::CreateStream<Eng> as Operation>::NAME,
            Box::new(crate::ops::create_stream::PassRoleGate::over(
                InModelledEnvelope::new(crate::ops::create_stream::CreateStream::new(Arc::clone(
                    &engine,
                ))),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusal it owes stated in this service's
/// own modelled `ValidationException`.
///
/// The framework's pre-dispatch renderer gets the status (400) and the wire code
/// (`ValidationException`) right and the BODY wrong: `Service::override_refusal`, the
/// only seam it consults, carries a code and a sentence, so the body is `message`
/// alone — while this model declares `reason` `@required` on that shape and binds it
/// to every operation. A client branching on `reason` finds it absent for the most
/// common failure of the whole service.
///
/// So this wrapper collects the refusal itself — the very classes the framework's own
/// walk collects, from the framework's own deserializer and constraint walk — and
/// renders it through the model's `ValidationException` schema, built from the
/// structured violations rather than from a rendered response. Nothing else is
/// re-implemented: a request it does not refuse is handed to the generated
/// `ErasedOperation::run`, which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error. The cost is that such a request is read twice,
/// once here and once in the operation's own sequence, because the framework hands no
/// already-built input to an operation.
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
            // Scoped so the borrowed `&dyn ErasedServiceDefinition` — which is not
            // `Sync`, while this future is — is dropped before the await below. The
            // protocol itself is.
            let refusal = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                validation::modeled_refusal::<O>(protocol, req)
            };
            if let Some(refusal) = refusal {
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
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused before the handler runs is refused
        // in this service's modelled `ValidationException`, which carries the
        // `@required` `reason` the framework's pre-dispatch envelope cannot, and
        // `CreateStream` wrapped once more in `PassRoleGate` so the pass-role
        // authorization decision on `targetDefinition` is reached ahead of that
        // validation, which no other seam is handed the request to do. A key the model
        // does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service state it the way Aurora DSQL states it. `InModelledEnvelope`
        // answers a refusal it can state in the modelled `ValidationException`
        // BODY, so what reaches here is what the framework still renders: a tag
        // map over its modelled maximum (a bad request body, which is not a
        // modelled shape), and the fallback when the modelled error cannot be
        // serialized. Worded from the same helpers, so the two paths cannot
        // disagree. Both halves replace the code AND the message together, because
        // a client branches on the code.
        match refusal {
            Refusal::Invalid(violations) => Some(validation::refused_for(violations)),
            // Authentication is the framework's answer to give; DSQL words it no
            // differently.
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
