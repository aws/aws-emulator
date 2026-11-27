// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lambdacore-lambdacoreapiservice` service: how it claims requests and how it words the
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
//! ## This service's edited seam: how a broken constraint is stated
//!
//! A constraint refusal is worded from the REQUEST — the member path, the value as
//! submitted and the requirement that member failed ([`validation`]). Two parts of that
//! sentence the framework cannot produce from the violations alone:
//!
//! * the value a clause echoes. The framework's constraint walk stores it ELIDED (a
//!   byte-bounded prefix plus `... [<n> bytes]`, a marker AWS never emits), because a
//!   clause interpolates whatever is stored and a caller controls the length. The unelided
//!   value is only recoverable from the built input, which [`Service::override_refusal`] is
//!   never handed — it is given the violations alone;
//! * the value a clause states at a LIST member when one of its ELEMENTS failed. AWS
//!   reports such a failure at the list and echoes what that member held; the violation
//!   carries the one element.
//!
//! [`StatedPerOperation`] is where both come from: every modelled operation is served
//! wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//! violations against that operation's input schema — echoes restored — together with the
//! request's own lists, and [`validation`] turns them into the sentence. `override_refusal`
//! states a refusal that reaches the centralized walk anyway through that SAME module, so
//! the wording lives in one place and cannot differ between the two paths.
//!
//! The refusals a HANDLER words — a connector that is not there, a name already taken —
//! are not here: they are built by the shared helpers in [`crate::state`], so every
//! operation states them one way.

/// The AUDIT leg of this front door: every control-plane call it serves is a management
/// event of the account, recorded in the event history CloudTrail's `LookupEvents` reads;
/// see the module's own header.
pub mod management_events;

mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

/// The `com-amazonaws-lambdacore-lambdacoreapiservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served AUDITED — so the
    /// call is recorded in the account's event history (see [`management_events`]) — and
    /// wrapped in [`StatedPerOperation`] so a refused request is worded the way this service
    /// words it. Keyed by the routing key the protocol resolves — the key the generated
    /// table uses — and built from the SAME shared engine, so a wrapped operation reads and
    /// writes the one store.
    stated: OperationRoutingTable,
}

/// Serve one `crate::ops::<module>::<Operation>` per line through `table`: AUDITED, as a
/// call of the read/write class the management-event catalog states for it
/// ([`management_events::recorded_class`]), and still wrapped in [`StatedPerOperation`] —
/// inside the recorder, so a record reports the operation's own typed input and outcome
/// while the refusal wording stays this service's. Each row is keyed by its operation shape
/// name — the routing key `ServerProtocol::routing_key` resolves and the generated table is
/// keyed by.
///
/// A row the catalog holds no class for is skipped rather than guessed at, so the list below
/// and that catalog cannot disagree about which calls are management events; `tests` below
/// fails when the model declares an operation neither of them holds.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            if let Some(kind) = management_events::recorded_class(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
            ) {
                $table.serve_behind(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                    kind,
                    |audited, _envelope| StatedPerOperation::new(audited),
                );
            }
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // The table every modelled operation of this service is served through: the AUDIT
        // table, so each control-plane call it serves is recorded in the account's event
        // history where CloudTrail's `LookupEvents` reads it (see [`management_events`]) —
        // one code path for all of them, rather than a recorder per handler.
        let mut stated = management_events::recording_table(&engine);
        // EVERY modelled operation (one per file in `src/ops/`), because both rules are the
        // service's and not one operation's: an operation whose input declares no constraint
        // simply never has a violation to state, which the wrapper reads off that
        // operation's own input schema, and every control-plane call of an account is a
        // management event. An operation added later is added here as well, or it answers a
        // refusal in the framework's wording rather than this service's and its calls are
        // served unrecorded; `tests` below fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            create_network_connector::CreateNetworkConnector,
            create_runtime::CreateRuntime,
            create_runtime_alias::CreateRuntimeAlias,
            create_runtime_version::CreateRuntimeVersion,
            delete_network_connector::DeleteNetworkConnector,
            delete_runtime::DeleteRuntime,
            delete_runtime_alias::DeleteRuntimeAlias,
            delete_runtime_version::DeleteRuntimeVersion,
            get_network_connector::GetNetworkConnector,
            get_runtime::GetRuntime,
            get_runtime_alias::GetRuntimeAlias,
            get_runtime_version::GetRuntimeVersion,
            list_network_connector_versions::ListNetworkConnectorVersions,
            list_network_connectors::ListNetworkConnectors,
            list_runtime_aliases::ListRuntimeAliases,
            list_runtime_versions::ListRuntimeVersions,
            list_runtimes::ListRuntimes,
            restore_runtime::RestoreRuntime,
            update_network_connector::UpdateNetworkConnector,
            update_runtime::UpdateRuntime,
            update_runtime_alias::UpdateRuntimeAlias,
            update_runtime_version::UpdateRuntimeVersion,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated: stated.finish(),
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it: the value as submitted, and a list-element failure reported at its list.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this
/// wrapper does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation. The
/// structurally right place for the collecting half is the frozen crate — handing a
/// handler, or `override_refusal`, the input it already built — which
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
/// the violations the framework's own walk finds, and the wording [`validation`] gives
/// them. `None` when the request breaks none — and for a failure that is NOT a refusal (a
/// body the codec cannot read at all, a member of the wrong type), which the framework's
/// own walk answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_lists`: one read of the request
/// yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input) with each value echo
/// restored to the bytes the caller sent, plus the request's own lists — the value a clause
/// stated at a LIST member needs and no violation carries. Nothing is re-checked here, and
/// no second copy of that walk lives in this crate: both halves are bounded where those
/// bounds are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    let stated = validation::refusal(&violations, || lists)?;
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
                return svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated), req);
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
        // This service signs as `lambda` — a signing name it SHARES with the Lambda
        // control plane, Lambda MicroVMs and Lambda Web (`smithy_query(
        // query="service_info")` gives all four `sigv4Name: lambda`). The router
        // resolves a request's routing key from the SigV4 credential scope first, so all
        // four see the key `lambda`: claiming on the signing name alone makes each one
        // claim the others' traffic, which the router answers `500 AmbiguousService`
        // instead of serving any of them.
        //
        // So the claim is narrowed to this service's own request space, by the one rule
        // `service-common` states for a shared signing name: this model is `restJson1`
        // (a REST protocol, so the `rpc_service_shape_id` argument is `None`), and a
        // REST service's own surface is the modelled URI path space its generated
        // `ROUTE_PATH_SEGMENTS` publishes — here the API-date segments its two resource
        // families bind (`2026-04-04` for the network connectors, `2026-10-01` for the
        // customer-managed runtimes), neither of which any peer model binds.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in `StatedPerOperation` so
        // a refused request is worded the way this service words it. A key the model does
        // not name resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) so this service can
        // state it its own way.
        //
        // AWS states it as the `400 ValidationException` envelope the framework already
        // renders — `N validation error(s) detected: Value '<value>' at '<member path>'
        // failed to satisfy constraint: <requirement>` — with the two differences
        // [`validation`] owns: a `@pattern` quoted without the `^` / `$` the model writes
        // around it (`service_common::validation::unanchored` is the one renderer of that
        // rule), and a constraint broken by a collection's ELEMENT stated at the
        // collection member with its requirement nested. The CHECK still runs the model's
        // constraints as written, and every other requirement, every member path and the
        // envelope stay the framework's.
        //
        // `None` for anything that is not a constraint refusal — an unauthenticated
        // caller keeps the framework's 403 code and wording.
        //
        // This path is handed the violations ALONE — no request — so it can neither echo a
        // value the walk elided nor state the whole list a failed element belongs to;
        // `StatedPerOperation` answers a modelled operation's refusals before this walk is
        // reached, with both of those in hand. A refusal that gets here anyway is still
        // stated by this service, through the same module.
        validation::refused(refusal)
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

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-lambdacore-lambdacoreapiservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// AUDITED and behind [`StatedPerOperation`], under the same key the generated routing
    /// table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` (or to the management-event catalog
    /// that macro reads its class from) fails here instead of silently answering a refusal
    /// in the framework's wording or serving its calls unrecorded — and a key spelled
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
            22,
            "case the model's surface: expected this service's 22 operations, got \
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
                management_events::recorded_class(operation).is_some(),
                "case {operation}: every call of this service is a management event, so \
                 every modelled operation carries a recorded class; this one is missing \
                 from the catalog, so `wrap_operations!` skips it and its calls are served \
                 unrecorded"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served audited and behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it \
                 answers a refusal in the framework's wording instead of this service's \
                 and its calls reach event history nowhere"
            );
        }
    }
}
