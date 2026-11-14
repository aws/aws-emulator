// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-wafv2-awswaf-20190729` service: how it claims requests and how it words the
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

//! ## This service's other edited seam: the DATA plane a web ACL decides
//!
//! A web ACL is a decision about web requests, and the requests it decides reach the
//! PROTECTED RESOURCE's own endpoint rather than this service's. [`edge`] is that
//! decision — the one evaluation of a web ACL against one request — called by the
//! service that owns the resource (an API Gateway stage, say) when it serves a
//! request. It lives here, with this service, because the semantics are WAFv2's:
//! the rule order, the actions, the labels, the rate counts and the samples.
//! [`sqli`] is the injection detection the SQL and cross-site-scripting statements
//! inspect with. [`metrics`] is what that evaluation REPORTS to CloudWatch about the
//! requests each dimension acted on, which is the other half of a `VisibilityConfig`.
//! [`logs`] is what it DELIVERS about each request to the destination the web ACL's
//! `LoggingConfiguration` names, which is the other channel a web ACL reports on itself
//! through.

pub mod edge;
/// The log record a web ACL's `LoggingConfiguration` delivers about each request it
/// decided, and the write of that record into the destination it names.
pub mod logs;
/// The `AWS/WAFV2` series the data plane publishes: the per-dimension counters a
/// `VisibilityConfig$CloudWatchMetricsEnabled` asks WAF to send to CloudWatch.
pub mod metrics;
/// The judgement every operation makes about the request itself, before it resolves
/// anything the request names: the `Scope` against the endpoint it was signed for.
pub mod requests;
/// Where the `Scope` precondition is judged: ahead of the request boundary's own
/// refusals, which is the order WAF takes the two in.
pub mod scope_gate;
pub mod sqli;

use scope_gate::ScopeGate;
use smithy_server_runtime::{
    http, ErasedOperation, Operation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The wire error code a member-constraint failure carries, which is the code the
/// framework's own renderer picks for this class too: rewording the message leaves it
/// unchanged, so a client keeps branching on the code it already branches on.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The `com-amazonaws-wafv2-awswaf-20190729` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Every operation whose `Scope` RESOLVES a namespace, each the same operation the
    /// generated table holds and built from the same shared engine, wrapped in
    /// [`ScopeGate`] so that namespace is judged before the request boundary refuses
    /// anything else about the request. Keyed by the routing key the protocol resolves
    /// — the key the generated table uses — so a wrapped operation cannot be filed
    /// under a key no request resolves.
    gated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`ScopeGate`] over its `Scope` member and keyed by its operation shape name — the
/// routing key `ServerProtocol::routing_key` resolves and the generated table is keyed
/// by.
///
/// For the operations whose model marks `Scope` `@required`, so the input carries it
/// outright.
macro_rules! gate_required_scope {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(ScopeGate::over(
                    crate::ops::$module::$operation::new(Arc::clone(&$engine)),
                    |input| Some(input.scope.as_str()),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// [`gate_required_scope`] for the operations whose `Scope` is optional because the
/// request may address its target by ARN instead, which carries the namespace itself:
/// a request that names no scope names no namespace for the gate to judge.
macro_rules! gate_optional_scope {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(ScopeGate::over(
                    crate::ops::$module::$operation::new(Arc::clone(&$engine)),
                    |input| input.scope.as_deref(),
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
        let mut gated = OperationRoutingTable::new();
        // Every operation that RESOLVES something in the namespace its `Scope` names —
        // the whole set, not a family of it, because the precondition is the same
        // judgement about the same member whichever operation was called. The two
        // operations that take a `Scope` and are NOT here are named in
        // [`scope_gate`]'s own docs: `CreateWebACL`, which mints into a namespace
        // rather than resolving in one, and `GetTopPathStatisticsByTraffic`, whose
        // target is an ARN judged against the scope ahead of this precondition.
        gate_required_scope!(
            gated,
            engine,
            check_capacity::CheckCapacity,
            create_api_key::CreateAPIKey,
            create_ip_set::CreateIPSet,
            create_regex_pattern_set::CreateRegexPatternSet,
            create_rule_group::CreateRuleGroup,
            delete_api_key::DeleteAPIKey,
            delete_ip_set::DeleteIPSet,
            delete_regex_pattern_set::DeleteRegexPatternSet,
            delete_rule_group::DeleteRuleGroup,
            delete_web_acl::DeleteWebACL,
            describe_all_managed_products::DescribeAllManagedProducts,
            describe_managed_products_by_vendor::DescribeManagedProductsByVendor,
            describe_managed_rule_group::DescribeManagedRuleGroup,
            get_decrypted_api_key::GetDecryptedAPIKey,
            get_ip_set::GetIPSet,
            get_managed_rule_set::GetManagedRuleSet,
            get_rate_based_statement_managed_keys::GetRateBasedStatementManagedKeys,
            get_regex_pattern_set::GetRegexPatternSet,
            get_revenue_statistics_summary::GetRevenueStatisticsSummary,
            get_revenue_statistics_time_series::GetRevenueStatisticsTimeSeries,
            get_sampled_requests::GetSampledRequests,
            list_api_keys::ListAPIKeys,
            list_available_managed_rule_group_versions::ListAvailableManagedRuleGroupVersions,
            list_available_managed_rule_groups::ListAvailableManagedRuleGroups,
            list_ip_sets::ListIPSets,
            list_logging_configurations::ListLoggingConfigurations,
            list_managed_rule_sets::ListManagedRuleSets,
            list_regex_pattern_sets::ListRegexPatternSets,
            list_rule_groups::ListRuleGroups,
            list_settlement_records::ListSettlementRecords,
            put_managed_rule_set_versions::PutManagedRuleSetVersions,
            update_ip_set::UpdateIPSet,
            update_managed_rule_set_version_expiry_date::UpdateManagedRuleSetVersionExpiryDate,
            update_regex_pattern_set::UpdateRegexPatternSet,
            update_rule_group::UpdateRuleGroup,
            update_web_acl::UpdateWebACL,
        );
        // `ListWebACLs` is gated here rather than above although its model marks `Scope`
        // `@required`: it reads its input through `ListWebACLsWireInput`, which keeps an
        // absent scope as the absence it is so the handler can report the omission against
        // the request itself (`crate::state::critical_information_missing`) instead of the
        // request validator's member envelope. A request that names no scope names no
        // namespace for this gate to judge, which is exactly what the optional form says.
        gate_optional_scope!(
            gated,
            engine,
            get_rule_group::GetRuleGroup,
            get_web_acl::GetWebACL,
            list_web_ac_ls::ListWebACLs,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            gated,
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
        // [`ScopeGate`], so the namespace the request asks about is judged before the
        // request boundary refuses anything else about it. An operation the model names
        // and that table does not resolves to `None` here and is served by the
        // generated table unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.gated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // A value outside a declared bound is stated in WAF's own clause order and
        // with its `@pattern` reported as the full-match expression WAF advertises
        // (`crate::state::constraint_envelope`) — the SAME reporter a handler's
        // self-applied check goes through, so a member refused at the boundary reads
        // exactly like one refused by an operation. The code stays the one the
        // protocol chose for this class: `ValidationException` is what WAF answers a
        // member-constraint failure with, which is why only the message is reworded.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION_CODE.to_string(),
                message: crate::state::constraint_envelope(violations),
            }),
            // An authentication refusal is the framework's: this service neither
            // signs nor verifies a request.
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
