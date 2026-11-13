// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-paymentcryptography-paymentcryptographycontrolplane` service: how it claims requests and how it words the
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
//! Six opt-in seams are yours to change; every one defaults to the framework's
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
//! - `override_protocol` — the codec the dispatch sequence reads and answers one
//!   request with. THIS service overrides it: the generated `awsJson1_0` protocol
//!   with this service's own choice of the STATUS a modelled error is answered
//!   with (see [`wire`]). Everything a request reads and every success it writes
//!   is the generated protocol's, unchanged.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## This service's edited seams: how a refused request is stated
//!
//! A request this service refuses before any handler runs is answered in AWS's own
//! constraint-violation sentence ([`validation`]), carried in the body member every error
//! shape in this model declares for its prose ([`message_member`]). Three parts of that
//! answer the framework cannot produce on its own, and each needs something
//! [`Service::override_refusal`] is not handed — that seam is given the violations alone:
//!
//! * the value a clause states at a LIST member when one of its ELEMENTS failed. AWS reports
//!   such a failure at the list and echoes the whole list; the violation carries the one
//!   element, so the list has to be read from the request.
//! * the element shape's whole declared constraint set that same clause nests, which is read
//!   off the operation's own input schema.
//! * the body member the text is reported in, which is a property of the rendered RESPONSE
//!   rather than of the refusal's words.
//!
//! [`StatedPerOperation`] is where all three come from: each modelled operation is served
//! wrapped in it, the wrapper asks `service_common::validation` for the framework's OWN
//! violations against that operation's input schema — echoes restored — together with the
//! request's own lists, and answers whatever it produced in this service's error envelope.
//! `override_refusal` states a refusal that reaches the centralized walk anyway through the
//! same [`validation`] module, so the wording lives in ONE place.
//!
//! The refusals a HANDLER words — a key that is not there, an alias name already taken, key
//! material that cannot be exported — are not here: they are modelled errors the operations
//! build themselves. The STATUS each of those is answered at is this service's too, and is
//! decided once in [`wire`].

mod message_member;
mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

pub mod wire;

/// This service's own Smithy shape id, whose LOCAL name
/// (`PaymentCryptographyControlPlane`) is what an `awsJson1_0` request names its target
/// service by (`X-Amz-Target: PaymentCryptographyControlPlane.<Operation>`).
///
/// The SigV4 signing name cannot tell this service from its data-plane peer — both models
/// declare `sigv4Name` `payment-cryptography`, while the endpoints differ
/// (`controlplane.payment-cryptography` here, `dataplane.payment-cryptography` there) — so
/// the shape id is what `service_common::claims_request` keys this service's RPC traffic
/// on.
const RPC_SERVICE_SHAPE_ID: &str = "com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane";

/// The `com-amazonaws-paymentcryptography-paymentcryptographycontrolplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated, model-derived half. Shared with [`Self::protocol`], which wraps
    /// the protocol this entry carries, so the entry is held behind an `Arc` rather
    /// than copied: there is exactly one routing table and one codec per mounted
    /// service.
    entry: Arc<crate::ServiceDefinition<Eng>>,
    /// This service's wire protocol: the generated one, with this service's own choice
    /// of the status a modelled error is answered with. Built once here, never per
    /// request, because [`Service::override_protocol`] hands back a borrow of it.
    protocol: wire::ClientErrorStatusProtocol<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded and carried the way this
    /// service answers it. Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped operation
    /// reads and writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
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
        // EVERY modelled operation (32 of them, one per file in `src/ops/`), because the
        // rule is the service's and not one operation's: an operation whose input declares
        // no constraint simply never has a violation to state, which the wrapper reads off
        // that operation's own input schema. An operation added later is added here as
        // well, or it answers a refusal in the framework's wording and member rather than
        // this service's; `tests` below fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            add_key_replication_regions::AddKeyReplicationRegions,
            associate_mpa_team::AssociateMpaTeam,
            create_alias::CreateAlias,
            create_key::CreateKey,
            delete_alias::DeleteAlias,
            delete_key::DeleteKey,
            delete_resource_policy::DeleteResourcePolicy,
            disable_default_key_replication_regions::DisableDefaultKeyReplicationRegions,
            disassociate_mpa_team::DisassociateMpaTeam,
            enable_default_key_replication_regions::EnableDefaultKeyReplicationRegions,
            export_key::ExportKey,
            get_alias::GetAlias,
            get_certificate_signing_request::GetCertificateSigningRequest,
            get_default_key_replication_regions::GetDefaultKeyReplicationRegions,
            get_key::GetKey,
            get_mpa_team_association::GetMpaTeamAssociation,
            get_parameters_for_export::GetParametersForExport,
            get_parameters_for_import::GetParametersForImport,
            get_public_key_certificate::GetPublicKeyCertificate,
            get_resource_policy::GetResourcePolicy,
            import_key::ImportKey,
            list_aliases::ListAliases,
            list_keys::ListKeys,
            list_tags_for_resource::ListTagsForResource,
            put_resource_policy::PutResourcePolicy,
            remove_key_replication_regions::RemoveKeyReplicationRegions,
            restore_key::RestoreKey,
            start_key_usage::StartKeyUsage,
            stop_key_usage::StopKeyUsage,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_alias::UpdateAlias,
        );
        let entry = Arc::new(crate::ServiceDefinition::new(engine));
        Self {
            protocol: wire::ClientErrorStatusProtocol::new(Arc::clone(&entry)),
            entry,
            stated,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it — the value as submitted, a list-element failure reported at its list with the
/// element shape's declared set — and carried in the body member this model declares for an
/// error's prose.
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
/// the violations the framework's own walk finds, and the wording [`validation`] gives them.
/// `None` when the request breaks none — and for a failure that is NOT a refusal (a body the
/// codec cannot read at all, a member of the wrong type), which the framework's own walk
/// answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_lists`: one read of the request
/// yields the framework's own violations (the protocol's deserializer for an absent
/// `@required` member, `constraints::validate` over the built input) with each value echo
/// restored to the bytes the caller sent, plus the request's own lists — the value a clause
/// stated at a LIST member needs and no violation carries. Nothing is re-checked here, and
/// no second copy of that walk lives in this crate.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    // This operation's own input schema, which is where the element constraint set a clause
    // stated at a LIST member nests comes from.
    let stated = validation::refusal(&violations, &[O::INPUT_SCHEMA], &lists)?;
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
                let answered = svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                );
                return message_member::with_modelled_message_member(answered);
            }
            // Every other answer this operation can give — a success, a modelled error, the
            // scaffold's 501 — passes through the same member rule, which rewrites only the
            // bodies it owns.
            let answered = self.operation.run(svc, req, ctx).await;
            message_member::with_modelled_message_member(answered)
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
        &*self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `payment-cryptography` — a signing name it SHARES with
        // the Payment Cryptography DATA plane, which declares the same `sigv4Name` and
        // is a wire service of its own (`dataplane.payment-cryptography`, `restJson1`)
        // against this one's `controlplane.payment-cryptography`, `awsJson1_0`. The
        // router resolves the routing key from the SigV4 credential scope first, so both
        // services see the key `payment-cryptography`: claiming on the signing name ALONE
        // makes each claim the other's traffic, and the router — which refuses rather
        // than guesses — answers `AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing name.
        // This model is `awsJson1_0` and binds no `smithy.api#http` route, so its surface
        // is the RPC form: the request must NAME this service in
        // `X-Amz-Target: PaymentCryptographyControlPlane.<Operation>` and arrive on the
        // path that protocol posts to (the root). The data plane's own URI path space is
        // then claimed by it alone.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            Some(RPC_SERVICE_SHAPE_ID),
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in `StatedPerOperation` so
        // a refused request is worded and carried the way this service answers it. A key the
        // model does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service reword it. It is
        // handed the violations ALONE — no request and no operation — so it can neither echo
        // the list a failed element belongs to nor read that list's element constraint set
        // off an input schema; `StatedPerOperation` answers a modelled operation's refusals
        // before this walk is reached, with both of those in hand. A refusal that gets here
        // anyway is still stated by this service, through the SAME module, so the wording
        // cannot differ between the two paths — it just has only the violations to state
        // from.
        //
        // An authentication refusal (HTTP 403) is not an input-constraint failure and keeps
        // the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => {
                validation::refusal(violations, &[], &std::collections::HashMap::new())
            }
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

    fn override_protocol(&self) -> Option<&dyn ServerHttpProtocol> {
        // The model-derived protocol is `awsJson1_0`, and so is this one: it
        // delegates every read and every success body to the generated codec and
        // decides only the STATUS a modelled error is answered with, which this
        // service answers as one value for the whole client-error class rather than
        // as each error shape's `@httpError`. See `wire::error_status`. Held in a
        // field (built once in `new`), as this seam requires — never constructed per
        // request.
        Some(&self.protocol)
    }
}

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-paymentcryptography-paymentcryptographycontrolplane.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// answering a refusal in the framework's wording — and a key spelled differently from
    /// the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
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
            32,
            "case the model's surface: expected this service's 32 operations, got \
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
                 answers a refusal in the framework's wording instead of this service's"
            );
        }
    }
}
