// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sso-swbportalservice` service: how it claims requests and how it words the
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
//!   correctly, or one this service answers before the framework reads it. The default
//!   returns `None`, so every request resolves through the generated routing table.
//!   Return `Some(op)` to serve one yourself. The portal answers its authentication
//!   refusal here, because that refusal precedes reading anything else of the request
//!   (see [`session_gate`]).
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

mod session_gate;

use session_gate::SessionGate;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// This service's modeled endpoint prefix (`portal.sso`), as `OPERATIONS.md` and the
/// model's `awsService` trait give it. Used by [`EmulatedService::can_handle`] to claim
/// an unsigned request addressed to the portal endpoint.
const PORTAL_ENDPOINT_PREFIX: &str = "portal.sso";

/// The host a request is addressed to, from the `Host` header or (HTTP/2) the URI
/// authority, lowercased with any `:port` removed. `None` when the request names none.
fn endpoint_host(parts: &http::request::Parts) -> Option<String> {
    let host = parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| parts.uri.authority().map(|a| a.as_str().to_string()))?;
    let without_userinfo = host.rsplit('@').next().unwrap_or(&host).to_string();
    let without_port = without_userinfo
        .split(':')
        .next()
        .unwrap_or(&without_userinfo)
        .to_ascii_lowercase();
    (!without_port.is_empty()).then_some(without_port)
}

/// Whether `host` is this service's own endpoint: the modelled prefix itself, or that
/// prefix followed by a dot (`portal.sso.<region>.amazonaws.com`). A dot is required so
/// a host that merely STARTS with the prefix (`portal.ssoother.example`) is not this
/// endpoint.
///
/// What follows that dot is DELIBERATELY unconstrained: the model publishes the endpoint
/// prefix, not the region labels or the partition's DNS suffix, so pinning a suffix here
/// would be invented data, and an emulator is reached through whatever endpoint its
/// operator configured. A host naming another partition is served; only the fidelity of
/// endpoint resolution — which this emulator does not model — would refuse it.
fn is_portal_endpoint(host: &str) -> bool {
    host == PORTAL_ENDPOINT_PREFIX
        || host
            .strip_prefix(PORTAL_ENDPOINT_PREFIX)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Whether `host` is the published endpoint of a service OTHER than this one: an endpoint
/// host AWS itself publishes
/// (`service_common::data_plane_addressing::addresses_an_aws_endpoint`, the one reading of
/// that question in this tree) that is not this service's own endpoint
/// ([`is_portal_endpoint`]).
///
/// Such a request belongs to the service its endpoint names, even on a path this model
/// binds, so it is not claimed here. Every other host — the emulator's own address, an IP
/// literal, a loopback name, an ordinary hostname an operator points a client at — names NO
/// service, however the framework's host-style tier reads its first label, and so neither
/// attributes a request nor withholds it from this service.
fn names_another_services_endpoint(host: &str) -> bool {
    !is_portal_endpoint(host)
        && ::service_common::data_plane_addressing::addresses_an_aws_endpoint(host)
}

/// Whether anything OUT OF BAND in `parts` names a target service: a SigV4 credential
/// scope — in the `Authorization` header, or in a presigned URL's `X-Amz-Credential`
/// query parameter — or an `X-Amz-Target`.
///
/// These are the signals the framework's own tiered resolution reads AHEAD of the
/// endpoint host, and each of them names a service outright. The framework's readers are
/// called rather than re-implemented, so "this request names a service" means here exactly
/// what it means to the router.
///
/// A portal request carries none of them: every operation is `@optionalAuth` with an empty
/// `@auth`, so an SDK sends it unsigned, and `restJson1` has no `X-Amz-Target`.
fn names_a_service_out_of_band(parts: &http::request::Parts) -> bool {
    ::emulator_server::service_from_sigv4(&parts.headers).is_some()
        || ::emulator_server::service_from_sigv4_query(&parts.uri).is_some()
        || ::emulator_server::service_from_target(&parts.headers).is_some()
}

/// Where an http-bound member rides on the wire.
enum BoundIn {
    Header,
    Query,
}

/// One `@required` http-bound member of a route: the Smithy member name, where it
/// rides, and the name it rides under.
struct RequiredMember {
    member: &'static str,
    bound_in: BoundIn,
    name: &'static str,
}

const ACCESS_TOKEN: RequiredMember = RequiredMember {
    member: "accessToken",
    bound_in: BoundIn::Header,
    name: "x-amz-sso_bearer_token",
};
const ACCOUNT_ID: RequiredMember = RequiredMember {
    member: "accountId",
    bound_in: BoundIn::Query,
    name: "account_id",
};
const ROLE_NAME: RequiredMember = RequiredMember {
    member: "roleName",
    bound_in: BoundIn::Query,
    name: "role_name",
};

/// One route this service serves: the method and the path of an `@http` binding, and the
/// `@required` http-bound members of that operation.
type Route = (&'static str, &'static str, &'static [RequiredMember]);

/// Every route this service serves and the `@required` http-bound members of each, as
/// the model declares them (`OPERATIONS.md` lists the same four).
///
/// `restJson1` selects a route only when every one of those members is on the request,
/// so a request that omits one matches no operation at all — including the one that
/// omits the bearer credential, which is why the portal's authentication gate has to be
/// answered in [`Service::override_routing_error`] as well as in every handler. This
/// table is what lets that seam tell a request addressed to one of these routes — one
/// presenting no session, or a readable request missing a required parameter — from a
/// path this service does not serve, and it is also the request SIGNATURE
/// [`serves_route`] claims an anonymous portal request by.
///
/// DRIFT: this is model-derived data held by hand, because the generated
/// `ServiceDefinition`'s routing table exposes no required-member set to read it from.
/// A route or required member the model gains and this table misses is not answered
/// WRONGLY, just unreworded — the request falls back to the framework's routing
/// answer. The invalid-request test in `tests/all/portal_session.rs` carries one row
/// per (route, required member) pair here, so a row that disagrees with the model
/// fails there, and `tests/all/endpoint_agnostic_routing.rs` carries one row per
/// route.
const ROUTES: &[Route] = &[
    ("GET", "/assignment/accounts", &[ACCESS_TOKEN]),
    ("GET", "/assignment/roles", &[ACCESS_TOKEN, ACCOUNT_ID]),
    (
        "GET",
        "/federation/credentials",
        &[ACCESS_TOKEN, ACCOUNT_ID, ROLE_NAME],
    ),
    ("POST", "/logout", &[ACCESS_TOKEN]),
];

/// The route in [`ROUTES`] that `method` and `uri` address, or `None` when they address
/// none of them.
///
/// The ONE place this service turns a request into a route, for both callers below, so
/// that claiming a request and wording its refusal cannot disagree about what path it
/// named. The path is whatever `uri` reports as its path component — no scheme, no
/// authority, no query string — whichever form the request arrived in: origin
/// (`/assignment/accounts?max_result=1`) or absolute
/// (`https://host/assignment/accounts?max_result=1`). Nothing is split by hand here; the
/// URI type the `http` crate already parses this request into answers it.
///
/// Matching is EXACT on both halves, never a prefix: a path this model does not bind
/// matches no route (so it stays the router's own `UnknownService`) rather than being
/// swallowed here. It recognises one of this service's own four literal routes, never
/// binds a value out of the path.
fn matched_route(method: &str, uri: &http::Uri) -> Option<&'static Route> {
    let path = uri.path();
    ROUTES
        .iter()
        .find(|(route_method, route, _)| *route_method == method && *route == path)
}

/// Whether the request's query string carries `name` as a key, with or without a value
/// — the same presence test the REST router applies to a required query binding.
fn query_has(uri: &http::Uri, name: &str) -> bool {
    uri.query().is_some_and(|query| {
        query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .any(|pair| pair.split_once('=').map_or(pair, |(key, _)| key) == name)
    })
}

/// Whether `parts` addresses one of this service's own modelled routes: its method and
/// its path are an `@http` binding in [`ROUTES`].
///
/// This is the request's own SIGNATURE — the same (method, path) pair `restJson1` routes
/// on — and for a portal request it is the only service-identifying signal there is.
/// Every portal operation is `@optionalAuth` with an empty `@auth`, so an SDK sends it
/// unsigned: there is no SigV4 credential scope and `restJson1` carries no
/// `X-Amz-Target`. All that is left is the endpoint the client was pointed at, and no
/// endpoint that is not a service's published host names a service — the emulator's own
/// `localhost:9297`, an IP literal, no host at all, or an ordinary hostname (a compose
/// service name, a reverse proxy, a corporate DNS name) whose first label the host-style
/// tier does resolve a routing key from while naming no service with it. Claiming by this
/// table is what makes the service reachable at every one of them.
///
/// Matching is [`matched_route`]'s, which is also what the refusal wording below reads,
/// so a request claimed here is a request that seam recognises.
fn serves_route(parts: &http::request::Parts) -> bool {
    matched_route(parts.method.as_str(), &parts.uri).is_some()
}

/// The `@required` members of `req`'s route that are absent from it, or `None` when
/// `req` names no route this service serves.
fn absent_required_members(req: &aws_smithy_runtime_api::http::Request) -> Option<Vec<String>> {
    // The runtime-API request carries its URI as a string, so it is read back through the
    // same URI type [`serves_route`] is handed: one spelling of "this request's path" for
    // both seams. A string that is no URI names no route, and the framework's own routing
    // answer stands.
    let uri: http::Uri = req.uri().parse().ok()?;
    let (.., required) = matched_route(req.method(), &uri)?;
    Some(
        required
            .iter()
            .filter(|member| match member.bound_in {
                BoundIn::Header => req.headers().get(member.name).is_none(),
                BoundIn::Query => !query_has(&uri, member.name),
            })
            .map(|member| member.member.to_string())
            .collect(),
    )
}

/// The bearer credential `req` presents: the value of the header [`ACCESS_TOKEN`] names,
/// the one the model binds the access token to. `None` when the request does not carry
/// that header at all — a request presenting NO credential.
fn presented_access_token(req: &aws_smithy_runtime_api::http::Request) -> Option<&str> {
    debug_assert!(matches!(ACCESS_TOKEN.bound_in, BoundIn::Header));
    req.headers().get(ACCESS_TOKEN.name)
}

/// The `com-amazonaws-sso-swbportalservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The portal's authentication gate, on the SAME shared engine the generated entry
    /// threaded into every operation: it resolves a presented token against the one store
    /// the handlers read, and serves the refusal to a request that presents no usable
    /// session. Both seams below ask it, so the two cannot disagree (see
    /// [`session_gate`]).
    session_gate: SessionGate<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            session_gate: SessionGate::new(engine),
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
        let resolved = smithy_server_runtime::resolve_route_key(parts);
        // Claim a request whose resolved route key is this service's signing name
        // (`awsssoportal`).
        if resolved.as_deref() == Some(self.entry.identity().signing_name()) {
            return true;
        }
        // SOMETHING named a service OUT OF BAND — a SigV4 credential scope or an
        // `X-Amz-Target` — and it was not this service: that naming is the router's own
        // strongest signal, so the request is somebody else's whatever path it addresses,
        // and two claimants are an error to the router.
        if names_a_service_out_of_band(parts) {
            return false;
        }
        // What is left is the normal shape of a portal request: nothing but the endpoint
        // HOST, which is the weakest routing signal there is and the only one a client
        // pointed at this emulator can carry. A host names a service only when it is a
        // service's PUBLISHED endpoint:
        //
        // - this service's own, which the model expresses but the routing key cannot
        //   carry (the host-style tier yields only the endpoint's first label, `portal`),
        //   claimed WHOLE — so a path this model does not bind gets this service's own
        //   routing answer rather than the router's;
        // - another service's, which is that service's request even on a path this model
        //   binds, so it is not claimed here.
        //
        // Every OTHER host names no service: the emulator's own address, an IP literal, a
        // loopback name, no `Host` header at all — and equally an ordinary hostname an
        // operator points a client at (a compose service name, a reverse proxy, a
        // corporate DNS name), which the framework's host-style tier does resolve a
        // routing key from, by taking the host's first label, while that key names no
        // service at all. So such a host can neither attribute the request nor withhold
        // it, and what attributes it is the one signal left — its own modelled signature,
        // the method and path of one of this service's four `@http` bindings. Nothing
        // weaker is claimed: an unmodelled path on such a host reaches no service.
        match endpoint_host(parts) {
            Some(host) if is_portal_endpoint(&host) => true,
            Some(host) if names_another_services_endpoint(&host) => false,
            _ => serves_route(parts),
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // AUTHENTICATION FIRST, for a request the model DOES name an operation for. This
        // is the one seam that sees the whole request before the framework reads any of
        // it, and the portal's authentication precedes reading anything else: everything
        // this service answers comes out of the sign-in session the request's bearer
        // token names, so a request presenting no usable session is an unauthenticated
        // request and is refused as one — whatever else it did or did not carry. Asked
        // any later, the rule could not hold: the framework validates the request's
        // modelled members before a handler runs, so a value outside a declared bound
        // would be worded by `override_refusal` below while the handler's own gate never
        // ran.
        //
        // Only a request the model names an operation for is claimed here. One that
        // names none — a path this service does not bind, or a route missing a
        // `@required` http-bound member — still falls to `override_routing_error` below,
        // which answers the same rule from the other side.
        let routing_key = self
            .entry
            .wire_protocol(req)
            .protocol()
            .routing_key(req)
            .ok()?;
        self.entry.routing_table().get(routing_key)?;
        (!self.session_gate.presents_a_usable_session(req))
            .then_some(&self.session_gate as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // This service states a request it will not read as `InvalidRequestException`,
        // the error its model declares for exactly that — "a required parameter might
        // be missing or out of range" — rather than the framework's generic
        // `ValidationException`. The status (400) is the protocol's and matches the
        // modelled `@httpError`. Both the code and the message are replaced together.
        //
        // A request presenting no usable session never reaches this seam: the gate in
        // `override_operation` above answers it first, so what is left to word here is a
        // request from an AUTHENTICATED caller that this service will not read.
        if let Refusal::Invalid(violations) = refusal {
            return Some(Refused {
                code: "InvalidRequestException".to_string(),
                message: aws_smithy_schema_server::envelope(violations)
                    .unwrap_or_else(|| "the request is not valid".to_string()),
            });
        }
        // The only other refusal class is the EMULATOR's own request authentication
        // refusing the caller, which is not this service's sign-in session: the portal's
        // modelled authentication refusal (`UnauthorizedException`, 401, worded by
        // `crate::state::unauthorized_session`) is answered before any value of the
        // request is read — by the gate in `override_operation` above, by
        // `override_routing_error` below, and by every handler — so restating it here
        // would reword somebody else's refusal as this service's.
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // A request that named no operation but DOES name one of this service's routes
        // got there by omitting a required, http-bound member: the REST router selects a
        // route only when every one of them is present.
        let absent = absent_required_members(req)?;

        // AUTHENTICATION FIRST. The portal has no other caller identity: everything it
        // answers comes out of the sign-in session the request's bearer token names, so
        // a request presenting no usable session is an unauthenticated request and is
        // refused as one — `UnauthorizedException`, with the status its model carries —
        // whatever else the request did or did not carry. That covers the request whose
        // absent member IS the credential: an absent credential is not a malformed
        // parameter, and it is exactly the case no handler's own gate can answer,
        // because the router never selects a route without it.
        if !self.session_gate.presents_a_usable_session(req) {
            return crate::state::unauthorized_response(self.entry.protocol());
        }

        // The caller is authenticated, so what is left is a missing required parameter,
        // which this service's model has an error for (`InvalidRequestException`, "a
        // required parameter might be missing or out of range"), so it is stated as that
        // rather than as "not implemented".
        let violations: Vec<aws_smithy_schema_server::Violation> = absent
            .into_iter()
            .map(|member| aws_smithy_schema_server::Violation {
                path: member,
                kind: aws_smithy_schema_server::ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            })
            .collect();
        // Nothing was absent after all, so the miss is not about a required member and
        // the framework's own routing answer stands.
        let message = aws_smithy_schema_server::envelope(&violations)?;
        Some(self.entry.refusal_response(
            aws_smithy_schema_server::ProtocolError::Invalid(violations),
            Some(Refused {
                code: "InvalidRequestException".to_string(),
                message,
            }),
            req,
        ))
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
