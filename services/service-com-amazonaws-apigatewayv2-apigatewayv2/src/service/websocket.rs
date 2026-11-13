// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` WEBSOCKET data plane: how an RFC 6455 OPENING HANDSHAKE addressed to
//! a stage of a WebSocket api is recognised, and what it is answered with.
//!
//! A WebSocket api is reached over a SOCKET, and the socket it is served on is this api's
//! own: [`transport`] opens one per WebSocket api through the core
//! (`engine.listeners()`), completes the handshake on it, and carries the frames that
//! follow, and the address it landed on is what that api announces as its `ApiEndpoint`
//! ("the stage name … appended to this URI to form a complete path to a deployed API
//! stage", `Api$ApiEndpoint`). A client that dials the announced address therefore reaches
//! the api's data plane in full — `101 Switching Protocols`, then frames dispatched to the
//! route the api's `RouteSelectionExpression` selects ([`dispatch`]).
//!
//! THIS module serves the other address the same handshake can arrive at: the emulator's
//! own SERVICE endpoint, where a client that rewrote the announced URI onto the endpoint it
//! is already talking to ends up. No service claimed such a request before, so the front
//! door could resolve no target service for it and answered `404` `UnknownService` — an
//! "this API does not exist" shape for an endpoint this very service told the caller to
//! use, and one no WebSocket client can act on: it cannot tell a missing data plane from a
//! stage that was never deployed. So this module is the second half of
//! [`crate::service::EmulatedService`]: the claim ([`claims_parts`], read by
//! `Service::can_handle`) and the serving ([`serve`], reached from
//! `Service::override_routing_error` — a handshake names no modelled operation, and it
//! carries no credential whose verification could change the answer). It is a pure READ of
//! this service's own records: a handshake changes nothing here, and every status it answers
//! is computed from the api's and the stage's stored state.
//!
//! # What it resolves, in order
//!
//! 1. the request is an RFC 6455 opening handshake ([`Handshake::is_opening`]) and does not
//!    address the `@connections` space, which belongs to ApiGatewayManagementApi;
//! 2. it addresses an api THIS service holds whose `ProtocolType` is `WEBSOCKET`
//!    ([`resolve`]) — by the api id in an `execute-api` host, by the api id a rewritten
//!    endpoint carries in the path (`/_aws/execute-api/{api-id}/{stage}`, the marker the
//!    HTTP data plane reads too), or, for a handshake whose host names no api (the
//!    emulator's own endpoint, or a custom domain fronting the api), by the stage segment of
//!    its path;
//! 2b. that api's `DisableExecuteApiEndpoint` has not turned this endpoint off. While it
//!    has, the endpoint upgrades nothing and every handshake made at it is refused `403`
//!    `{"message":"Forbidden"}` before a stage of the api is looked for
//!    ([`default_endpoint_refusal`]);
//! 3. that api holds a stage the path names, and that stage has a deployment released to
//!    it; a step that resolves nothing is `403` `{"message":"Forbidden"}`, which is what
//!    API Gateway answers for an api id it does not serve and for a stage that has no
//!    deployment (the `execute-api` status taxonomy the REST data plane already states,
//!    `service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`);
//! 4. the AUTHORIZATION of the route that serves the connection request is applied before
//!    anything that route fronts ([`connect_authorization_refusal`]): when that route's
//!    `AuthorizationType` names an `Authorizer` of the api, every one of that authorizer's
//!    `IdentitySource` expressions must be "present in the request, not null, and
//!    non-empty" — "only when this is true does the authorizer invoke the authorizer
//!    Lambda function. Otherwise, it returns a 401 Unauthorized response without calling
//!    the Lambda function" (`Authorizer$IdentitySource`) — so a handshake missing one is
//!    `401` `{"message":"Unauthorized"}`, and a route naming an authorizer the api does
//!    not hold is a configuration this endpoint cannot serve, `500`
//!    `{"message":"Internal server error"}`.
//!
//! # What it does not do, and what it answers instead
//!
//! 5. the CONNECTION REQUEST itself is served by the `$connect` route's integration, and the
//!    handshake completes only on a successful reply ([`dispatch::connection_request`]), so
//!    a reply the api's own records alone cannot produce is `500`
//!    `{"message":"Internal server error"}` — which is what a `$connect` route with no
//!    integration at all, or one bound to a `MOCK`, is answered on the api's own socket too.
//!
//! A handshake that every step above let through would be answered `101 Switching
//! Protocols` — and is, on the api's own socket ([`transport`]). On THIS endpoint it is
//! answered `501` `NotImplemented` with the sentence [`NO_WEBSOCKET_TRANSPORT_MESSAGE`],
//! which names the address the api IS reached at.
//!
//! The emulator's front door serves one BUFFERED request per call — `emulator_server`'s
//! production listener hands each request to the router and writes the one response it
//! answers with, and the only seams a service is given (`Service::override_operation`,
//! `Service::override_routing_error`) take an already-buffered request and return a
//! buffered response. Nothing in that path can hand the connection's byte stream to a
//! service, so a `101` answered HERE would promise a session nothing can carry: the client
//! would see the upgrade succeed and then read EOF on its first frame, and a connection id
//! minted for it would be addressable while no socket existed. That is a fabricated
//! success, so it is refused instead — the same answer, for the same reason, that the
//! Transcribe Streaming WebSocket transport gives
//! (`service-com-amazonaws-transcribestreaming-transcribe/src/ops/types/event_stream.rs`,
//! which refuses the upgrade with its operation's own modelled error). `501` is this
//! emulator's "not implemented" channel (`crate::scaffolded`), and it is not a retryable
//! 5xx: a client learns to dial the announced socket rather than retrying for ever.
//!
//! Nor is an authorizer's own ALLOW/DENY decision obtained HERE, and neither is the
//! connection request's own backend called: both are an invocation of a Lambda function,
//! which is awaited, while `Service::override_routing_error` is a SYNCHRONOUS seam handed a
//! buffered request. Neither changes what this endpoint answers — the handshake made here is
//! refused whatever they would decide — so what step 4 above applies is the part of the
//! authorization that is the SERVICE's own and is computed from stored records: the
//! identity-source precondition the authorizer is only invoked past, and a route whose
//! authorizer the api does not hold. On the api's OWN socket, where the handshake is
//! completed, both are performed: the authorizer's function decides the connection
//! ([`connect_authorizer`]) and the `$connect` route's integration serves the connection
//! request ([`integration`]).

// What the STAGE logs about each call a connection makes: the access-log record its own
// `AccessLogSettings` ask for, rendered from the `$context` of that call and delivered to the
// destination the stage names.
pub mod access_logs;
pub mod connect_authorizer;
pub mod dispatch;
// What the api's own socket answers a request on it that is NOT an opening handshake: the
// address that socket landed on is the api's whole `ApiEndpoint`, and the `@connections`
// management API is published at that same endpoint, so an ordinary HTTP request made there
// is answered by this emulator's own front door rather than dropped.
pub mod endpoint_http;
pub mod event;
pub mod integration;
// The DATA MAPPING `Integration$RequestParameters` applies to a WebSocket call, which the
// event above states after it: the member is a mapping from a request parameter of the call
// (or a quoted static value) onto an integration request parameter, and this module is the
// one reader of it.
pub mod parameters;
pub mod transport;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::http;

// The readings this data plane SHARES with the HTTP one ([`super::invoke`]): the invoke
// endpoint's host and path, the identity-source lookup, and the `{"message": …}` envelope
// every answer is rendered in. One owner, so the two planes cannot disagree about them.
use super::data_plane::{
    addresses_connections, api_id_of_host, authority_of, first_segment, forbidden, header_of,
    json_response, path_of, presents_identity_source, query_parameters, UNAUTHORIZED_MESSAGE,
    UNAUTHORIZED_STATUS, UNPLACEABLE_STATUS,
};
use crate::state::{Api, ComAmazonawsApigatewayv2Apigatewayv2State, Stage};

/// The `RouteKey` of the route a WebSocket api serves the CONNECTION REQUEST with — the
/// opening handshake this module answers.
///
/// A reserved route key of the WebSocket protocol rather than a value the model
/// enumerates (`RouteKey` is an unconstrained string): AWS documents `$connect` as the
/// route "API Gateway invokes when a client connects to your API", which is why the
/// authorization configured on it is what decides a handshake. The other two reserved keys
/// (`$disconnect`, `$default`) are reached over an ESTABLISHED socket, which this endpoint
/// never establishes — the api's own socket does ([`transport`]).
const CONNECT_ROUTE_KEY: &str = "$connect";

/// The `Connection` field token that asks for a protocol upgrade (RFC 6455 §4.1).
const UPGRADE_TOKEN: &str = "upgrade";

/// The `Upgrade` field value that names the WebSocket protocol (RFC 6455 §4.1).
const WEBSOCKET_PROTOCOL: &str = "websocket";

/// The header carrying the client's nonce, "a base64-encoded value that, when decoded, is
/// 16 bytes in length" (RFC 6455 §4.1). Its presence is what distinguishes an opening
/// handshake from any other upgrade request.
const SEC_WEBSOCKET_KEY: &str = "sec-websocket-key";

/// The error code the capability refusal carries, in the `x-amzn-errortype` header
/// [`super::data_plane::json_response`] sets. The value is the one [`crate::scaffolded`]
/// renders for an unimplemented capability.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The reason a deployed WebSocket stage cannot complete the handshake. Stated to a
/// developer in one sentence, because the distinction they need first is "this emulator
/// has no WebSocket transport" rather than "the stage is misconfigured"; see the module
/// docs for why no `101` is answered.
pub const NO_WEBSOCKET_TRANSPORT_MESSAGE: &str =
    "NO_WEBSOCKET_TRANSPORT: this emulator's front door serves one buffered request per \
     call and cannot switch a connection to the WebSocket protocol, so an RFC 6455 opening \
     handshake made HERE is refused; the api's own socket, which completes the handshake \
     and carries its frames, is the one its ApiEndpoint announces";

/// [`NO_WEBSOCKET_TRANSPORT_MESSAGE`] naming the address this api IS reached at, so a
/// developer who made the handshake against the service endpoint reads where to make it
/// instead.
fn wrong_endpoint_message(api_endpoint: &str) -> String {
    format!("{NO_WEBSOCKET_TRANSPORT_MESSAGE}: {api_endpoint}")
}

/// The status the capability refusal carries: [`crate::scaffolded`]'s 501, which an SDK
/// retry classifier does not treat as retryable.
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The sentence [`UNPLACEABLE_STATUS`] carries for a WEBSOCKET api.
const INTERNAL_MESSAGE: &str = "Internal server error";

/// The parts of a request this data plane reads: the method, the host and path it is
/// addressed at, and the three handshake fields of RFC 6455 §4.1.
///
/// One reader for BOTH request shapes the seams are handed — `http::request::Parts` in
/// `Service::can_handle` and the runtime-API request everywhere else — so the claim and the
/// serve cannot disagree about what a request addresses.
#[derive(Debug, Clone, Copy)]
pub struct Handshake<'r> {
    method: &'r str,
    host: &'r str,
    path: &'r str,
    connection: Option<&'r str>,
    upgrade: Option<&'r str>,
    key: Option<&'r str>,
}

impl<'r> Handshake<'r> {
    /// The reading of the request parts `Service::can_handle` is handed.
    pub fn of_parts(parts: &'r http::request::Parts) -> Self {
        Self {
            method: parts.method.as_str(),
            host: header_of(&parts.headers, "host")
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str))
                .unwrap_or_default(),
            path: parts.uri.path(),
            connection: header_of(&parts.headers, "connection"),
            upgrade: header_of(&parts.headers, "upgrade"),
            key: header_of(&parts.headers, SEC_WEBSOCKET_KEY),
        }
    }

    /// The reading of the runtime-API request, which carries its URI as a string.
    pub fn of_request(req: &'r RtRequest) -> Self {
        Self {
            method: req.method(),
            host: req
                .headers()
                .get("host")
                .or_else(|| authority_of(req.uri()))
                .unwrap_or_default(),
            path: path_of(req.uri()),
            connection: req.headers().get("connection"),
            upgrade: req.headers().get("upgrade"),
            key: req.headers().get(SEC_WEBSOCKET_KEY),
        }
    }

    /// The reading of a handshake read off a SOCKET, whose request head this service parsed
    /// itself ([`transport`]) rather than received as a built request.
    ///
    /// The same six fields, so one rule ([`Self::is_opening`]) decides what an opening
    /// handshake is wherever the bytes came from.
    pub(crate) fn of_fields(
        method: &'r str,
        host: &'r str,
        path: &'r str,
        connection: Option<&'r str>,
        upgrade: Option<&'r str>,
        key: Option<&'r str>,
    ) -> Self {
        Self {
            method,
            host,
            path,
            connection,
            upgrade,
            key,
        }
    }

    /// Whether this is an RFC 6455 opening handshake: a `GET` whose `Connection` field
    /// includes the `Upgrade` token, whose `Upgrade` field names `websocket`, and which
    /// carries a non-empty `Sec-WebSocket-Key`.
    ///
    /// Each field is a comma-separated token list compared case-insensitively, as RFC 6455
    /// §4.1 requires ("a |Connection| header field that includes the token "Upgrade"",
    /// "an |Upgrade| header field containing the value "websocket", treated as an
    /// ASCII case-insensitive value"), so `keep-alive, Upgrade` and `WebSocket` are the
    /// same handshake a client library sends.
    pub fn is_opening(&self) -> bool {
        self.method.eq_ignore_ascii_case("GET")
            && self
                .connection
                .is_some_and(|field| has_token(field, UPGRADE_TOKEN))
            && self
                .upgrade
                .is_some_and(|field| has_token(field, WEBSOCKET_PROTOCOL))
            && self.key.is_some_and(|key| !key.trim().is_empty())
    }

    /// Whether the path addresses the `@connections` space of ApiGatewayManagementApi, in
    /// either of the two forms that service's callers use: with the stage prefix its
    /// endpoint carries (`/{stage}/@connections/{id}`) and without it.
    fn addresses_connections(&self) -> bool {
        addresses_connections(self.path)
    }
}

/// What a handshake addresses, as this service's records resolve it: the api, the stage
/// name the path carries, and that api's stage of that name when it holds one.
#[derive(Debug)]
struct Resolved<'s> {
    api: &'s Api,
    stage_name: String,
    stage: Option<&'s Stage>,
}

/// The WEBSOCKET api and stage `handshake` addresses, or `None` when it addresses none
/// this service holds.
///
/// Two addressings resolve, because a client reaches this emulator at a different host
/// than it reaches AWS at:
///
/// * the host's first label names the api, which is the addressing `Api$ApiEndpoint`
///   documents. A host that IS an `execute-api` endpoint but whose api id this service
///   does not hold resolves to nothing at all — that host belongs to another api (an API
///   Gateway REST api is reached on the same host shape and its own service claims it), so
///   reading its path as one of THIS service's stages would make two services claim one
///   request;
/// * a host that names no api at all — the emulator's own endpoint, which a WebSocket
///   client points at because `{api-id}.execute-api.{region}.amazonaws.com` resolves
///   nowhere on the machine it runs on, or a custom domain fronting the api — is resolved
///   by the stage segment of its path, the only part of such a handshake that names a
///   deployed stage. This is the same host-independent claim ApiGatewayManagementApi makes
///   over its own `@connections` path space. When two WebSocket apis hold a stage of that
///   name the request names no ONE api, so nothing is resolved rather than one of them
///   guessed at.
///
/// An api whose `ProtocolType` is not `WEBSOCKET` is never resolved: an upgrade request to
/// an HTTP api's stage is an ordinary HTTP request to that api's routes, which is that
/// api's data plane and not this handshake.
fn resolve<'s>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    handshake: &Handshake<'_>,
) -> Option<Resolved<'s>> {
    if handshake.addresses_connections() {
        return None;
    }
    // LOCAL-ENDPOINT style first, because its marker is unambiguous on any host: a client
    // that resolves `{api-id}.execute-api.{region}.amazonaws.com` nowhere rewrites the
    // endpoint it was given into the emulator's own, carrying the api id in the path ahead
    // of the stage (`/_aws/execute-api/{api-id}/{stage}`). It is the SAME marker the HTTP
    // data plane reads, through that plane's one reading of it
    // ([`super::invoke::local_endpoint_path`]).
    if let Some((api_id, stage_path)) = super::invoke::local_endpoint_path(handshake.path) {
        let api = store.apis.get(api_id).filter(|api| is_websocket(api))?;
        // The path already names the api, so a path that names no stage of it is still
        // this api's to answer: an endpoint that serves nothing (`403`).
        let stage_name = first_segment(stage_path).unwrap_or_default();
        return Some(Resolved {
            api,
            stage_name: stage_name.to_string(),
            stage: api.stages.get(stage_name),
        });
    }
    let stage_name = first_segment(handshake.path);
    match api_id_of_host(handshake.host) {
        Some(api_id) => {
            let api = store.apis.get(api_id).filter(|api| is_websocket(api))?;
            // The host already names the api, so a path that names no stage of it is
            // still this api's to answer: an endpoint that serves nothing (`403`), not a
            // request this service cannot place.
            let stage_name = stage_name.unwrap_or_default();
            Some(Resolved {
                api,
                stage_name: stage_name.to_string(),
                stage: api.stages.get(stage_name),
            })
        }
        None => {
            let stage_name = stage_name?;
            let mut carrying = store
                .apis
                .values()
                .filter(|api| is_websocket(api) && api.stages.contains_key(stage_name));
            let api = carrying.next()?;
            if carrying.next().is_some() {
                return None;
            }
            Some(Resolved {
                api,
                stage_name: stage_name.to_string(),
                stage: api.stages.get(stage_name),
            })
        }
    }
}

/// Whether this api speaks the WebSocket protocol: `ProtocolType` is an enum of exactly
/// `HTTP | WEBSOCKET` (`smithy_query(query="shape", shape="ProtocolType")`).
fn is_websocket(api: &Api) -> bool {
    api.protocol_type == crate::state::WEBSOCKET_PROTOCOL_TYPE
}

/// The refusal a handshake made at an api's DEFAULT endpoint owes when that endpoint is off,
/// or `None` for an api that still serves on it
/// ([`super::data_plane::serves_default_endpoint`]).
///
/// `403` `{"message":"Forbidden"}`, the same answer the endpoint gives for an api or a stage
/// it does not serve: an upgrade the endpoint does not accept is REFUSED rather than upgraded,
/// and is refused before the stage, the authorizer or the connection request's own backend is
/// read, so nothing of the api is reached through an endpoint that serves none of it.
fn default_endpoint_refusal(api: &Api) -> Option<RtResponse> {
    if super::data_plane::serves_default_endpoint(api) {
        return None;
    }
    tracing::debug!(
        api_id = %api.api_id,
        "this api's default execute-api endpoint is disabled, so it upgrades nothing"
    );
    Some(forbidden())
}

/// Whether THIS service serves the request these parts came from as a WebSocket opening
/// handshake — the data-plane half of `Service::can_handle`.
///
/// Narrowed to a handshake against an api this service HOLDS, read from the one shared
/// store, so nothing is claimed that this service cannot answer about, and so the api ids
/// it minted are the only ones it claims a host for.
pub fn claims_parts<Eng: Send + Sync + 'static>(engine: &Eng, parts: &http::request::Parts) -> bool {
    claims(engine, &Handshake::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the routing-error seam is handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Handshake::of_request(req))
}

/// Whether this data plane serves `handshake`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, handshake: &Handshake<'_>) -> bool {
    if !handshake.is_opening() {
        return false;
    }
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), handshake).is_some())
}

/// Serve one RFC 6455 opening handshake addressed to a WebSocket api's stage, from this
/// service's own records ALONE — the answer a seam carrying no in-process transport can give
/// it.
///
/// Free-standing and reached with the ONE shared engine, the way the REST data plane's
/// `serve` is, so the claim above and this answer read the same store.
///
/// What it cannot do is PERFORM the connection request's integration or obtain the
/// authorizer's own decision, both of which are an awaited invocation: a handshake this
/// reading lets through is answered by [`serve_with_transport`] when the router named this
/// request an operation, and here only when it did not (a request the front door would
/// refuse as cross-site, which is the one case left to the routing-error seam).
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let handshake = Handshake::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    let store = state.lock();
    // Unreachable through the claim, which resolves the same api from the same store; the
    // api can still have been deleted between the claim and now, which is an api this
    // endpoint no longer serves.
    let Some(resolved) = resolve(&store, &handshake) else {
        return forbidden();
    };
    // The DEFAULT endpoint the handshake was made at serves nothing while the api turned it
    // off, so the upgrade is refused before a stage of it is even looked for.
    if let Some(refusal) = default_endpoint_refusal(resolved.api) {
        return refusal;
    }
    // A stage the api does not hold, and a stage with no deployment released to it, are
    // both an endpoint that serves nothing: `403` `{"message":"Forbidden"}`.
    let Some(stage) = resolved.stage else {
        tracing::debug!(
            api_id = %resolved.api.api_id,
            stage = %resolved.stage_name,
            "the WebSocket handshake names no stage of this api"
        );
        return forbidden();
    };
    // What the stage serves is the configuration its deployment FROZE, so the connection
    // request is answered by the `$connect` route of that deployment and not by the api's
    // current routes.
    let served = stage
        .deployment_id
        .as_deref()
        .and_then(|deployment_id| resolved.api.deployments.get(deployment_id));
    let Some(deployment) = served else {
        tracing::debug!(
            api_id = %resolved.api.api_id,
            stage = %stage.stage_name,
            "the WebSocket handshake names a stage with no deployment released to it"
        );
        return forbidden();
    };
    // A deployed stage: the handshake is authorized by the route that serves the
    // connection request, and only then met by the answer this endpoint can give it. See
    // the module docs for why that answer is not a `101` on THIS endpoint.
    if let Some((status, message)) = connect_authorization_refusal(
        resolved.api,
        stage,
        &deployment.configuration,
        |name: &str| req.headers().get(name).map(str::to_string),
        req.uri(),
    ) {
        return json_response(status, message, None);
    }
    // And then by the CONNECTION REQUEST itself, which the `$connect` route's integration
    // serves: the handshake completes only on a successful reply, so a reply the api's own
    // configuration cannot produce is the `500` a gateway answers for it
    // ([`dispatch::connection_request`]) — observable here, because it is decided before any
    // socket exists.
    let admits = match dispatch::connection_request(&deployment.configuration) {
        Ok(None) => true,
        Ok(Some(reply)) => dispatch::is_success_status(reply.status_code.as_deref()),
        Err(_unserved) => false,
    };
    if !admits {
        tracing::info!(
            api_id = %resolved.api.api_id,
            stage = %stage.stage_name,
            "refusing a WebSocket opening handshake: the connection request's integration \
             did not reply successfully"
        );
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    }
    tracing::info!(
        api_id = %resolved.api.api_id,
        stage = %stage.stage_name,
        announced = %resolved.api.api_endpoint,
        "refusing a WebSocket opening handshake on the service endpoint: this api's socket \
         is the one its ApiEndpoint announces"
    );
    json_response(
        NOT_IMPLEMENTED_STATUS,
        &wrong_endpoint_message(&resolved.api.api_endpoint),
        Some(NOT_IMPLEMENTED_CODE),
    )
}

/// Serve one opening handshake addressed at this emulator's own SERVICE endpoint WITH the
/// in-process transport the router hands an operation — so every refusal that is decided
/// before the upgrade is the one AWS answers, and not a status this endpoint chose because
/// it could not ask.
///
/// Two steps of [`serve`] become the real ones here, and they are the two a buffered request
/// is enough for, because API Gateway decides both BEFORE it answers the handshake:
///
/// * the `$connect` route's AUTHORIZER decides the connection ([`connect_authorizer`]), so a
///   policy that denies is `403` `{"message":"Forbidden"}` rather than a status chosen for
///   lack of a decision;
/// * the CONNECTION REQUEST's own integration is performed ([`integration`]), so a
///   `$connect` route whose function refuses the client is answered with the status and the
///   body that function returned.
///
/// Only the ADMITTED handshake still cannot be served here: that one needs the `101` and the
/// socket behind it, which this seam cannot give (see the module docs), so it is refused
/// [`NOT_IMPLEMENTED_STATUS`] naming the address the api IS reached at.
pub async fn serve_with_transport<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> RtResponse {
    let handshake = Handshake::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    // Everything the two steps below need, read out of the store under ONE lock and owned
    // from then on: performing an integration is awaited, and no state guard may be held
    // across an await.
    let offered = {
        let mut store = state.lock();
        // One id per connection request, minted before the request is served because the
        // event that serves it carries the id — the connection AWS mints for a handshake it
        // then refuses is minted too.
        let connection_id = store.websocket_transport.mint_connection_id();
        let Some(resolved) = resolve(&store, &handshake) else {
            return forbidden();
        };
        // As in [`serve`]: an api that turned its default endpoint off upgrades nothing on it.
        if let Some(refusal) = default_endpoint_refusal(resolved.api) {
            return refusal;
        }
        let Some(stage) = resolved.stage else {
            tracing::debug!(
                api_id = %resolved.api.api_id,
                stage = %resolved.stage_name,
                "the WebSocket handshake names no stage of this api"
            );
            return forbidden();
        };
        let served = stage
            .deployment_id
            .as_deref()
            .and_then(|deployment_id| resolved.api.deployments.get(deployment_id));
        let Some(deployment) = served else {
            tracing::debug!(
                api_id = %resolved.api.api_id,
                stage = %stage.stage_name,
                "the WebSocket handshake names a stage with no deployment released to it"
            );
            return forbidden();
        };
        let authorization = connect_authorization(
            resolved.api,
            stage,
            &deployment.configuration,
            |name: &str| req.headers().get(name).map(str::to_string),
            req.uri(),
        );
        let authorizer = match authorization {
            Ok(authorizer) => authorizer,
            Err((status, message)) => return json_response(status, message, None),
        };
        OfferedHandshake {
            connection: event::Connection {
                api_id: resolved.api.api_id.clone(),
                stage_name: stage.stage_name.clone(),
                connection_id,
                region: resolved.api.region.clone(),
                connected_at: ::std::time::SystemTime::now(),
                headers: req
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                query_parameters: super::data_plane::joined_query_parameters(req.uri()),
                stage_variables: stage.stage_variables.clone(),
                authorizer_context: None,
                authorizer_principal: None,
            },
            authorizer,
            connect: integration::select_route_key(
                &deployment.configuration,
                dispatch::CONNECT_ROUTE_KEY,
            ),
            announced: resolved.api.api_endpoint.clone(),
        }
    };
    let mut connection = offered.connection;
    if let Some((authorizer, identity)) = offered.authorizer {
        match connect_authorizer::decided(ctx.callback(), &connection, &authorizer, &identity).await
        {
            Ok(decided) => {
                connection.authorizer_context = decided.context;
                connection.authorizer_principal = decided.principal_id;
            }
            Err((status, message)) => return json_response(status, message, None),
        }
    }
    if let Some(connect) = offered.connect {
        match integration::perform(
            ctx.callback(),
            &connection,
            &connect,
            &event::Call::Connect,
            // The id of this connection request, as the event the integration is handed
            // reports it: this endpoint completes no handshake, so there is no record of the
            // call to join it to, and the call still names itself.
            &crate::service::invoke::proxy_event::mint_request_id(),
        )
        .await
        {
            Ok(answered) if dispatch::is_success_status(answered.status_code.as_deref()) => {}
            Ok(answered) => {
                // The status the integration answered, and the body it answered with it.
                tracing::info!(
                    api_id = %connection.api_id,
                    stage = %connection.stage_name,
                    status = ?answered.status_code,
                    "refusing a WebSocket opening handshake: the connection request's \
                     integration did not reply successfully"
                );
                return refused_by_the_connection_request(&answered);
            }
            Err(unserved) => {
                tracing::info!(
                    api_id = %connection.api_id,
                    stage = %connection.stage_name,
                    ?unserved,
                    "refusing a WebSocket opening handshake: the connection request's \
                     integration produced no reply"
                );
                return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
            }
        }
    }
    tracing::info!(
        api_id = %connection.api_id,
        stage = %connection.stage_name,
        announced = %offered.announced,
        "refusing a WebSocket opening handshake on the service endpoint: this api's socket \
         is the one its ApiEndpoint announces"
    );
    json_response(
        NOT_IMPLEMENTED_STATUS,
        &wrong_endpoint_message(&offered.announced),
        Some(NOT_IMPLEMENTED_CODE),
    )
}

/// What a handshake at the service endpoint OFFERED, read out of the store under one lock;
/// see [`serve_with_transport`].
struct OfferedHandshake {
    connection: event::Connection,
    authorizer: Option<OwedDecision>,
    connect: Option<integration::Selected>,
    /// The address this api IS reached at, for the refusal an admitted handshake earns here.
    announced: String,
}

/// The answer a handshake the CONNECTION REQUEST refused earns: the status its integration
/// answered, and that integration's own body — "the response body is the $connect
/// integration's own body bytes" — relayed as the bytes it returned.
fn refused_by_the_connection_request(
    answered: &integration::Answered,
) -> RtResponse {
    let Some(status) = answered
        .status_code
        .as_deref()
        .and_then(|status| status.trim().parse::<u16>().ok())
    else {
        // A reply naming no status names no decision at all.
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    };
    let body = answered.payload.clone().unwrap_or_default();
    let length = body.len().to_string();
    let mut response = RtResponse::new(
        ::aws_smithy_runtime_api::http::StatusCode::try_from(status).unwrap_or(
            ::aws_smithy_runtime_api::http::StatusCode::try_from(UNPLACEABLE_STATUS)
                .expect("500 is a valid status code"),
        ),
        ::aws_smithy_types::body::SdkBody::from(body),
    );
    let _ = response.headers_mut().try_insert("content-length", length);
    response
}

/// The refusal the CONNECTION REQUEST's authorization owes this handshake, or `None` when
/// the route that serves it lets the handshake through to the next step.
///
/// The authorization of a route is applied before anything that route fronts, so this is
/// what makes a refusal observable without a socket: nothing the `$connect` route targets
/// is reached by a handshake refused here. Two answers are computed from the api's own
/// records:
///
/// * an authorizer-backed route ([`crate::state::AUTHORIZER_BACKED_AUTHORIZATION_TYPES`])
///   whose `AuthorizerId` names no authorizer of this api cannot be served at all — the
///   decision has no configuration to come from — which is a fault of the api's
///   configuration and not of the caller: `500`. The writers of a route refuse that
///   association ([`crate::state::UNRESOLVED_AUTHORIZER_MESSAGE`]), so this is a store
///   that lost the authorizer some other way and not a route a caller can create;
/// * an authorizer whose `IdentitySource` list is not satisfied by the request is `401`,
///   and its Lambda is NOT invoked: "These parameters are used to perform runtime
///   validation … by verifying all of the identity-related request parameters are present
///   in the request, not null, and non-empty. Only when this is true does the authorizer
///   invoke the authorizer Lambda function. Otherwise, it returns a 401 Unauthorized
///   response without calling the Lambda function" (`Authorizer$IdentitySource`).
///
/// An authorizer whose identity sources ARE satisfied is one whose own function has yet to
/// decide, which is an invocation: [`connect_authorization`] answers that authorizer and the
/// identity its decision is about, and a caller holding the in-process transport obtains the
/// decision ([`connect_authorizer`]). A caller without one — the service endpoint, whose seam
/// is synchronous — lets the handshake through this step, which changes nothing about what it
/// answers.
///
/// The route read is the one the stage's DEPLOYMENT froze
/// ([`crate::state::Deployment::configuration`]), so a stage pinned to an older deployment
/// is authorized by the `$connect` route as it stood then; the authorizer that route names
/// is the api's current record of it, which is where authorizer configuration lives.
///
/// The work this adds per handshake is bounded, and so is the PRODUCT of its two factors:
/// the route is found by one scan of the deployed routes, capped at
/// [`crate::state::MAX_ROUTES_PER_API`]; the request's query string is decoded ONCE into
/// [`query_parameters`], never once per identity source; and the identity sources then
/// looked up in it are capped at [`crate::state::MAX_IDENTITY_SOURCES_PER_AUTHORIZER`] by
/// the create that stored them.
pub(crate) fn connect_authorization_refusal<H>(
    api: &Api,
    stage: &Stage,
    deployed: &crate::state::DeployedConfiguration,
    header: H,
    uri: &str,
) -> Option<(u16, &'static str)>
where
    H: Fn(&str) -> Option<String>,
{
    connect_authorization(api, stage, deployed, header, uri).err()
}

/// The AUTHORIZER a connection request's own route still owes a decision from, and the
/// identity-source VALUES that decision is about and is keyed by.
pub(crate) type OwedDecision = (crate::state::Authorizer, Vec<String>);

/// The refusal a step of the handshake owes by itself: a status and the sentence it carries.
pub(crate) type HandshakeRefusal = (u16, &'static str);

/// The DECISION the connection request's authorization is still owed, or the refusal its
/// precondition already owes.
///
/// `Ok(None)` is a handshake no authorizer decides — the `$connect` route's
/// `AuthorizationType` names none, or the deployment holds no such route at all. `Ok(Some)`
/// is the authorizer whose function has yet to run and the identity-source VALUES its
/// decision is about ("API Gateway uses the identity sources as the cache key"), which is
/// what a caller holding the in-process transport then obtains
/// ([`connect_authorizer::decided`]). `Err` is the refusal this step owes by itself; see
/// [`connect_authorization_refusal`] for the rule each carries.
pub(crate) fn connect_authorization<H>(
    api: &Api,
    stage: &Stage,
    deployed: &crate::state::DeployedConfiguration,
    header: H,
    uri: &str,
) -> Result<Option<OwedDecision>, HandshakeRefusal>
where
    H: Fn(&str) -> Option<String>,
{
    let Some(route) = deployed
        .routes
        .values()
        .find(|route| route.route_key == CONNECT_ROUTE_KEY)
    else {
        return Ok(None);
    };
    if !crate::state::AUTHORIZER_BACKED_AUTHORIZATION_TYPES
        .contains(&route.authorization_type.as_str())
    {
        // `NONE` is "open access" and `AWS_IAM` is decided by an IAM policy this emulator
        // holds no records of, so neither has an authorization this module can apply. The
        // other value of that constant, `JWT`, cannot reach this data plane: a JWT
        // authorizer is "supported only for HTTP APIs" and `validate_authorizer_config`
        // refuses one on a WEBSOCKET api, so the identity-source rule below is only ever
        // applied to the `REQUEST` authorizers it is the rule for.
        return Ok(None);
    }
    let Some(authorizer) = route
        .authorizer_id
        .as_deref()
        .and_then(|authorizer_id| api.authorizers.get(authorizer_id))
    else {
        tracing::warn!(
            api_id = %api.api_id,
            route_key = %route.route_key,
            authorization_type = %route.authorization_type,
            authorizer_id = ?route.authorizer_id,
            "the connection request's route is authorized by an authorizer this api does \
             not hold"
        );
        return Err((UNPLACEABLE_STATUS, INTERNAL_MESSAGE));
    };
    let query_parameters = query_parameters(uri);
    let absent: Vec<&str> = authorizer
        .identity_source
        .iter()
        .filter(|identity_source| {
            !presents_identity_source(
                identity_source,
                &header,
                &query_parameters,
                &stage.stage_variables,
            )
        })
        .map(String::as_str)
        .collect();
    if !absent.is_empty() {
        tracing::info!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            authorizer_id = %authorizer.authorizer_id,
            absent = ?absent,
            "refusing a WebSocket opening handshake: an identity source of the connection \
             request's authorizer is absent, so the authorizer is not invoked"
        );
        return Err((UNAUTHORIZED_STATUS, UNAUTHORIZED_MESSAGE));
    }
    // Every identity source is there, so the authorizer's own function decides. The VALUES
    // travel with the decision, which is what it is cached under.
    let identity = authorizer
        .identity_source
        .iter()
        .map(|identity_source| {
            presented_identity(
                identity_source,
                &header,
                &query_parameters,
                &stage.stage_variables,
            )
        })
        .collect();
    Ok(Some((authorizer.clone(), identity)))
}

/// The VALUE the handshake presents for one identity source, as the cache key carries it.
///
/// The three request-bound spellings are read — a header, a query string parameter and a
/// stage variable, the parts `Authorizer$IdentitySource` names — and anything else presents
/// the empty value, which is what the precondition above already admitted it as (a context
/// parameter, which every call has).
fn presented_identity<H>(
    identity_source: &str,
    header: H,
    query_parameters: &std::collections::BTreeMap<String, String>,
    stage_variables: &std::collections::BTreeMap<String, String>,
) -> String
where
    H: Fn(&str) -> Option<String>,
{
    if let Some(value) =
        super::data_plane::identity_source_value(identity_source, header, query_parameters)
    {
        return value;
    }
    super::data_plane::parameter_name(
        identity_source.trim(),
        super::data_plane::STAGE_VARIABLE_IDENTITY_SOURCES,
    )
    .and_then(|name| stage_variables.get(name).cloned())
    .unwrap_or_default()
}

/// Whether a comma-separated HTTP field value carries `token`, compared
/// case-insensitively.
fn has_token(field: &str, token: &str) -> bool {
    field
        .split(',')
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(token))
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns that a wire test cannot vary cheaply: which request
    //! is an RFC 6455 opening handshake. The readings it SHARES with the HTTP data plane
    //! are pinned in [`super::super::data_plane`], and the served behaviour end to end in
    //! `tests/all/websocket_handshake_data_plane.rs`.

    use super::*;

    /// A handshake addressed at `host` with `path`, carrying the three fields of RFC 6455
    /// §4.1 as `connection` / `upgrade` / `key` spell them.
    fn handshake<'r>(
        method: &'r str,
        host: &'r str,
        path: &'r str,
        connection: Option<&'r str>,
        upgrade: Option<&'r str>,
        key: Option<&'r str>,
    ) -> Handshake<'r> {
        Handshake {
            method,
            host,
            path,
            connection,
            upgrade,
            key,
        }
    }

    #[test]
    fn only_an_rfc_6455_opening_handshake_is_one() {
        let key = Some("dGhlIHNhbXBsZSBub25jZQ==");
        let cases: &[(&str, bool, Handshake<'_>)] = &[
            (
                "the three fields a client library sends",
                true,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("websocket"), key),
            ),
            (
                "a Connection field carrying more than one token",
                true,
                handshake(
                    "GET",
                    "host",
                    "/prod",
                    Some("keep-alive, Upgrade"),
                    Some("WebSocket"),
                    key,
                ),
            ),
            (
                "the fields in lower case, as HTTP/1.1 allows",
                true,
                handshake("get", "host", "/prod", Some("upgrade"), Some("websocket"), key),
            ),
            (
                "no upgrade asked for: an ordinary GET on the stage path",
                false,
                handshake("GET", "host", "/prod", Some("keep-alive"), None, None),
            ),
            (
                "an upgrade to another protocol",
                false,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("h2c"), None),
            ),
            (
                "the handshake fields without a key",
                false,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("websocket"), None),
            ),
            (
                "an empty key",
                false,
                handshake("GET", "host", "/prod", Some("Upgrade"), Some("websocket"), Some("  ")),
            ),
            (
                "a POST carrying the handshake fields",
                false,
                handshake("POST", "host", "/prod", Some("Upgrade"), Some("websocket"), key),
            ),
        ];
        for (case, expected, request) in cases {
            assert_eq!(
                request.is_opening(),
                *expected,
                "case {case:?}: expected is_opening() to be {expected}, got {:?}",
                request
            );
        }
    }

    #[test]
    fn the_connections_path_space_is_not_a_stage_handshake() {
        let key = Some("dGhlIHNhbXBsZSBub25jZQ==");
        for path in [
            "/@connections/abc",
            "/prod/@connections/abc",
            "/@connections",
        ] {
            let request = handshake("GET", "host", path, Some("Upgrade"), Some("websocket"), key);
            assert!(
                request.addresses_connections(),
                "case {path:?}: the @connections space belongs to ApiGatewayManagementApi"
            );
        }
        for path in ["/prod", "/prod/connections/abc", "/@connectionsx/abc"] {
            let request = handshake("GET", "host", path, Some("Upgrade"), Some("websocket"), key);
            assert!(
                !request.addresses_connections(),
                "case {path:?}: expected a stage path, not the @connections space"
            );
        }
    }
}
