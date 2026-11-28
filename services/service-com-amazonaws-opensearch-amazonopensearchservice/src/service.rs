// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-opensearch-amazonopensearchservice` service: how it claims requests and how it words the
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
//! Two of those seams are taken for a second reason: a DOMAIN this service creates
//! publishes the endpoint "used to submit index, search, and data upload requests"
//! (`DomainStatus$Endpoint`), and that address is not this API's endpoint — so the front
//! door could resolve no target service for a request addressed to it and answered
//! `404 UnknownService`, for the one address this service told the caller to use.
//! [`can_handle`](Service::can_handle) also claims a request addressed to a domain this
//! service HOLDS, and [`domain_rest`] is what resolves and serves it: as an OPERATION
//! for a request presenting a SigV4 credential, so the emulator verifies the signature
//! and the handler can resolve which principal signed, and from
//! [`override_routing_error`](Service::override_routing_error) for one presenting none.
//! What the control plane publishes is a bare `host:port` of the domain's OWN listener
//! (`crate::state::domain_plane`), which the core serves; these seams claim the addressings a
//! caller reaches the same domain by at this front door.
//!
//! [`override_operation`](Service::override_operation) is taken for a third reason: a
//! request whose body carries a value the modelled member it is bound to cannot be read
//! from never becomes an input at all, and the framework renders the codec's own parser
//! diagnostic for it under the constraint-violation class. [`request_layer`] is what
//! answers such a request instead — the protocol's serialization error, naming the class
//! of value sent and the type it could not be read into.
//!
//! The claim and the routing-error seams carry one more address for the same reason a
//! domain's does: an APPLICATION this service creates publishes the URL a client follows to
//! reach its user interface (`GetApplicationResponse$endpoint`), and this emulator publishes
//! that URL on its OWN endpoint with the application named in the path, so a client of the
//! emulator can reach it. [`application_ui`] is the half that claims and serves it, out of
//! the stored application.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

pub mod domain_rest;
pub(crate) mod request_layer;
pub(crate) mod required_query_contract;
pub(crate) mod validation;

/// The route that answers at the endpoint an OpenSearch UI application reports, which this
/// emulator publishes on its own endpoint.
pub mod application_ui;

/// What the SERVICE, rather than the caller, puts in a domain's configuration: the
/// storage performance members it refuses, the option blocks it defaults for a request
/// that named none, and the members a descriptor derives at the moment of the read.
/// Reached by every operation that creates, updates or reports a domain, so no two of
/// them can report one stored record differently.
pub mod domain_configuration;

/// Whether the request these `parts` came from resolves to this service's signing name
/// AND addresses a first URI path segment this model publishes
/// (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here rather than re-implemented. It takes a whole
/// `http::Request` while [`Service::can_handle`] is handed only the request's parts, so
/// the two things it reads — the URI and the headers — are re-presented to it as a
/// bodiless request. It reads neither the body nor the method, so nothing it decides on is
/// missing.
///
/// `services/common`'s `claims_request` states the same rule over `Parts` and is what a
/// service under a shared signing name normally calls; it is not used here only because
/// depending on it edits `Cargo.lock`, which this repository's serialized squash merge
/// refuses whenever another change lands against it — the same reason
/// `services/service-com-amazonaws-wisdom-wisdomservice/src/service.rs` calls the
/// framework predicate directly and records the migration as a one-line follow-up. The
/// rule is identical: `claims_request`'s path conjunct for a REST model IS
/// `claims_modeled_path`, which mirrors this framework function.
fn claims_own_path_space(parts: &http::request::Parts, signing_name: &str) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        signing_name,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-opensearch-amazonopensearchservice` emulated service: the generated [`ServiceDefinition`] plus
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
    /// The endpoint an APPLICATION publishes, as an [`ErasedOperation`] for the same reason:
    /// so the router verifies the SigV4 credential a fetch of it presents before any stored
    /// application is read ([`CredentialedApplicationUi`]).
    credentialed_application_ui: CredentialedApplicationUi<Eng>,
    /// An operation with that operation's OWN validation of its input run first
    /// ([`StatedInputOperation`]): `StartMigration`, which states a constraint report over
    /// members this model declares no constraint trait for. Held in a field for the same
    /// reason as the data plane: `override_operation` hands back a reference borrowed from
    /// `&self`.
    stated_start_migration: StatedInputOperation,
    /// The request-DESERIALIZATION layer ([`request_layer`]): the answer to a request
    /// whose body carries a value the modelled member it is bound to cannot be read from.
    /// Held in a field for the same reason as the two above, and because it carries this
    /// service's input schemas, which are indexed once rather than per request.
    unreadable_body: request_layer::UnreadableBody,
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
/// what they serve — only in whether a signature was verified on the way in, and in
/// whether a calling principal could be resolved at all.
struct CredentialedDataPlane<Eng> {
    engine: Arc<Eng>,
}

/// The endpoint an APPLICATION publishes, for a fetch that PRESENTS a SigV4 credential.
///
/// Held for the same reason the credentialed data plane is: the router verifies a presented
/// credential only for what it names an OPERATION for, and this plane serves a stored
/// application. Naming it here is what makes a presented signature VERIFIED rather than merely
/// present; a fetch carrying no credential at all is admitted to no application at all
/// ([`crate::state::application_ui_admits`]) and is refused from the routing-error seam, which
/// the router consults before it authenticates anything.
///
/// It serves the request through exactly the function that seam uses
/// ([`application_ui::answer`]), so the credentialed and the uncredentialed path cannot
/// diverge in what they serve — only in whether a signature was verified on the way in.
struct CredentialedApplicationUi<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for CredentialedApplicationUi<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let _ = ctx;
            application_ui::answer(&*self.engine, req)
                // Unreachable: `override_operation` names this only for a request this plane
                // claims.
                .unwrap_or_else(|| svc.definition().routing_error_response(req))
        })
    }

    /// SigV4, the only scheme this service's model declares: naming it is what makes the
    /// router verify the credential this fetch carries, as it does for a modelled operation.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for CredentialedDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            if domain_rest::claims_request(&*self.engine, req) {
                // Serving is synchronous because this plane answers out of one store it
                // holds for the read or the write it performs. The one step that is neither —
                // the CPU-bound evaluation of the caller's own trigger conditions and message
                // templates — is driven by `served` with that store released and off this
                // async worker (`domain_rest::monitor_run::off_the_async_worker`), so no
                // caller-supplied source runs while the worker owes the runtime its health
                // checks, its timeouts and its part in a graceful shutdown.
                let answered = domain_rest::served(&*self.engine, req, Some(ctx));
                // The notifications the work this request did owes a channel are handed over
                // HERE: this service's store has been released by now, and the request has
                // not been answered yet, so a client that reads the channel's own
                // subscriptions after its request answered finds them there.
                crate::state::notification_publication::publish(
                    ctx.callback(),
                    &answered.notifications,
                )
                .await;
                return answered.response;
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
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            credentialed_data_plane: CredentialedDataPlane {
                engine: Arc::clone(&engine),
            },
            credentialed_application_ui: CredentialedApplicationUi {
                engine: Arc::clone(&engine),
            },
            stated_start_migration: StatedInputOperation {
                // The generated operation's own name, so this can never name a routing
                // key the generated table does not hold.
                routing_key: <crate::ops::start_migration::StartMigration<Eng> as Operation>::NAME,
                violations: crate::ops::start_migration::stated_input_violations,
            },
            unreadable_body: request_layer::UnreadableBody::new::<Eng>(),
            engine,
        }
    }
}

/// An operation with its OWN validation of the request's input run BEFORE the request
/// boundary reports on the constraints this model declares.
///
/// WHY a seam and not the handler: such an operation states ONE constraint report over
/// every member of its input it refuses, and some of those members carry no constraint
/// trait in this model at all — so the boundary, which reports only the traits and answers
/// before any handler runs, can neither state those clauses nor let a handler state them.
/// What the report SAYS is the operation's own rule
/// ([`crate::ops::start_migration::stated_input_violations`]); this type only runs it
/// first, and hands every request it refuses nothing in to the generated operation
/// unchanged.
///
/// An operation whose own check is of a VALUE rather than of a constraint the live service
/// declares states it from its handler instead, which the framework reaches only once the
/// boundary has accepted the request — `crate::ops::attach_data_source` is the one that
/// does — so such a check never speaks over the clause a member's declared constraint earns.
struct StatedInputOperation {
    /// The routing key the generated operation is wired under.
    routing_key: &'static str,
    /// That operation's own validation of the request's body.
    violations: fn(&[u8]) -> Vec<::aws_smithy_schema_server::Violation>,
}

impl ErasedOperation for StatedInputOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // A body the runtime has not buffered carries no member to validate, so this
            // validation states nothing and the request goes on to the generated
            // operation — where the boundary refuses it for the members the model
            // requires. Nothing is accepted that would otherwise have been refused.
            let body = req.body().bytes().unwrap_or_default();
            let violations = (self.violations)(body);
            if let Some(refused) = validation::stated_report(&violations) {
                // The clauses, the envelope and the code are the ones every other refusal
                // of this service states; the status and the error envelope are the
                // protocol's own for a refused input.
                return svc.definition().refusal_response(
                    aws_smithy_schema_server::ProtocolError::Invalid(violations),
                    Some(refused),
                    req,
                );
            }
            match svc.definition().routing_table().get(self.routing_key) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable: the key is the generated operation's own name, and the
                // generated table wires every modelled operation under it.
                None => svc.definition().routing_error_response(req),
            }
        })
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
        // SHARES with the legacy Amazon Elasticsearch Service model
        // (`services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015`;
        // `smithy_query(query="service_info")` on either gives sigv4Name `es`,
        // endpointPrefix `es`, arnNamespace `es`, protocol `restJson1`, differing only in
        // sdkId and in the API version they declare — `2021-01-01` here, `2015-01-01`
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
        // operation of this model is bound under `/2021-01-01/…` and every operation of the
        // legacy model under `/2015-01-01/…`, so the two first segments are disjoint and
        // each request has exactly one claimant.
        claims_own_path_space(parts, self.entry.identity().signing_name())
            // And a DOMAIN's own traffic: a request addressed to the endpoint host a
            // domain this service holds published ([`domain_rest::claims_parts`]). Such a
            // request names no modelled operation of either model under the shared `es`
            // signing name — its path is an OpenSearch REST path — so the front door
            // could resolve no claimant for it and answered `404 UnknownService`, for the
            // one address this service told the caller to send index and search requests
            // to.
            //
            // The claim is narrowed to a domain this service HOLDS, read from the one
            // shared store, rather than to a host or a path shape alone, so one request
            // keeps one claimant.
            || domain_rest::claims_parts(&*self.engine, parts)
            // And the endpoint an APPLICATION of this service reports (`endpoint`, "the
            // endpoint URL of the OpenSearch application"), which this emulator publishes
            // on its own front door with the application named in the path
            // ([`application_ui`]). Such a fetch is opened by whatever browser, probe or
            // test holds that URL: anonymously, with no credential scope and no target
            // naming this service — only the path this service itself minted, under a plane
            // segment that is this service's alone. It is therefore claimed whatever host
            // the client addressed, so the interface is reachable at the address the
            // application advertised.
            || application_ui::claimed(parts)
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
        //
        // Every other request resolves through the generated routing table.
        if domain_rest::claims_request(&*self.engine, req)
            && domain_rest::carries_sigv4_credential(req)
        {
            return Some(&self.credentialed_data_plane);
        }
        // And a fetch of the endpoint an APPLICATION publishes that presents a credential, for
        // the same reason: the router verifies a presented credential only for what it names
        // an operation for, and this plane serves a stored application
        // ([`CredentialedApplicationUi`]). A fetch presenting none is left unnamed on purpose —
        // it is refused by `override_routing_error` below, which the router reaches before
        // authentication, so a client carrying no AWS credential is answered at all.
        if application_ui::claims_request(req) && domain_rest::carries_sigv4_credential(req) {
            return Some(&self.credentialed_application_ui);
        }
        // A body value whose JSON class the modelled member it is bound to cannot be read
        // from never becomes an input at all, so it is answered by the deserialization
        // layer (see [`request_layer`]) before any operation runs — and before the
        // validation seam below, which reports on members that WERE read. WHICH value that
        // is comes from the model: the operation is the one the protocol's own routing key
        // names, and what each of its members takes is read off that operation's input
        // schema.
        if self
            .entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|operation| {
                self.unreadable_body
                    .refusal(operation, request_layer::request_body(req))
            })
            .is_some()
        {
            return Some(&self.unreadable_body);
        }
        // `StartMigration` is served through the seam that runs the operation's own
        // validation of the input first (see [`StatedInputOperation`]). WHICH request that is
        // comes from the model: the protocol resolves a request's routing key off the method
        // and the URI pattern the model declares, so this names no path of its own.
        let routing_key = self.entry.protocol().routing_key(req).ok();
        [&self.stated_start_migration]
            .into_iter()
            .find(|stated| routing_key == Some(stated.routing_key))
            .map(|stated| stated as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. The words live in [`validation`], which states the framework's own
        // clause for every violation with the renderings this service's refusals use —
        // the `@pattern` expression it advertises, an enumerated value set in the order
        // this front door's own hash table over the admitted values is walked, the
        // surface it validates a member against where the model has not caught up — plus
        // three statements about the message as a whole: a violation on an ELEMENT
        // of a collection member becomes ONE validation error about that member carrying
        // the element shape's whole constraint set, a member the request broke SEVERAL
        // constraints of is stated without the value it submitted, and a role ARN that
        // cannot be read as one replaces the report entirely. The code travels with the
        // message, as it must.
        validation::aws_validation_refusal(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. A request to a domain's own endpoint carrying no
        // SigV4 credential is exactly that case by construction, so this is where it is
        // served — BEFORE the ROUTER's authentication, which is what lets a client
        // carrying no AWS credential be answered at all.
        //
        // Being answered is not being admitted. The credential such a request does carry —
        // an HTTP Basic one, or none — is examined by the CLUSTER, which is the layer that
        // decides it on a domain endpoint: [`domain_rest`]'s own walk authenticates it
        // against the domain's internal user database before it reads anything of the
        // cluster (`crate::state::fgac::authentication`, which admits nobody anonymously on
        // a domain with fine-grained access control) and then authorizes it against the
        // domain's access policy and role mappings. A request this seam serves therefore
        // reaches exactly what the domain's own configuration admits it to, which is the
        // boundary this plane emulates; what the seam decides is only that the answer comes
        // from the cluster rather than from the front door's unknown-operation default.
        //
        // Anything else keeps the protocol-correct default for this service's envelope
        // (rest -> 501) by returning `None`.
        if domain_rest::claims_request(&*self.engine, req) {
            return Some(domain_rest::serve(&*self.engine, req, None));
        }
        // The same for a fetch of the endpoint an APPLICATION reports: it is answered here
        // because the router consults this seam before it authenticates anything, which is
        // what lets a client carrying no AWS credential be answered at all. What it is
        // answered is the refusal, not the application: a fetch presenting no credential is
        // admitted to no application ([`crate::state::application_ui_admits`]) and is told
        // nothing of one — not even whether the address names one. A fetch that DOES present
        // a credential is named as an operation above, so the router verified its signature
        // before this plane read anything.
        if let Some(served) = application_ui::answer(&*self.engine, req) {
            return Some(served);
        }
        // One class of request lands here that this service DOES answer: one whose
        // method and path address an operation of this API but which omits a member that
        // operation requires in the QUERY STRING — a required query binding being part of
        // the route the framework matches, so no route is selected for it and the
        // protocol default would answer 501 `NotImplemented`, claiming nobody implemented
        // the operation. Such a request IS that operation's request and is answered with
        // the refusal the absent member earns, worded by the same seam the boundary's own
        // refusals are ([`Service::override_refusal`]); see
        // [`required_query_contract`], which reads both the route and the member off the
        // model. A read of ONE resource whose required `@httpLabel` rendered EMPTY is in
        // that class: the empty label leaves the request on the COLLECTION path, whose
        // route requires the member naming the set to report.
        required_query_contract::refusal_response(
            self.definition(),
            self.entry.wire_protocol(req).protocol(),
            req,
        )
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
