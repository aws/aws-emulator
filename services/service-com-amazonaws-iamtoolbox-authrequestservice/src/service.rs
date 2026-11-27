// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iamtoolbox-authrequestservice` service: how it claims requests and how it words the
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
//!   correctly. This service uses it for one route: `GetRequestAuthorizationPolicy`
//!   binds a `@required` query parameter, and the REST route table names no operation at
//!   all when a request omits one — so the operation a request ADDRESSED, read off that
//!   operation's own `@http` binding, serves it and refuses the absent member as the
//!   modelled 400 instead of answering 501. Every request the routing key names is left
//!   alone.
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

use smithy_server_runtime::{http, ErasedOperation, Operation, Refusal, Refused, Service};
use std::sync::Arc;

/// This service's endpoint prefix, as the model gives it (`iam-toolbox`) — distinct from
/// its SigV4 signing name (`iam`), which the generated identity carries. `can_handle`
/// claims a request that names either, so a request addressed at this service's own
/// endpoint host (`iam-toolbox.<region>.amazonaws.com`, whose first label is the routing
/// key for an unsigned request) still reaches it.
pub const ENDPOINT_PREFIX: &str = "iam-toolbox";

/// The WIRED operations of this service whose model binds a `@required` member to the
/// query string, as `(the operation's name, its generated input schema)`.
///
/// Source: the model.
/// `smithy_query(query="operation",operation="GetRequestAuthorizationPolicy")` reports
/// `policyUri` as `required` with `httpQuery: "policyUri"`, and it is the only such member
/// on either operation this crate wires: `GetRequestAuthorizationDetails` requires only
/// its `@httpLabel`, so the lookup never withholds its route.
///
/// The row holds no method, no URI pattern and no parameter name: those are read off the
/// schema and the operation is taken out of the generated routing table, so an entry here
/// cannot fall out of step with the model.
const ROUTES_WITH_REQUIRED_QUERY: &[(&str, &aws_smithy_schema::Schema)] = &[(
    <crate::ops::get_request_authorization_policy::GetRequestAuthorizationPolicy<()> as Operation>::NAME,
    crate::ops::types::GET_REQUEST_AUTHORIZATION_POLICY_INPUT_SCHEMA,
)];

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first
/// URI path segments this service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means the model published no usable discriminator and
/// is "no constraint", and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a
/// service that must answer `POST /` claims the root path without claiming every path.
/// Matching is EXACT segment equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The `com-amazonaws-iamtoolbox-authrequestservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
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
        // This service signs as `iam` — a signing name it SHARES with AWS IAM
        // (`services/service-com-amazonaws-iam-awsidentitymanagementv20100508`), whose
        // own endpoint is `iam` while this one's is `iam-toolbox`. The router's routing
        // key comes from the SigV4 credential scope first, so both services resolve to
        // `iam` and matching on the signing name ALONE makes both claim every request —
        // which the router answers `500 AmbiguousService` for, to BOTH services' traffic.
        //
        // So the claim is narrowed the way the framework narrows it for the other
        // signing-name collisions: the routing key must name this service (its signing
        // name, or its own endpoint prefix for a request addressed by host) AND the
        // request must address this service's own modeled URI path space (see
        // `claims_modeled_path`), which here is the `/authorization-details/…` space its
        // one operation binds. IAM, the service whose endpoint prefix IS the shared
        // signing name, serves that name's remaining traffic (the awsQuery root path).
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() || key == ENDPOINT_PREFIX => {
                claims_modeled_path(parts, crate::ROUTE_PATH_SEGMENTS)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // `GetRequestAuthorizationPolicy` binds its `@required` `policyUri` to
        // `@httpQuery`, and the framework's REST route table rejects a candidate route
        // whose required bindings the request does not carry. That rule picks BETWEEN
        // operations sharing one method and URI template; here it leaves the only
        // operation of that route unnamed, so a call omitting `policyUri` falls to the
        // unknown-operation tail — `501 NotImplemented`, the claim that nobody has
        // written the operation yet (`crate::scaffolded`), for what is a bad request.
        //
        // Naming the operation it ADDRESSED hands such a request to the centralized walk,
        // whose boundary refuses the absent member as the `400 ValidationException` this
        // operation models for a request "missing one or more required parameters".
        //
        // Nothing about a route is written down here: the routing key is consulted FIRST,
        // so every request that routes today keeps the answer it had, and which requests
        // an operation addresses is read off that operation's own generated input schema —
        // its `@http` method and URI pattern — by the shared reader.
        let definition = self.definition();
        if definition
            .wire_protocol(req)
            .protocol()
            .routing_key(req)
            .is_ok()
        {
            return None;
        }
        let (operation, _) = ROUTES_WITH_REQUIRED_QUERY
            .iter()
            .find(|(_, schema)| ::service_common::addresses_modeled_http_route(req, schema))?;
        definition.routing_table().get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
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
