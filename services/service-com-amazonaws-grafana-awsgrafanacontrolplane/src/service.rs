// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-grafana-awsgrafanacontrolplane` service: how it claims requests and how it words the
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
//! - `override_protocol` — the wire protocol this service serves. The default
//!   returns `None`, so the generated entry's own protocol renders every request
//!   and response. This service returns its model's `restJson1` with one framing
//!   correction, see [`no_content_framing`].
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The workspace data plane: the native Grafana HTTP API
//!
//! `WorkspaceDescription$endpoint` is "The URL that users can use to access the
//! Grafana console in the workspace", so it is a promise that a caller can follow it:
//! reading the field and calling Grafana's own REST API under it is what a client that
//! minted a workspace API key or a service-account token does with it. That API is not
//! in the Smithy model, so it is claimed by the ADDRESS the request names and the
//! CREDENTIAL it presents — this emulator's own announced endpoint with a bearer key that
//! names the workspace, the announced endpoint's workspace path, or the host the real
//! service publishes for a workspace — and served from the two seams below;
//! [`workspace_api`] is the module that answers it.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

pub mod authorization;
pub mod no_content_framing;
pub mod workspace_api;

use no_content_framing::NoContentRestJson;

/// The error code the service's request validator answers a request whose BODY
/// does not satisfy the model's own rules with, and the message it states.
///
/// This is recorded behaviour, not a modelled error: Amazon Managed Grafana
/// fronts its control plane with a request validator that checks the request body
/// against the operation's schema and refuses one that fails it with the
/// unmodelled code `BadRequestException` and the message "Invalid request body" —
/// the message names the body because the body is what it validated. A member
/// bound to the URI (a `@httpLabel` path segment, a `@httpQuery` parameter) is not
/// part of that body, so a refusal of one keeps the modelled `ValidationException`
/// the service itself raises.
const BAD_REQUEST_CODE: &str = "BadRequestException";
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The members this service carries in the request URI rather than in the body:
/// every `@httpLabel` and `@httpQuery` member of any operation of
/// `com.amazonaws.grafana#AWSGrafanaControlPlane`.
///
/// Source: the model, read per operation with
/// `smithy_query(query="operation", operation="...")` — the labels are the
/// segments of the operations' own `@http` URIs (`/workspaces/{workspaceId}`,
/// `/workspaces/{workspaceId}/licenses/{licenseType}`,
/// `/workspaces/{workspaceId}/apikeys/{keyName}`,
/// `/workspaces/{workspaceId}/serviceaccounts/{serviceAccountId}/tokens/{tokenId}`,
/// `/tags/{resourceArn}`) and the query members are the pagination and filter
/// parameters (`maxResults`, `nextToken`, `tagKeys`, `userId`, `userType`,
/// `groupId`, and `ListVersions`' own `workspace-id`).
///
/// `keyName` is deliberately NOT here: `CreateWorkspaceApiKeyRequest` carries it
/// in the BODY, and the refusal renderer is handed the violated member without the
/// operation, so the member is classified by the binding it has where a request
/// body carries it at all.
const URI_BOUND_MEMBERS: [&str; 10] = [
    "workspaceId",
    "licenseType",
    "serviceAccountId",
    "tokenId",
    "resourceArn",
    "maxResults",
    "nextToken",
    "tagKeys",
    "userId",
    "userType",
];

/// How this service words the refusal of a URI-bound member whose value is outside
/// the `@pattern` the model declares for it: `The <member> should satisfy the
/// pattern <pattern>.`
///
/// Recorded behaviour: Amazon Managed Grafana states a pattern violation from the
/// member's OWN constraint — it names the member and the pattern the model carries
/// and does NOT echo the rejected value, where the framework's multi-service default
/// quotes the value back. The code and the status stay the modelled
/// `ValidationException` / 400, which is what this service raises for a member it
/// reads out of the request URI.
fn pattern_refusal(member: &str, pattern: &str) -> Refused {
    Refused {
        code: VALIDATION_CODE.to_string(),
        message: format!("The {member} should satisfy the pattern {pattern}."),
    }
}

/// The modelled error code a refusal of a URI-bound member keeps.
const VALIDATION_CODE: &str = "ValidationException";

/// How this service states a refusal of one violated member, or `None` to leave
/// the framework's wording (and the modelled `ValidationException` code) in place.
///
/// A violation of a member the request carries in its body is the request
/// validator's, so it is answered with [`BAD_REQUEST_CODE`]; a violation of a
/// URI-bound member is the service's own and keeps the modelled code — with the
/// service's own wording for a `@pattern` violation ([`pattern_refusal`]). Absence is
/// not reworded: the recorded refusal is of a value that was PRESENT and outside a
/// bound the model declares, so that is the class this answers for.
fn refusal_for(violation: &Violation) -> Option<Refused> {
    // A nested path names its top-level member first
    // (`samlConfiguration.idpMetadata`), and that member is the one the body
    // carries.
    let member = violation.path.split('.').next().unwrap_or_default();
    if matches!(violation.kind, ViolationKind::MissingRequiredMember) {
        return None;
    }
    if URI_BOUND_MEMBERS.contains(&member) {
        // Only a pattern violation is reworded: that is the constraint this service
        // states in its own form. Every other bound on a URI-bound member keeps the
        // framework's wording, which is the form this service has not been recorded
        // stating differently.
        return match &violation.kind {
            ViolationKind::Pattern(pattern) => Some(pattern_refusal(&violation.path, pattern)),
            _ => None,
        };
    }
    Some(Refused {
        code: BAD_REQUEST_CODE.to_string(),
        message: INVALID_REQUEST_BODY.to_string(),
    })
}

/// The `com-amazonaws-grafana-awsgrafanacontrolplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same shared engine the generated entry constructs each operation from.
    ///
    /// A workspace's own Grafana HTTP API ([`workspace_api`]) is not a modelled
    /// operation — the model describes the control plane only — so it is served
    /// from `override_routing_error` below, which is handed the request but no
    /// operation. It reaches this service's store through this handle, exactly as
    /// a handler reaches it through its own `self.engine`.
    engine: Arc<Eng>,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it: the model's own
    /// `restJson1`, with the framing a tag mutation's no-content answer needs (see
    /// [`no_content_framing`]).
    protocol: NoContentRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            protocol: NoContentRestJson::new(),
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
        let route_key = smithy_server_runtime::resolve_route_key(parts);
        if route_key
            .as_deref()
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request to a workspace's own Grafana HTTP API, which is claimed by
        // the ADDRESS it names and the CREDENTIAL it presents, because that is all
        // that identifies it: a client holding a workspace API key or a
        // service-account token presents it as a bearer token, with no SigV4
        // credential and no `X-Amz-Target`, and addresses the endpoint the control
        // plane announced for the workspace (this emulator's own authority, where the
        // credential names the workspace), the workspace's path beneath that
        // authority, or the host the real service publishes for it. Without this the
        // router answers `UnknownService` for an endpoint this service handed out.
        let host = workspace_api::addressed_host(
            parts
                .headers
                .get(::http::header::HOST)
                .and_then(|host| host.to_str().ok()),
            parts.uri.authority().map(::http::uri::Authority::as_str),
        );
        // `http::Uri::path` already strips the scheme, the authority and the
        // query, for an origin-form request line and an absolute-form one alike.
        workspace_api::route(&workspace_api::Dialled {
            method: parts.method.as_str(),
            host,
            path: parts.uri.path(),
            authorization: parts
                .headers
                .get(::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            this_service: self.entry.identity().signing_name(),
        })
        .is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Amazon Managed Grafana does not answer the modelled `ValidationException`
        // for a value its request validator rejected in the request BODY: that
        // refusal carries the unmodelled code `BadRequestException`. A client
        // branches on the code, so both the code and the message are replaced
        // here. See [`refusal_for`] for which members that covers and why a
        // URI-bound member keeps the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => violations.iter().find_map(refusal_for),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A request to a workspace's own Grafana HTTP API is exactly such a
        // request and is served here: that API is not in the Smithy model, so it
        // names no modelled operation and no routing key, and it carries no SigV4
        // signature for the framework to verify (its caller authenticates with a
        // workspace API key or a service-account token, presented as a bearer
        // token). It is answered in the Grafana API's own JSON envelope rather
        // than in an AWS error envelope, because a Grafana client is what reads
        // it.
        let path = workspace_api::uri_path(req.uri());
        let parsed = req.uri().parse::<::http::Uri>().ok();
        let host = workspace_api::addressed_host(
            req.headers().get("host"),
            parsed
                .as_ref()
                .and_then(::http::Uri::authority)
                .map(::http::uri::Authority::as_str),
        );
        let authorization = req.headers().get("authorization");
        let request = workspace_api::route(&workspace_api::Dialled {
            method: req.method(),
            host,
            path: &path,
            authorization,
            this_service: self.entry.identity().signing_name(),
        })?;
        let now = ::std::time::SystemTime::now();
        let response = match crate::state::service_state(&*self.engine) {
            Some(state) => {
                workspace_api::serve(&mut state.lock(), &request, authorization, now)
            }
            // The store was not mounted, so nothing can be said about the
            // workspace or the key presented; a server fault is the only honest
            // answer.
            None => workspace_api::Response {
                status: 500,
                body: br#"{"message":"the grafana workspace store is unavailable"}"#.to_vec(),
            },
        };
        let mut rendered = aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(response.status).unwrap_or_else(
                |_| {
                    aws_smithy_runtime_api::http::StatusCode::try_from(500u16)
                        .expect("500 is a status")
                },
            ),
            aws_smithy_types::body::SdkBody::from(response.body),
        );
        rendered
            .headers_mut()
            .insert("content-type", workspace_api::CONTENT_TYPE);
        // What a key authenticates is a snapshot the next control-plane call may
        // move (a delete revokes it), so no answer here may be cached.
        rendered.headers_mut().insert("cache-control", "no-store");
        Some(rendered)
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own `restJson1`, with one framing correction on the way out: a
        // tag mutation's success carries no content, so it carries no body rather
        // than the empty JSON object the renderer writes for a members-less output
        // structure (see [`no_content_framing`]). Every answer that does carry
        // content is left exactly as rendered.
        Some(&self.protocol)
    }
}
