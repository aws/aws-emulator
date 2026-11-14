// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015` service: how it claims requests and how it words the
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
//! This API words a constraint refusal from the REQUEST — the member path, the value as
//! submitted, and the requirement as the service itself publishes it. One part of that
//! sentence the refusal seam cannot produce on its own: the value a clause states at a
//! COLLECTION member when one of its ELEMENTS failed. AWS reports such a failure at the
//! collection and echoes the collection the request carried, while the violation carries
//! the offending element alone, and
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is handed
//! the violations without the request they were found in.
//!
//! [`StatedPerOperation`] is where it comes from: each modelled operation is served wrapped
//! in it, the wrapper asks `service_common::validation` for the framework's OWN violations
//! against that operation's input schema together with the request's own lists, and
//! [`validation`] turns them into the sentence. `override_refusal` states a refusal that
//! reaches the centralized walk anyway through that same module, so the wording lives in ONE
//! place and a clause there simply has no collection to quote.

mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The DOMAIN's own REST data plane: the requests a client addresses to the endpoint a
/// domain of this service published, and what they are answered out of the indexes and
/// documents that domain holds.
///
/// `can_handle` claims such a request ([`domain_rest::claims_parts`]) and the two serving
/// seams answer it: [`Service::override_operation`] for one PRESENTING a SigV4 credential,
/// so the router verifies that credential before anything is served, and
/// [`Service::override_routing_error`] for one presenting none, which the router reaches
/// before authentication.
pub mod domain_rest;

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
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

/// Whether the request these `parts` came from resolves to this service's signing name
/// AND addresses a first URI path segment this model publishes
/// (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `services/common`'s `claims_request` is the fleet's owner of that rule and is CALLED
/// here rather than re-implemented: it states it over `Parts`, which is exactly what
/// [`Service::can_handle`] is handed. `None` for the RPC service-shape id because this
/// model is `restJson1`, so the discriminator is the model-derived path space alone —
/// the same conjunct the framework's own predicate applies.
fn claims_own_path_space(parts: &http::request::Parts, signing_name: &str) -> bool {
    ::service_common::claims_request(parts, signing_name, crate::ROUTE_PATH_SEGMENTS, None)
}

/// The `com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because a DOMAIN's own traffic ([`domain_rest`]) is claimed by
    /// `can_handle` and served from `override_operation` / `override_routing_error`, none
    /// of which is an operation with a `self.engine` of its own: all of them must read the
    /// one store the control-plane operations write, not a second copy.
    engine: Arc<Eng>,
    /// The domain REST data plane as an [`ErasedOperation`], so the router verifies the
    /// SigV4 credential a signed data-plane request carries before anything is served.
    /// Held in a field, built once, because `override_operation` hands back a reference
    /// borrowed from `&self`.
    credentialed_data_plane: CredentialedDataPlane<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words it.
    /// Keyed by the routing key the protocol resolves — the key the generated table uses —
    /// and built from the SAME shared engine, so a wrapped operation reads and writes the
    /// one store.
    stated: OperationRoutingTable,
}

/// The auth schemes a credentialed data-plane request is verified under.
///
/// SigV4 is the only scheme this service's model declares, and it is the one a client
/// signs a request to a domain's endpoint with, so a data-plane request arrives either
/// signed — which this names so the router's own authenticator verifies it — or unsigned,
/// which the routing-error seam answers without the router authenticating anything.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// The domain REST data plane for a request that PRESENTS a SigV4 credential.
///
/// It holds the SAME shared engine the generated entry threads into every operation, and
/// serves the request through exactly the function the routing-error seam uses
/// ([`domain_rest::serve`]), so the credentialed and the anonymous path cannot diverge in
/// what they serve — only in whether a signature was verified on the way in.
struct CredentialedDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for CredentialedDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        let _ = ctx;
        Box::pin(async move {
            if domain_rest::claims_request(&*self.engine, req) {
                // Serving is synchronous because this plane answers out of the one store
                // it holds for the read or the write it performs.
                return domain_rest::serve(&*self.engine, req);
            }
            // Unreachable: `override_operation` names this only for a request this data
            // plane claims.
            svc.definition().routing_error_response(req)
        })
    }

    /// This is NOT a modelled operation, so there is no modelled operation for the router
    /// to read schemes off: without this the router would find no scheme for a data-plane
    /// request and could not verify the credential it carries.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
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
        // that operation's own input schema. A still-scaffolded operation is wrapped too —
        // the framework refuses a bad request before any handler runs, so its refusals are
        // this service's words even while its success path is a 501. An operation added
        // later is added here as well, or it answers a refusal in the framework's wording
        // rather than this service's; `tests` below fails when one is missing.
        wrap_operations!(
            stated,
            engine,
            accept_inbound_cross_cluster_search_connection::AcceptInboundCrossClusterSearchConnection,
            add_tags::AddTags,
            associate_package::AssociatePackage,
            authorize_vpc_endpoint_access::AuthorizeVpcEndpointAccess,
            cancel_domain_config_change::CancelDomainConfigChange,
            cancel_elasticsearch_service_software_update::CancelElasticsearchServiceSoftwareUpdate,
            create_elasticsearch_domain::CreateElasticsearchDomain,
            create_outbound_cross_cluster_search_connection::CreateOutboundCrossClusterSearchConnection,
            create_package::CreatePackage,
            create_vpc_endpoint::CreateVpcEndpoint,
            delete_elasticsearch_domain::DeleteElasticsearchDomain,
            delete_inbound_cross_cluster_search_connection::DeleteInboundCrossClusterSearchConnection,
            delete_outbound_cross_cluster_search_connection::DeleteOutboundCrossClusterSearchConnection,
            delete_package::DeletePackage,
            delete_vpc_endpoint::DeleteVpcEndpoint,
            describe_domain_auto_tunes::DescribeDomainAutoTunes,
            describe_domain_change_progress::DescribeDomainChangeProgress,
            describe_elasticsearch_domain::DescribeElasticsearchDomain,
            describe_elasticsearch_domain_config::DescribeElasticsearchDomainConfig,
            describe_elasticsearch_domains::DescribeElasticsearchDomains,
            describe_elasticsearch_instance_type_limits::DescribeElasticsearchInstanceTypeLimits,
            describe_inbound_cross_cluster_search_connections::DescribeInboundCrossClusterSearchConnections,
            describe_outbound_cross_cluster_search_connections::DescribeOutboundCrossClusterSearchConnections,
            describe_packages::DescribePackages,
            describe_reserved_elasticsearch_instance_offerings::DescribeReservedElasticsearchInstanceOfferings,
            describe_reserved_elasticsearch_instances::DescribeReservedElasticsearchInstances,
            describe_vpc_endpoints::DescribeVpcEndpoints,
            dissociate_package::DissociatePackage,
            get_compatible_elasticsearch_versions::GetCompatibleElasticsearchVersions,
            get_package_version_history::GetPackageVersionHistory,
            get_upgrade_history::GetUpgradeHistory,
            get_upgrade_status::GetUpgradeStatus,
            list_domain_names::ListDomainNames,
            list_domains_for_package::ListDomainsForPackage,
            list_elasticsearch_instance_types::ListElasticsearchInstanceTypes,
            list_elasticsearch_versions::ListElasticsearchVersions,
            list_packages_for_domain::ListPackagesForDomain,
            list_tags::ListTags,
            list_vpc_endpoint_access::ListVpcEndpointAccess,
            list_vpc_endpoints::ListVpcEndpoints,
            list_vpc_endpoints_for_domain::ListVpcEndpointsForDomain,
            purchase_reserved_elasticsearch_instance_offering::PurchaseReservedElasticsearchInstanceOffering,
            reject_inbound_cross_cluster_search_connection::RejectInboundCrossClusterSearchConnection,
            remove_tags::RemoveTags,
            revoke_vpc_endpoint_access::RevokeVpcEndpointAccess,
            start_elasticsearch_service_software_update::StartElasticsearchServiceSoftwareUpdate,
            update_elasticsearch_domain_config::UpdateElasticsearchDomainConfig,
            update_package::UpdatePackage,
            update_vpc_endpoint::UpdateVpcEndpoint,
            upgrade_elasticsearch_domain::UpgradeElasticsearchDomain,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            credentialed_data_plane: CredentialedDataPlane {
                engine: Arc::clone(&engine),
            },
            stated,
            engine,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way this service
/// states it: a collection-element failure reported at its collection, quoting the
/// collection the REQUEST carried.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this
/// wrapper does not refuse is read twice, once here and once in the operation's own
/// sequence, because the framework hands no already-built input to an operation.
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
/// stated at a COLLECTION member needs and no violation carries. Nothing is re-checked here,
/// and no second copy of that walk lives in this crate: both halves are bounded where those
/// bounds are stated.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        ::service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
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
        // This service signs as `es` — a signing name, endpointPrefix AND arnNamespace it
        // SHARES with the modern Amazon OpenSearch Service model
        // (`services/service-com-amazonaws-opensearch-amazonopensearchservice`;
        // `smithy_query(query="service_info")` on either gives sigv4Name `es`,
        // endpointPrefix `es`, arnNamespace `es`, protocol `restJson1`, differing only in
        // sdkId and in the API version they declare — `2015-01-01` here, `2021-01-01`
        // there). AWS serves both APIs on the one `es.<region>.amazonaws.com` endpoint and
        // every SDK for either signs that one name, so the router — which resolves a
        // request's routing key from the SigV4 credential scope first — handed both
        // services the key `es`. Claiming on the signing name ALONE therefore made each
        // claim the other's traffic, the router found two claimants and answered `500
        // AmbiguousService` for EVERY request to the endpoint: a code no AWS model
        // declares, and one botocore retries, so a client burnt its retry budget before
        // failing.
        //
        // The claim is narrowed to this service's own modelled wire surface — the rule
        // every service under a shared signing name claims by, delegated to the
        // framework's own owner of it (see [`claims_own_path_space`]). This model is
        // `restJson1`, which binds every operation to a URI, so the discriminator is the
        // model-derived path space the generated `ROUTE_PATH_SEGMENTS` publishes — and it
        // is exactly how AWS separates the two APIs behind that one endpoint: every
        // operation of this model is bound under `/2015-01-01/…` and every operation of the
        // modern model under `/2021-01-01/…`, so the two first segments are disjoint and
        // each request has exactly one claimant.
        claims_own_path_space(parts, self.entry.identity().signing_name())
            // And a DOMAIN's own traffic: a request addressed to the endpoint a domain
            // this service holds published ([`domain_rest::claims_parts`]). Such a request
            // names no modelled operation of either model under the shared `es` signing
            // name — its path is an Elasticsearch REST path — so the front door could
            // resolve no claimant for it and answered `404 UnknownService`, for the one
            // address this service told the caller to send index and search requests to.
            //
            // The claim is narrowed to a domain this service HOLDS, read from the one
            // shared store, rather than to a host or a path shape alone, so one request
            // keeps one claimant.
            || domain_rest::claims_parts(&*self.engine, parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A data-plane request that PRESENTS a SigV4 credential is named as an operation,
        // so the router verifies that credential's signature before anything is served —
        // the router authenticates only what it names an operation for. One carrying no
        // credential is left unnamed on purpose: it is answered by
        // `override_routing_error` below, which the router reaches before authentication.
        if domain_rest::claims_request(&*self.engine, req)
            && domain_rest::carries_sigv4_credential(req)
        {
            return Some(&self.credentialed_data_plane);
        }
        // Otherwise NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `StatedPerOperation` so a refused request is worded the way this service words
        // it, with the collection a clause states read from the request. A key the model
        // does not name resolves to `None` here exactly as it does in the generated table,
        // so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. It is handed the violations ALONE — no request — so it cannot state
        // the collection a failed element belongs to; `StatedPerOperation` answers a
        // modelled operation's refusals before this walk is reached, with that in hand. A
        // refusal that gets here anyway is still stated by this service, through the SAME
        // module, so the wording cannot differ between the two paths — it just has only
        // the violations to state from.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => validation::refusal(violations, HashMap::new),
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
        // A DATA-PLANE request carrying no AWS credential arrives here, before the router
        // authenticates anything, and is answered by the plane that claimed it: the
        // cluster's own envelope, out of the same serving function the credentialed path
        // uses.
        if domain_rest::claims_request(&*self.engine, req) {
            return Some(domain_rest::serve(&*self.engine, req));
        }
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
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015.json"
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
        assert!(
            !operations.is_empty(),
            "case the model's surface: a service model declares operations"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        let mut served = 0_usize;
        for operation in operations {
            if service.entry.routing_table().get(operation).is_none() {
                // An operation the model BINDS and this crate generated no handler for is
                // served by nothing, so there is nothing to wrap: the generated table does
                // not hold it either, and a request naming it takes the router's
                // routing-error path.
                continue;
            }
            served += 1;
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every operation the generated table serves is served \
                 behind StatedPerOperation; this one is missing from wrap_operations!, so \
                 it answers a refusal in the framework's wording instead of this service's"
            );
        }
        assert!(
            served >= 50,
            "case the served surface: expected every generated operation to be checked, got \
             {served}"
        );
    }
}
