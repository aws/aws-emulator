// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-route53domains-route53domains-v20140515` service: how it claims requests and how it words the
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
//! - `override_operation` — which operation serves a request. EDITED HERE: the key
//!   and the handler stay the generated table's, but the operation is served through
//!   [`StatedPerOperation`], so a request refused before its handler runs is worded
//!   with the REQUEST in hand — the value a clause quotes, which the refusal alone
//!   does not carry.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: delegated to [`validation`], the one place
//!   this service's refusal wording lives, so the two paths cannot word one refusal
//!   differently.
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

mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The routing key an UNSIGNED awsJson request to this service resolves to.
///
/// The framework reads the SigV4 credential scope first — `route53domains`, this
/// service's signing name — and falls back to the `X-Amz-Target` prefix with a trailing
/// `_<digits>` API version stripped. This service's prefix is
/// `Route53Domains_v20140515` (OPERATIONS.md), whose version suffix starts with a letter
/// and so survives that stripping, which leaves the key below. It names this service and
/// no other, so claiming it costs nothing and lets an anonymous client reach the service.
const TARGET_ROUTE_KEY: &str = "route53domains_v20140515";

/// The `com-amazonaws-route53domains-route53domains-v20140515` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a request refused before its handler runs is worded the
    /// way this service words it — including the value a clause quotes, which only a
    /// reading of the REQUEST can state. Keyed by the routing key the protocol resolves
    /// (the key the generated table uses) and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
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
        // EVERY modelled operation (one per file in `src/ops/`), because the rule is the
        // SERVICE's and not one operation's: a clause quotes the value the request
        // carried wherever a constrained member of this model is refused, and an
        // operation whose input declares no constraint simply never has a violation to
        // state, which the wrapper reads off that operation's own input schema.
        wrap_operations!(
            stated,
            engine,
            accept_domain_transfer_from_another_aws_account::AcceptDomainTransferFromAnotherAwsAccount,
            associate_delegation_signer_to_domain::AssociateDelegationSignerToDomain,
            cancel_domain_transfer_to_another_aws_account::CancelDomainTransferToAnotherAwsAccount,
            check_domain_availability::CheckDomainAvailability,
            check_domain_transferability::CheckDomainTransferability,
            delete_domain::DeleteDomain,
            delete_tags_for_domain::DeleteTagsForDomain,
            disable_domain_auto_renew::DisableDomainAutoRenew,
            disable_domain_transfer_lock::DisableDomainTransferLock,
            disassociate_delegation_signer_from_domain::DisassociateDelegationSignerFromDomain,
            enable_domain_auto_renew::EnableDomainAutoRenew,
            enable_domain_transfer_lock::EnableDomainTransferLock,
            get_contact_reachability_status::GetContactReachabilityStatus,
            get_domain_detail::GetDomainDetail,
            get_domain_prices::GetDomainPrices,
            get_domain_suggestions::GetDomainSuggestions,
            get_operation_detail::GetOperationDetail,
            list_domains::ListDomains,
            list_operations::ListOperations,
            list_prices::ListPrices,
            list_tags_for_domain::ListTagsForDomain,
            push_domain::PushDomain,
            register_domain::RegisterDomain,
            reject_domain_transfer_from_another_aws_account::RejectDomainTransferFromAnotherAwsAccount,
            renew_domain::RenewDomain,
            resend_contact_reachability_email::ResendContactReachabilityEmail,
            resend_operation_authorization::ResendOperationAuthorization,
            restore_domain::RestoreDomain,
            retrieve_domain_auth_code::RetrieveDomainAuthCode,
            transfer_domain::TransferDomain,
            transfer_domain_to_another_aws_account::TransferDomainToAnotherAwsAccount,
            update_domain_contact::UpdateDomainContact,
            update_domain_contact_privacy::UpdateDomainContactPrivacy,
            update_domain_nameservers::UpdateDomainNameservers,
            update_tags_for_domain::UpdateTagsForDomain,
            view_billing::ViewBilling,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated,
        }
    }
}

/// One modelled operation, served so that a request refused BEFORE its handler runs is
/// worded the way this service words it — with the value a clause quotes in hand, which
/// only the request carries.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`validation::stated_from_request`]), so this cannot refuse a request the
/// framework's own walk would accept, nor accept one it would refuse. The cost is that a
/// request is read twice, once here and once in the operation's own sequence, because the
/// framework hands no already-built input to an operation.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
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
            let refused = validation::stated_from_request::<O>(svc.definition().protocol(), req);
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
        // Claim a request whose resolved route key is this service's signing name, or
        // the `X-Amz-Target` spelling an unsigned awsJson request resolves to — see
        // [`TARGET_ROUTE_KEY`] for why the two differ for this service.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name() || key == TARGET_ROUTE_KEY,
            None => false,
        }
    }

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`StatedPerOperation`]
        // so a refused request is worded the way this service words it, with the value its
        // clause quotes. A key the model does not name resolves to `None` here exactly as
        // it does in the generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Route 53 Domains has ONE client-input error, and every operation in the
        // model lists it: `InvalidInput`, HTTP 400 — it is this service's
        // `common_errors` entry (smithy_query(query="service_info")). So a request
        // the framework refused against the model's own rules — an absent
        // `AdminContact`, a `DurationInYears` outside the modelled 1..10 range, a
        // `DomainName` past 255 characters — must come back under that code rather
        // than the framework's generic `ValidationException`, because that is the
        // code a Route 53 Domains client branches on.
        //
        // The wording is [`validation`]'s, the one place this service's refusal prose
        // lives, so a refusal that reaches this seam — one raised for an operation the
        // wrapped table does not hold — reads exactly like one the wrapper answered. This
        // path has no request in hand, so a clause stated at a collection member states no
        // value rather than one rebuilt from the violations.
        match refusal {
            Refusal::Invalid(violations) => validation::stated(violations, &HashMap::new()),
            // An unauthenticated caller is not an input problem: leave the
            // framework's 403 wording alone.
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
