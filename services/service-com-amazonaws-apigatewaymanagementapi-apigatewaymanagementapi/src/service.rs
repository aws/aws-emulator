// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi` service: how it claims requests and how it words the
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
//! - `override_protocol` — this service's wire protocol for the dispatch sequence.
//!   It serves the model's own `restJson1`, with the ERROR BODY this service is
//!   recorded answering: see [`rest_json_error_body`].
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

/// The ENDPOINT a `@connections` request was addressed through, and the refusal an endpoint
/// serving no deployed stage earns.
mod connections_endpoint;
/// Whether IAM lets the caller MANAGE the connections of the stage it addressed — the
/// `execute-api:ManageConnections` decision the three operations take before they read the
/// connection store.
pub(crate) mod manage_connections_authorization;
/// What this service's error body carries on the wire: the error shape's own modelled
/// members and nothing else.
mod rest_json_error_body;

use smithy_server_runtime::{http, BoxFuture, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// The path segment every modeled operation of this service is bound under:
/// `GetConnection`/`PostToConnection`/`DeleteConnection` all bind
/// `/@connections/{ConnectionId}`
/// (`smithy_query(query="operation", operation="GetConnection,PostToConnection,DeleteConnection")`).
const CONNECTIONS_SEGMENT: &str = "/@connections/";

/// Rewrite `uri` so its path starts at the modeled `/@connections/{ConnectionId}`
/// binding, dropping the invoke-path prefix in front of it. `None` when the path
/// carries no `@connections` segment, or already begins with it (nothing to strip).
///
/// WHY a prefix is there at all: a caller of this API addresses it through the owning
/// API's own invoke endpoint, and "the stage name is typically appended to the API
/// endpoint" (the `Api$ApiEndpoint` doc in the API Gateway V2 model), so AWS serves
/// these operations at `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`.
/// An SDK client is configured with that whole URL — stage path included — as its
/// endpoint and appends the modeled URI to it, so the path that arrives here carries
/// the stage (and, through a custom domain's base-path mapping, whatever segments that
/// mapping adds) in front of the modeled binding.
///
/// Everything before the segment is dropped rather than parsed: the modeled operations
/// take no api or stage member, the `ConnectionId` label is the only path value any of
/// them binds, and a `ConnectionId` is one path segment (a `/` inside it is
/// percent-encoded), so the `@connections` segment occurs exactly once.
fn modeled_uri(uri: &str) -> Option<String> {
    // Parsed with the `http::Uri` parser the crate already depends on, rather than
    // split by hand: the authority (userinfo, port), an absent path and a non-origin
    // target are its rules to know, not this seam's.
    let addressed: http::Uri = uri.parse().ok()?;
    let path_and_query = addressed.path_and_query()?;
    let path = path_and_query.path();
    let segment_at = path.find(CONNECTIONS_SEGMENT)?;
    if segment_at == 0 {
        // Already the modeled path: the generated routing table resolves it.
        return None;
    }
    let query = path_and_query
        .query()
        .map_or_else(String::new, |query| format!("?{query}"));
    let modeled = format!("{}{query}", &path[segment_at..]);

    let mut rewritten = http::Uri::builder();
    // Scheme and authority survive the rewrite; an origin-form target carries neither,
    // and the builder then takes the path alone.
    if let Some(scheme) = addressed.scheme() {
        rewritten = rewritten.scheme(scheme.clone());
    }
    if let Some(authority) = addressed.authority() {
        rewritten = rewritten.authority(authority.clone());
    }
    rewritten
        .path_and_query(modeled)
        .build()
        .ok()
        .map(|uri| uri.to_string())
}

/// The same request addressed at the modeled `/@connections/{ConnectionId}` path,
/// carrying `body`: method and headers verbatim, with the invoke-path prefix dropped
/// from the URI. `None` when there is no prefix to drop.
fn at_modeled_path_with(
    req: &aws_smithy_runtime_api::http::Request,
    body: aws_smithy_types::body::SdkBody,
) -> Option<aws_smithy_runtime_api::http::Request> {
    let uri = modeled_uri(req.uri())?;
    let mut rewritten = aws_smithy_runtime_api::http::Request::new(body);
    rewritten.set_uri(uri.as_str()).ok()?;
    rewritten.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        // `append`, not `insert`: `iter` yields one entry per value, so a repeated
        // header keeps every value it arrived with.
        rewritten
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Some(rewritten)
}

/// The request addressed at its modeled path for a ROUTING decision only, with an empty
/// body: resolving a restJson1 routing key reads the method, the path and which headers
/// are present, never the body, so the payload is not copied to decide where a request
/// goes. `None` when there is no invoke-path prefix to drop.
fn routing_view_at_modeled_path(
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<aws_smithy_runtime_api::http::Request> {
    at_modeled_path_with(req, aws_smithy_types::body::SdkBody::empty())
}

/// The request addressed at its modeled path for the SERVE: the body copied verbatim, so
/// the modeled operation deserializes exactly the payload that arrived. `None` when there
/// is no prefix to drop, or when the body is not buffered (it cannot be copied without
/// consuming it).
fn request_at_modeled_path(
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<aws_smithy_runtime_api::http::Request> {
    let body = req.body().bytes()?.to_vec();
    at_modeled_path_with(req, aws_smithy_types::body::SdkBody::from(body))
}

/// The invoke-path PREFIX this request carries in front of the modeled
/// `/@connections/{ConnectionId}` binding — the deployed stage, or the base path a custom
/// domain maps — or `None` when the path carries no `@connections` segment at all.
///
/// The empty prefix is the modeled path itself. The prefix is what names the STAGE a request
/// was addressed through, which is what decides whether the endpoint serves anything at all
/// ([`connections_endpoint`]).
fn invoke_path_prefix(uri: &str) -> Option<String> {
    let addressed: http::Uri = uri.parse().ok()?;
    let path = addressed.path_and_query()?.path();
    let segment_at = path.find(CONNECTIONS_SEGMENT)?;
    Some(path[..segment_at].to_string())
}

/// The endpoint HOST this request named: the authority of an absolute request URI, or the
/// `Host` header an origin-form request carries it in.
fn endpoint_host(req: &aws_smithy_runtime_api::http::Request) -> Option<String> {
    if let Some(authority) = req
        .uri()
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(ToString::to_string))
    {
        return Some(authority);
    }
    req.headers()
        .get(http::header::HOST.as_str())
        .map(ToString::to_string)
}

/// Whether this service serves the request these `parts` came from.
///
/// This service signs as `execute-api` — a signing name it SHARES with the Amazon
/// Connect Participant Service
/// (`services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda`,
/// `aws.auth#sigv4` name `execute-api`), whose participant endpoint is itself served
/// through API Gateway. The router resolves a request's routing key from the SigV4
/// credential scope FIRST, so both services see the same key: claiming on the signing
/// name alone makes each claim its peer's traffic, the router finds two claimants and
/// answers `500 AmbiguousService` — a code no AWS model declares — which made BOTH
/// services unreachable. Every `@connections` call answered a retryable server fault
/// instead of its modelled 2xx or 4xx.
///
/// Their modelled URI path spaces are disjoint: this service's three operations are all
/// bound under `/@connections/…` (`ROUTE_PATH_SEGMENTS` is `["@connections"]`), the
/// participant service's under `/participant/…`. So the claim adds the framework's
/// path-segment conjunct over this service's own generated, model-derived segments.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule and is CALLED here rather than re-implemented. It takes a whole
/// `http::Request` while [`Service::can_handle`] is handed only the request's parts, so
/// the two things it reads — the URI and the headers — are re-presented to it as a
/// bodiless request. It reads neither the body nor the method, so nothing it decides on
/// is missing.
///
/// The path it judges is the one the request is addressed at MODELLED ([`modeled_uri`]):
/// a caller reaches these operations through the owning API's invoke URL, so the arriving
/// path carries that URL's prefix in front of the modelled binding and its FIRST segment
/// is the stage, not `@connections`.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let addressed = parts.uri.to_string();
    let uri = modeled_uri(&addressed).unwrap_or(addressed);
    let Ok(mut request) = http::Request::builder().uri(uri).body(bytes::Bytes::new()) else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one shared engine, held here because the ENDPOINT a request was addressed
    /// through is judged in [`Service::override_operation`] — before any operation runs —
    /// and that judgement reads the shared engine ([`connections_endpoint`]).
    engine: Arc<Eng>,
    /// This service's `restJson1`, with the error body it is recorded answering; see
    /// [`rest_json_error_body`]. Built once, here, because [`Service::override_protocol`]
    /// hands back a borrow of it.
    protocol: rest_json_error_body::ConnectionsRestJson,
    /// Refuses a request whose endpoint serves no deployed stage; see [`UndeployedEndpoint`].
    undeployed_endpoint: UndeployedEndpoint,
    /// Answers an over-limit `PostToConnection`; see [`OversizedPayload`].
    oversized_payload: OversizedPayload,
    /// Serves a request addressed through the owning API's invoke path; see
    /// [`InvokePathPrefixed`].
    invoke_path_prefixed: InvokePathPrefixed,
}

/// Refuses a `@connections` request addressed through an endpoint that serves NO deployed
/// stage, with the modelled `ForbiddenException` (403) and the sentence API Gateway words
/// that refusal in — see [`connections_endpoint`] for the rule and for who decides it.
///
/// WHY this is a seam and not a line in a handler: the stage is part of the ENDPOINT, not of
/// the request the model describes (no operation of this service takes an api or a stage
/// member), so the decision belongs before dispatch — ahead of the payload gate, the
/// `ConnectionId`'s own form and the connection store alike, which is the order AWS resolves
/// them in. [`Service::override_operation`] is the seam for a request the model's routing key
/// cannot answer correctly, and this is one: the model routes it to a handler that would
/// report on a connection the caller may not reach through this endpoint.
struct UndeployedEndpoint;

impl ErasedOperation for UndeployedEndpoint {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move { connections_endpoint::forbidden() })
    }
}

/// Serves a request whose path carries the owning API's invoke-path prefix in front of
/// the modeled `/@connections/{ConnectionId}` binding — `…/{stage}/@connections/{id}`,
/// which is the URL AWS documents for these operations and the one an SDK client built
/// from a stage endpoint sends.
///
/// WHY this is a seam and not a line in a handler: the prefix defeats the model's own
/// routing key. The generated routing table matches a request's method and path against
/// each operation's modeled URI pattern, so a path with an extra leading segment names
/// NO operation and the request never reaches a handler at all — it is answered with the
/// protocol's routing failure. [`Service::override_operation`] is exactly the seam for
/// "the model's routing key cannot name this request correctly": this routes it by
/// resolving the SAME model-derived routing table against the request addressed at its
/// modeled path, so the prefix changes nothing about which operation runs or what it
/// answers.
struct InvokePathPrefixed;

impl ErasedOperation for InvokePathPrefixed {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // `override_operation` only routes here for a request this rewrite already
            // succeeded on, so each `else` below is unreachable in the served path and
            // falls back to the protocol's own routing failure rather than guessing an
            // operation.
            let Some(rewritten) = request_at_modeled_path(req) else {
                return svc.definition().routing_error_response(req);
            };
            let Ok(routing_key) = svc.definition().protocol().routing_key(&rewritten) else {
                return svc.definition().routing_error_response(&rewritten);
            };
            let Some(operation) = svc.definition().routing_table().get(routing_key) else {
                return svc.definition().routing_error_response(&rewritten);
            };
            // The modeled operation deserializes the rewritten request, so its
            // `ConnectionId` label is read off the modeled path binding.
            operation.run(svc, &rewritten, ctx).await
        })
    }
}

/// Answers a `PostToConnection` whose `Data` payload is larger than the model allows with
/// a BARE HTTP 413: the status alone, no body and no `x-amzn-errortype`.
///
/// WHY this is a seam and not a line in the handler: `Data` carries
/// `@length(min: 0, max: 131072)`, and the framework validates every modelled constraint
/// BEFORE a handler runs and renders a violation as the protocol's generic refusal (HTTP
/// 400), so the 413 is unreachable from inside `handle` — the request is refused before it
/// gets there. The model's routing key cannot distinguish an over-limit payload either,
/// which is exactly what [`Service::override_operation`] exists for.
///
/// WHY the answer is bare rather than the modelled `PayloadTooLargeException` envelope: the
/// ceiling is enforced by the `@connections` endpoint IN FRONT of the modelled surface, and
/// what that endpoint answers is recorded as the status with no error shape named at all —
/// so a restJson1 client resolves no modelled error from it and reports the status itself as
/// the error code. It is answered BEFORE the `ConnectionId` is read for the same reason:
/// the request is refused on its size by the endpoint, whatever the id says, which is why
/// `override_operation` routes it here ahead of everything else — behind only the endpoint's
/// own stage authority ([`UndeployedEndpoint`]), since an endpoint that serves nothing
/// refuses the request before its payload is read at all.
struct OversizedPayload;

impl ErasedOperation for OversizedPayload {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let _ = (svc, req);
            oversized_payload_response()
        })
    }
}

/// The bare HTTP 413 [`OversizedPayload`] answers: the status, an empty body, and nothing
/// that names an error shape.
///
/// The status is this module's own constant, so the conversion cannot fail.
fn oversized_payload_response() -> aws_smithy_runtime_api::http::Response {
    let status = aws_smithy_runtime_api::http::StatusCode::try_from(PAYLOAD_TOO_LARGE_STATUS)
        .expect("the payload ceiling's status is a valid constant");
    let mut response = aws_smithy_runtime_api::http::Response::new(
        status,
        aws_smithy_types::body::SdkBody::empty(),
    );
    response.headers_mut().insert("content-length", "0");
    response
}

/// The status the `@connections` endpoint refuses an over-limit `Data` payload with: 413,
/// the status the model also declares for `PayloadTooLargeException`
/// (`smithy_query(query="shape", shape="PayloadTooLargeException")`, `@httpError(413)`).
const PAYLOAD_TOO_LARGE_STATUS: u16 = 413;

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            protocol: rest_json_error_body::ConnectionsRestJson::new(),
            undeployed_endpoint: UndeployedEndpoint,
            oversized_payload: OversizedPayload,
            invoke_path_prefixed: InvokePathPrefixed,
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
        // The signing name `execute-api` is SHARED with the Amazon Connect Participant
        // Service, so the claim is the signing name AND this service's own modelled
        // path space. See [`can_handle_parts`] for why, and for who owns the rule.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // Three cases need it.
        //
        // The first is the ENDPOINT the request was addressed through: these operations are
        // served on the owning api's own invoke endpoint, so the path carries that api's
        // stage, and an addressing that names no DEPLOYED stage is refused before anything
        // else is judged — see [`UndeployedEndpoint`] and [`connections_endpoint`].
        if let Some(prefix) = invoke_path_prefix(req.uri()) {
            if connections_endpoint::addresses_nothing(
                &*self.engine,
                endpoint_host(req).as_deref(),
                &prefix,
            ) {
                return Some(&self.undeployed_endpoint as &dyn ErasedOperation);
            }
        }

        // The second is the invoke-path prefix every caller of this API sends in front
        // of the modeled binding (`…/{stage}/@connections/{id}`), which the model's
        // routing key cannot match. Which operation the request names is read off the
        // request addressed at its modeled path; the serve rewrites it again, with the
        // body, in [`InvokePathPrefixed`].
        let routing_view = routing_view_at_modeled_path(req);
        let prefixed = routing_view.is_some();
        let addressed = routing_view.as_ref().unwrap_or(req);

        // The third is a `PostToConnection` whose `Data` payload is past the modeled
        // `@length` maximum. See [`OversizedPayload`] for what the `@connections` endpoint
        // answers such a request and why the handler cannot. Both the operation name and
        // the bound are read from the model, not spelled out.
        let routing_key = self.definition().protocol().routing_key(addressed).ok()?;
        if routing_key
            == <crate::ops::post_to_connection::PostToConnection<Eng> as smithy_server_runtime::Operation>::NAME
        {
            // `Data` is the `@httpPayload` member, so the body length IS the payload
            // length. `aws-chunked` framing is already stripped by this point, and this
            // dispatch path always hands over a fully buffered body (the router builds
            // the runtime-API request from the buffered bytes before any lookup), so
            // `bytes()` is `Some`. A body that is NOT buffered cannot be sized without
            // consuming it, so it is left to the framework's own constraint boundary
            // rather than guessed at — it does not decide the routing below.
            let over_limit = req
                .body()
                .bytes()
                .is_some_and(|body| body.len() > crate::ops::post_to_connection::MAX_DATA_BYTES);
            if over_limit {
                // The payload is judged before the path AND before the `ConnectionId`: an
                // over-limit post answers the same 413 whether or not it carried an
                // invoke-path prefix and whatever its id says.
                return Some(&self.oversized_payload as &dyn ErasedOperation);
            }
        }
        // Every other operation under an invoke-path prefix is served by the rewriting
        // seam; an unprefixed one needs no override at all.
        prefixed.then_some(&self.invoke_path_prefixed as &dyn ErasedOperation)
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // The default IS this service's answer; what it renders is handed back with the
        // ERROR BODY this service is recorded answering, because the shared renderer writes
        // the AWS JSON family's shape discriminator into every body it renders and a
        // `restJson1` body carries the error shape's own modelled members alone. See
        // [`rest_json_error_body`] for the rule and for the one class no seam can reach.
        Some(rest_json_error_body::with_served_error_body(
            self.entry.routing_error_response(req),
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own `restJson1`, with the ERROR BODY this service is recorded
        // answering: a failure's body carries the error shape's own modelled members and
        // nothing else, so a memberless `GoneException` or `ForbiddenException` is the empty
        // JSON document and the shape travels in `x-amzn-errortype` alone. The shared
        // framework renderer writes the AWS JSON family's `__type` discriminator into every
        // body it renders, and it is not this crate's to change; see
        // [`rest_json_error_body`] for the rule, why it matters and where else it is applied.
        Some(&self.protocol)
    }
}
