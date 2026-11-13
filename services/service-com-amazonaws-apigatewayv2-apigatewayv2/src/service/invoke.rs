// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` HTTP data plane: how a request addressed to a deployed HTTP api
//! stage's invoke URL is recognised, authorized and answered.
//!
//! An HTTP api is reached at the URI its `ApiEndpoint` reports — "`{api-id}.execute-api.
//! {region}.amazonaws.com`. The stage name is typically appended to this URI to form a
//! complete path to a deployed API stage" (`Api$ApiEndpoint`) — so a client of a deployed
//! stage makes an ORDINARY HTTP request to a route of the api, which is no modelled
//! operation of this service at all: its host's first label is the api's own id, which
//! names no service, and its path is a route key's rather than a `/v2/…` route's. The
//! emulator's front door could resolve no target service for such a request and answered
//! `404` `UnknownService` — for the endpoint this very service told the caller to use. A
//! client could therefore not observe the thing an HTTP api exists to do: nothing was ever
//! served on the invoke URL, so a JWT authorizer in front of a route never evaluated a
//! token, and an emulator that never answers is indistinguishable from one that authorises
//! everything.
//!
//! This module is therefore the second data-plane half of
//! [`crate::service::EmulatedService`] (beside [`super::websocket`]'s): the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving ([`serve`], reached
//! from the one seam the router offers before it authenticates anything). It is a READ of
//! this service's own records — a request to a stage changes nothing here — and every
//! status it answers is computed from them.
//!
//! A client on the machine this emulator runs on cannot reach that host at all: nothing
//! resolves `{api-id}.execute-api.{region}.amazonaws.com` there, so the api is ANNOUNCED on
//! the emulator's own endpoint instead, with the api id in the PATH behind the
//! [`LOCAL_ENDPOINT_PREFIX`] marker (`crate::state::announced_execute_api_endpoint`), and a
//! client that built its own address carries in the path what it cannot carry in the host —
//! the api id ahead of the stage (plainly, or behind that same marker), or the stage alone.
//! [`resolve`] reads every one of those addressings, and each is checked against this
//! service's own records, so one request still has one claimant. The sibling REST data plane
//! (`service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`)
//! resolves the same shapes for its own api ids, and the WEBSOCKET half of THIS service
//! ([`super::websocket`]) resolves only WEBSOCKET apis, so each claims only what it can
//! answer about.
//!
//! # What it resolves, in order
//!
//! 1. the request names an `Api` this service holds whose `ProtocolType` is `HTTP` — by the
//!    host's first label, by the first path segment, or, on a host that names no service,
//!    by the stage segment of its path ([`resolve`]);
//! 1b. that api's `DisableExecuteApiEndpoint` has not turned this endpoint off. While it
//!    has, the endpoint serves nothing of the api and every request to it is refused `403`
//!    `{"message":"Forbidden"}` before a stage is even looked for, and it serves again as
//!    soon as the member is cleared ([`super::data_plane::serves_default_endpoint`]);
//! 2. the path within that api names a `Stage` of it: its first segment, or the
//!    [`crate::state::DEFAULT_STAGE_NAME`] stage, which is "the stage that serves an api's
//!    root" and so is reached with no stage segment in the path at all — that is the
//!    "typically" of `Api$ApiEndpoint`;
//! 3. that stage has a deployment released to it. A step that resolves nothing is `403`
//!    `{"message":"Forbidden"}`, the answer an `execute-api` endpoint gives for an api it
//!    does not serve and for a stage with no deployment;
//! 4. the method and the remaining path match a `Route` of the api, most specific match
//!    first ([`matching_route`]); nothing matching is `404` `{"message":"Not Found"}`;
//! 5. that route's `AuthorizationType` is applied BEFORE anything the route fronts
//!    ([`authorization_refusal`]) — which is what this module makes observable: a JWT
//!    authorizer's gate ([`super::jwt`]), the identity-source precondition of a `REQUEST`
//!    authorizer, and the credential an `AWS_IAM` route needs;
//! 6. the request's PAYLOAD is within the quota an HTTP api's endpoint accepts
//!    ([`crate::state::MAX_HTTP_API_PAYLOAD_BYTES`], 10 MiB, which no member of this model
//!    adjusts). A body past it is refused by the GATEWAY — `413`, before step 7 runs at
//!    all, so the integration observes no invocation for it — and a body of exactly the
//!    quota is passed on to step 7 like any other ([`payload_refusal`]);
//! 6b. the serving stage's `RouteSettings` for that route admit the request: a route the
//!    stage throttles is refused `429` ([`crate::state::route_throttle`]) and its
//!    integration is not reached, while a twin route of the same stage is served;
//! 7. the route's integration decides the response.
//!
//! On top of those, the api's own `CorsConfiguration` decorates whatever is answered for it
//! ([`cors_decoration`]): `Cors` is a member of the API rather than of a backend
//! ("Represents a CORS configuration. Supported only for HTTP APIs"), so the gateway
//! applies it to the answer it returns — an allow-listed `Origin` is echoed back, a
//! preflight asking about an allow-listed method additionally carries the configured
//! allow-lists, and a request the configuration admits nothing of carries no CORS header at
//! all.
//!
//! A preflight is DECORATED rather than answered in place of the route: it walks steps 1-7
//! like any other request, and the decoration is put on what they answered. That is what the
//! recorded behaviour of a configured api shows — a preflight carrying an allow-listed
//! origin and method reaches the `$default` catch-all and is answered by the route's own
//! backend, with the CORS headers on that answer — so an `OPTIONS` request is not
//! short-circuited here. The consequence to know about: for an api whose routes match no
//! `OPTIONS` request, a preflight meets step 4 and is answered `404` with the decoration,
//! rather than a status the gateway chose on its own.
//!
//! An origin and a method are both matched EXACTLY against the collection that admits them
//! (or through its wildcard entry): `AllowOrigins` is "a collection of allowed origins", and
//! neither the model nor the recorded behaviour of a configured api admits a spelling the
//! collection does not hold.
//!
//! # How step 7 is performed, and what each fault answers
//!
//! The backend is REACHED, in process, over the transport `OperationContext::callback`
//! hands a handler ([`dispatch`]), so what a client receives is what that backend answered:
//!
//! * an `AWS_PROXY` route naming a Lambda FUNCTION invokes it with the event its
//!   `PayloadFormatVersion` defines ([`proxy_event`]) and answers with the response that
//!   version defines ([`proxy_response`]);
//! * an `HTTP_PROXY` route's endpoint is a URI the CALLER can reach, and this emulator
//!   forwards no request to a destination its own configuration names: the caller is sent
//!   there instead, `307 Temporary Redirect` to the upstream the integration resolved, which
//!   preserves the request's method and body, so the client makes exactly the request the
//!   gateway would have forwarded and nothing about the upstream's answer is invented;
//! * `MOCK` — the one type that answers without a backend — "is not permitted for APIs with
//!   a protocol type of HTTP" ([`crate::state::WEBSOCKET_ONLY_INTEGRATION_TYPES`]), so it
//!   cannot be configured here, and the two non-proxy types (`AWS`, `HTTP`) take the request
//!   and response templates a WebSocket api's integration carries.
//!
//! Nothing is faked: a body, a status or a header invented for a backend that never ran
//! would be indistinguishable from a real one. So `500`
//! `{"message":"Internal Server Error"}` is this data plane's mapping for the whole class
//! "the integration produced no response" — a handler that raised, a function the gateway
//! could not invoke, an answer the payload format cannot be read as, a route with nothing
//! behind it — and it is NOT the `502` the REST data plane answers for a malformed proxy
//! response, nor the scaffold channel's `501`
//! (`tests/all/http_api_integration_fault_status.rs` pins the mapping across the
//! configurations and fault classes it holds for). The one class that maps elsewhere is an
//! integration that RAN and did not answer within its `TimeoutInMillis`, which an HTTP api
//! answers `503`: the deadline is applied to the run itself, so nothing here times a call
//! that was never made.
//!
//! A `REQUEST` authorizer's ALLOW/DENY decision is likewise the answer of the Lambda
//! function its `AuthorizerUri` names, and is obtained the same way
//! ([`request_authorizer`]), kept for the TTL the authorizer declared and keyed by the
//! identity sources it was decided from. What step 5 still decides from stored records
//! alone is the part that is the SERVICE's own: the identity-source precondition the
//! authorizer is only invoked past, and a route whose authorizer the api does not hold. A
//! JWT authorizer is different in kind: its decision is a function of the token and of the
//! api's own `JWTConfiguration`, both of which are here, so it is computed in full
//! ([`super::jwt`]).
//!
//! One seam of the front door is what the two serving entry points below are about. The
//! router hands an `OperationContext` — the only in-process transport an invocation of the
//! route's function can be made over — only to a request it NAMES AN OPERATION for, and it
//! refuses a named operation whose browser-set headers say a browser sent it from another
//! site. So an invoke carrying such a header is served from the seam that answers an unnamed
//! request instead ([`serve_without_transport`]), where every step up to the integration is
//! the same and the integration itself is reported as the class it falls into.
//! `crate::service`'s own docs state that rule.

mod dispatch;
/// What the stage LOGS about a request it served: its access-log record, rendered by the
/// stage's own `$context` format.
mod access_logs;
/// What the stage PUBLISHES about a request it served: the CloudWatch counters and
/// measurements of that request.
mod request_metrics;
/// WHERE those datapoints land — the CloudWatch metric store — and the alarm wake-up each one
/// written there owes.
mod metrics;
// `pub(crate)` for the three steps BOTH data planes of this service perform, so the rule
// each states is written once and the two protocols cannot drift apart:
//
// * `lambda` — the one in-process hop to AWS Lambda, which an `AWS_PROXY` route and a
//   `REQUEST` authorizer of EITHER protocol is answered by;
// * `proxy_event::mint_request_id` and the renderings of a header or query map an event
//   states, which the WebSocket event states the same way;
// * `proxy_response` — how a function's answer is read back, read as the structured
//   response for a WebSocket route ([`proxy_response::websocket_reply`]);
// * `request_authorizer` — the cache, the invocation and the two response formats a
//   `REQUEST` authorizer's decision is read in.
pub(crate) mod lambda;
pub(crate) mod proxy_event;
pub(crate) mod proxy_response;
pub(crate) mod request_authorizer;
mod service_action;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, OperationContext};

use super::data_plane::{
    addresses_connections, api_id_of_host, authority_of, first_segment, forbidden, header_of,
    identity_source_value, joined_query_parameters, json_response, path_of,
    path_segments, presents_identity_source, query_of, query_parameters, service_named_by_host,
    split_first_segment, EXECUTE_API_LABEL, LOCAL_ENDPOINT_PREFIX, NOT_FOUND_MESSAGE,
    NOT_FOUND_STATUS, UNAUTHORIZED_MESSAGE, UNAUTHORIZED_STATUS, UNPLACEABLE_STATUS,
};
use super::jwt;
use crate::state::{
    Api, Authorizer, ComAmazonawsApigatewayv2Apigatewayv2State, DeployedConfiguration, Integration,
    Route, Stage,
};

/// The `RouteKey` that catches what no other route matches. AWS documents `$default` as
/// "the default route" of an HTTP api, the one reached when no route key matches the
/// request, and `Route$ApiGatewayManaged` names it ("the $default route is managed by API
/// Gateway") — a reserved key of the protocol rather than a value the model enumerates
/// (`RouteKey` targets the unconstrained `SelectionKey`).
const DEFAULT_ROUTE_KEY: &str = "$default";

/// The verb-independent catch-all a route key may name instead of a method: `ANY /{proxy+}`
/// is the key AWS's own quick-create and SAM templates write, and it matches every verb.
const ANY_METHOD: &str = "ANY";

/// The greedy suffix of a path-variable route-key segment: `{proxy+}` "matches the
/// remainder of the path", so such a segment stands for one or more request segments and
/// can only be the last one.
const GREEDY_VARIABLE_SUFFIX: char = '+';

/// The `AuthorizationType` that lets a request through with no authorization at all: "For
/// HTTP APIs, valid values are NONE for open access" (`Route$AuthorizationType`).
const AUTHORIZATION_NONE: &str = "NONE";

/// The `AuthorizationType` whose caller must sign the request: "AWS_IAM for using AWS IAM
/// permissions" (`Route$AuthorizationType`).
const AUTHORIZATION_AWS_IAM: &str = "AWS_IAM";

/// The `AuthorizationType` whose decision a JWT authorizer makes: "JWT for using JSON Web
/// Tokens" (`Route$AuthorizationType`).
const AUTHORIZATION_JWT: &str = "JWT";

/// The identity source a JWT authorizer extracts its token from when it records none.
///
/// `Authorizer$IdentitySource` for a JWT authorizer is "a single entry that specifies
/// where to extract the JSON Web Token (JWT) from inbound requests", and the example it
/// gives is this one; a configuration that names no entry still has to read the token
/// somewhere, and the `Authorization` header is where RFC 6750 puts a bearer token.
const DEFAULT_JWT_IDENTITY_SOURCE: &str = "$request.header.Authorization";

/// The sentence [`UNPLACEABLE_STATUS`] carries on the HTTP data plane: API Gateway's own
/// body for an HTTP api whose integration produced no response. Spelled as an HTTP api
/// spells it, which is not the REST data plane's `Internal server error`.
const INTERNAL_MESSAGE: &str = "Internal Server Error";

/// The status a request whose payload is past [`crate::state::MAX_HTTP_API_PAYLOAD_BYTES`]
/// is refused with: the gateway refuses the request itself rather than passing it to the
/// route's integration.
const PAYLOAD_TOO_LARGE_STATUS: u16 = 413;

/// The sentence that status carries, which — like every other sentence this endpoint
/// answers ([`INTERNAL_MESSAGE`], [`super::data_plane::FORBIDDEN_MESSAGE`],
/// [`NOT_FOUND_MESSAGE`], [`UNAUTHORIZED_MESSAGE`]) — is the status's own reason phrase
/// (RFC 7231 §6.5.11, the phrase for 413). The recorded answer for this refusal states
/// its status and not its body, and an HTTP api has no `GatewayResponse` resource to word
/// one with — that catalog, whose `REQUEST_TOO_LARGE` entry the REST plane words
/// differently, is bound to REST apis alone — so the phrase convention this endpoint
/// already follows decides it.
const PAYLOAD_TOO_LARGE_MESSAGE: &str = "Request Entity Too Large";

/// The request header that states how many bytes the payload has (RFC 9110 §8.6), read
/// only when the body itself is not in memory to be measured.
const CONTENT_LENGTH_HEADER: &str = "content-length";

/// The status a caller is sent to an upstream with: the redirect that preserves the
/// request's method and body (RFC 7231 §6.4.7), so the client repeats the request the
/// gateway would have forwarded.
const TEMPORARY_REDIRECT_STATUS: u16 = 307;

/// The header that redirect carries the upstream in (RFC 7231 §7.1.2).
const LOCATION_HEADER: &str = "location";

/// The request header that makes a request CROSS-ORIGIN: the origin the user agent took it
/// from (RFC 6454 §7). A request carrying none asks for no CORS decoration, so none is
/// added to its answer.
const ORIGIN_HEADER: &str = "origin";

/// The method a CORS PREFLIGHT is made with (Fetch, "CORS-preflight request").
const PREFLIGHT_METHOD: &str = "OPTIONS";

/// The request header a preflight names the method it is asking about in; a preflight is an
/// [`PREFLIGHT_METHOD`] request that carries it.
const REQUEST_METHOD_HEADER: &str = "access-control-request-method";

/// The response header that names the origin the answer may be read by —
/// `Cors$AllowOrigins`, "a collection of allowed origins".
const ALLOW_ORIGIN_HEADER: &str = "access-control-allow-origin";

/// The response header that names the methods a preflight's origin may use —
/// `Cors$AllowMethods`, "a collection of allowed HTTP methods".
const ALLOW_METHODS_HEADER: &str = "access-control-allow-methods";

/// The response header that names the request headers a preflight's origin may send —
/// `Cors$AllowHeaders`, "a collection of allowed headers".
const ALLOW_HEADERS_HEADER: &str = "access-control-allow-headers";

/// The response header that names the response headers the origin's script may read —
/// `Cors$ExposeHeaders`, "a collection of exposed headers".
const EXPOSE_HEADERS_HEADER: &str = "access-control-expose-headers";

/// The response header that admits a CREDENTIALED cross-origin request —
/// `Cors$AllowCredentials`, "whether credentials are included in the CORS request".
const ALLOW_CREDENTIALS_HEADER: &str = "access-control-allow-credentials";

/// The only value [`ALLOW_CREDENTIALS_HEADER`] carries (Fetch): the header is sent when the
/// configuration allows credentials and omitted when it does not.
const ALLOW_CREDENTIALS_VALUE: &str = "true";

/// The response header that caps how long a preflight's answer may be reused —
/// `Cors$MaxAge`, "the number of seconds that the browser should cache preflight request
/// results".
const MAX_AGE_HEADER: &str = "access-control-max-age";

/// How a CORS response header carries a LIST: one comma-separated value (RFC 9110 §5.3).
const CORS_LIST_SEPARATOR: &str = ",";

/// The parts of a request this data plane reads: the method, and the host and path it is
/// addressed at.
///
/// One reader for BOTH request shapes the seams are handed — `http::request::Parts` in
/// `Service::can_handle` and the runtime-API request everywhere else — so the claim and
/// the serve cannot disagree about what a request addresses.
#[derive(Debug, Clone, Copy)]
struct Addressed<'r> {
    method: &'r str,
    /// `None` when the request carries no host at all, which names no api and no service.
    host: Option<&'r str>,
    path: &'r str,
}

impl<'r> Addressed<'r> {
    /// The reading of the request parts `Service::can_handle` is handed.
    fn of_parts(parts: &'r http::request::Parts) -> Self {
        Self {
            method: parts.method.as_str(),
            host: header_of(&parts.headers, "host")
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str)),
            path: parts.uri.path(),
        }
    }

    /// The reading of the runtime-API request, which carries its URI as a string.
    fn of_request(req: &'r RtRequest) -> Self {
        Self {
            method: req.method(),
            host: req.headers().get("host").or_else(|| authority_of(req.uri())),
            path: path_of(req.uri()),
        }
    }
}

/// What a request addresses, as this service's records resolve it: the api, and the path
/// WITHIN that api — the part that names a stage and a route.
#[derive(Debug)]
struct Resolved<'s, 'r> {
    api: &'s Api,
    invoke_path: &'r str,
}

/// The HTTP api `addressed` addresses, or `None` when this data plane serves it not at all.
///
/// THREE addressings resolve, because a client reaches this emulator at a different address
/// than it reaches AWS at:
///
/// * HOST-STYLE, the addressing `Api$ApiEndpoint` documents: the host's first label names
///   the api and the whole path is the invoke path. A host that IS an `execute-api`
///   endpoint but whose api id this service does not hold resolves to nothing at all —
///   that host belongs to another api (an API Gateway REST api is reached on the same host
///   shape and its own service claims it), so reading its path as one of THIS service's
///   stages would make two services claim one request;
/// * PATH-STYLE: the api id is the FIRST PATH SEGMENT and the invoke path is what follows
///   it. A client on this machine resolves the host above nowhere, so it reaches the
///   emulator's own endpoint and carries in the path what it cannot carry in the host. It
///   is read only on a host that names NO SERVICE (the emulator's own endpoint) or names
///   THIS one, so a first segment that happens to equal an api id cannot take a request
///   away from the service whose endpoint the host names. Its [`LOCAL_ENDPOINT_PREFIX`]
///   spelling, which marks the rewritten host explicitly, is read on ANY host
///   ([`local_endpoint_path`]);
/// * STAGE-ONLY, for a client that carries the invoke path alone: a host that names NO
///   SERVICE at all is resolved by the STAGE segment of the path, the only part of such a
///   request that names a deployed stage — the same host-independent claim the WebSocket
///   half of this service makes over a stage's path. It is the LAST addressing tried,
///   because it is the only one that does not carry an api id: when two apis of this
///   service hold a stage of that name the request names no ONE api, so nothing is resolved
///   rather than one of them guessed at.
///
/// The limit of that last addressing, stated because the uniqueness guard above cannot
/// reach it: a stage name is unique within an api, not across SERVICES. The REST data
/// plane claims the same path space for its own api ids, so one stage name held by a REST
/// api AND by an HTTP api is claimed by both services and the router answers
/// `500 AmbiguousService` for it — which is why every addressing that names an api is
/// tried first, and why a client whose stage name may collide reaches the api through one
/// of them (the api's own host, or the api id in the path). Nothing here can decide that
/// collision alone: a claim narrowed to "no OTHER service holds this stage name" would
/// need another service's records, which this service does not read.
///
/// This service's own CONTROL-plane path space (`crate::ROUTE_PATH_SEGMENTS`, the single
/// segment `v2`) is never read as a stage: a request whose first segment is one of those is
/// a control-plane call on whatever host it arrives at, and answering it from the data
/// plane would take `/v2/apis` away from the operations that serve it. A stage named like
/// one of those segments is still reachable through the addressings above, where the api is
/// named before the stage is.
///
/// An api whose `ProtocolType` is not `HTTP` is never resolved here: a request to a
/// WEBSOCKET api's stage is its own data plane's ([`super::websocket`]).
fn resolve<'s, 'r>(
    store: &'s ComAmazonawsApigatewayv2Apigatewayv2State,
    addressed: &Addressed<'r>,
) -> Option<Resolved<'s, 'r>> {
    // The `@connections` path space belongs to ApiGatewayManagementApi, on either host.
    if addresses_connections(addressed.path) {
        return None;
    }
    // This service's own CONTROL-plane path space is never a data-plane path, on any host:
    // a request whose first segment is one of `crate::ROUTE_PATH_SEGMENTS` is a
    // control-plane call wherever it arrives, and claiming it here would take `/v2/apis`
    // away from the operations that serve it.
    if first_segment(addressed.path).is_some_and(|first| crate::ROUTE_PATH_SEGMENTS.contains(&first))
    {
        return None;
    }
    // LOCAL-ENDPOINT style first, because its marker is unambiguous on any host.
    if let Some((api_id, invoke_path)) = local_endpoint_path(addressed.path) {
        return servable(store.apis.get(api_id)).map(|api| Resolved { api, invoke_path });
    }
    if let Some(api_id) = addressed.host.and_then(api_id_of_host) {
        return servable(store.apis.get(api_id)).map(|api| Resolved {
            api,
            invoke_path: addressed.path,
        });
    }
    let (first, rest) = split_first_segment(addressed.path)?;
    // Which service the host names, by the front door's own rule: `None` is the emulator's
    // own endpoint, and this service's own key is its control-plane host.
    let host_service = addressed.host.and_then(service_named_by_host);
    if host_service
        .as_deref()
        .is_some_and(|service| service != crate::ROUTE_KEY)
    {
        // Another service's endpoint: whatever its path carries is that service's to
        // answer, so neither addressing below is read and one request keeps one claimant.
        return None;
    }
    if let Some(api) = servable(store.apis.get(first)) {
        return Some(Resolved {
            api,
            invoke_path: rest,
        });
    }
    if host_service.is_some() {
        // This service's own control-plane host: a path segment that is no api id of this
        // account is a control-plane call, not a stage.
        return None;
    }
    let mut holding = store
        .apis
        .values()
        .filter(|api| is_http(api))
        .filter(|api| api.stages.contains_key(first));
    let api = holding.next()?;
    holding.next().is_none().then_some(Resolved {
        api,
        invoke_path: addressed.path,
    })
}

/// `api` when it is an api of THIS plane's protocol — the one narrowing every addressing
/// above shares.
///
/// Whether that api's default endpoint still SERVES is not read here: an api whose
/// `DisableExecuteApiEndpoint` turned the endpoint off is still the api its own endpoint
/// addresses, so the request is resolved and then REFUSED by this plane
/// ([`resolved`], [`super::data_plane::serves_default_endpoint`]) rather than left unclaimed
/// for the front door to call unknown.
fn servable(api: Option<&Api>) -> Option<&Api> {
    api.filter(|api| is_http(api))
}

/// The api id and invoke path of a request that names the api's endpoint INSIDE ITS PATH,
/// behind the [`LOCAL_ENDPOINT_PREFIX`] marker, or `None` for a path carrying no marker.
///
/// A client on the machine this emulator runs on resolves
/// `{api-id}.execute-api.{region}.amazonaws.com` nowhere, so it rewrites the endpoint it
/// was given into one it can reach: the emulator's own address, with the endpoint the
/// original URI named carried in the path ahead of the rest of it —
/// `/_aws/execute-api/{api-id}/{stage}/{route-path}`. The marker makes that rewriting
/// explicit, so it is read on ANY host and is never confused with a path of the api's own:
/// no route key of an api can begin with it, because the segment the api would have to
/// own is the emulator's.
///
/// Both spellings of the endpoint behind the marker resolve — the endpoint PREFIX followed
/// by the api id (`/_aws/execute-api/{api-id}/…`), and the endpoint HOST itself
/// (`/_aws/{api-id}.execute-api.{region}.amazonaws.com/…`) — because what a rewriter has
/// to carry across is the host, and those are the two ways to write it.
///
/// `pub(super)` because BOTH data planes are addressed this way: a client that rewrote a
/// WebSocket api's endpoint into the emulator's carries the api id in the path exactly like
/// this, so [`super::websocket`] reads the marker through this one function rather than
/// writing a second reading of it.
pub(super) fn local_endpoint_path(path: &str) -> Option<(&str, &str)> {
    let (marker, rest) = split_first_segment(path)?;
    if marker != LOCAL_ENDPOINT_PREFIX {
        return None;
    }
    let (endpoint, rest) = split_first_segment(rest)?;
    if let Some(api_id) = api_id_of_host(endpoint) {
        return Some((api_id, rest));
    }
    // The endpoint PREFIX spelling: the api id is the segment after it.
    (endpoint == EXECUTE_API_LABEL)
        .then(|| split_first_segment(rest))
        .flatten()
}

/// Whether this api speaks the HTTP protocol: `ProtocolType` is an enum of exactly
/// `HTTP | WEBSOCKET` (`smithy_query(query="shape", shape="ProtocolType")`).
fn is_http(api: &Api) -> bool {
    api.protocol_type == crate::state::HTTP_PROTOCOL_TYPE
}

/// Whether THIS data plane serves the request these parts came from — the HTTP half of
/// `Service::can_handle`.
///
/// Narrowed to an api this service HOLDS, read from the one shared store, plus the one
/// address no api of this emulator holds at all ([`claims_an_unresolved_endpoint`]), so
/// exactly one service claims a request.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    let addressed = Addressed::of_parts(parts);
    claims(engine, &addressed)
        || claims_an_unresolved_endpoint(engine, &addressed, || {
            ::service_com_amazonaws_apigateway_backplanecontrolservice::service::invoke::claims_parts(
                engine, parts,
            )
        })
}

/// [`claims_parts`] for the runtime-API request the serving seams are handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let addressed = Addressed::of_request(req);
    claims(engine, &addressed)
        || claims_an_unresolved_endpoint(engine, &addressed, || {
            ::service_com_amazonaws_apigateway_backplanecontrolservice::service::invoke::claims_request(
                engine, req,
            )
        })
}

/// Whether this data plane answers an `execute-api` ENDPOINT that resolves to NO api of this
/// emulator — the address a client keeps calling after the api behind it was deleted.
///
/// AWS serves one `execute-api` endpoint space, and an endpoint whose api id resolves to no
/// api is answered there rather than by nothing: the answer is `404`
/// `{"message": "Not Found"}` ([`resolved`]). The endpoint space is SHARED by the two data
/// planes of API Gateway — a REST api is reached on the same host shape, and that service's
/// own data plane claims a request it resolves — so an id neither of them holds is claimed by
/// exactly one of them, which is this one. The REST data plane is asked through its OWN claim
/// (`rest_claims`), so the two cannot disagree about which requests it answers, and it is
/// asked with no lock of this service held, in the order every cross-service read of this
/// crate follows ([`crate::state::rest_endpoint`]).
///
/// The api id is read from the two addressings that NAME an endpoint: the host AWS documents
/// (`{api-id}.execute-api.{region}.amazonaws.com`), and the emulator's own rewriting of it
/// behind the [`LOCAL_ENDPOINT_PREFIX`] marker, which is the endpoint this service ANNOUNCES
/// to a client that resolves nothing else (`crate::state::announced_execute_api_endpoint`) —
/// so the address a client was handed answers the same way whichever of the two it is.
///
/// What this does NOT claim, so nothing an api of this service holds changes hands: an
/// address whose api id this service HOLDS (an api that turned its default endpoint off is
/// refused by [`resolved`], and one of the other protocol is [`super::websocket`]'s), the
/// addressings that name NO endpoint (a bare stage path, which names no api id at all), and
/// the `@connections` path space, which belongs to ApiGatewayManagementApi on this same host.
fn claims_an_unresolved_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    addressed: &Addressed<'_>,
    rest_claims: impl FnOnce() -> bool,
) -> bool {
    if addresses_connections(addressed.path) {
        return false;
    }
    let Some(api_id) = addressed_endpoint_api_id(addressed) else {
        return false;
    };
    if rest_claims() {
        return false;
    }
    crate::state::service_state(engine)
        .is_some_and(|state| !state.lock().apis.contains_key(api_id))
}

/// The api id the ENDPOINT `addressed` names, or `None` for an address that names no endpoint
/// of the `execute-api` space at all.
fn addressed_endpoint_api_id<'r>(addressed: &Addressed<'r>) -> Option<&'r str> {
    local_endpoint_path(addressed.path)
        .map(|(api_id, _)| api_id)
        .or_else(|| addressed.host.and_then(api_id_of_host))
}

/// Whether this data plane serves `addressed`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), addressed).is_some())
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization` header a
/// signed SDK request carries, or in the `X-Amz-Credential` query parameter of a presigned
/// URL.
///
/// Only PRESENCE is read, which is all a route's `AuthorizationType` turns on: `AWS_IAM`
/// authorizes the caller by IAM permissions, so a request carrying no credential at all is
/// refused here, and one that carries a credential is admitted. The presigned form is read
/// beside the header form because a presigned URL carries its credential in the query
/// string, so reading only the header would refuse a presigned invoke of an `AWS_IAM` route
/// that AWS admits.
///
/// Whether the SIGNATURE over that credential is correct is the emulator's own check, never
/// a service's — and it is one the emulator cannot make for this plane: it verifies a
/// request against the signing name of the service that claimed it, which for this service
/// is its control plane's, while an invoke is signed under the data plane's own
/// (`crate::service`'s module docs state the whole rule).
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    query_of(req.uri())
        .unwrap_or_default()
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// Serve one request addressed to a deployed HTTP api stage, from this service's own
/// records and from the backend the matched route fronts.
///
/// `ctx` is the in-process transport the route's integration is performed over
/// ([`dispatch`]). The store's guard is held for the RESOLUTION only and released before the
/// backend is reached, so one caller's backend cannot stall another caller's request.
pub async fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> RtResponse {
    // Minted HERE, before anything is served, because the id names the request in THREE
    // places: the header this response reports it in, the `$context.requestId` the
    // invocation's own event carries, and the stage's access-log record ([`access_logs`]) —
    // which is what lets a client join the answer it holds to the record of it.
    let request_id = proxy_event::mint_request_id();
    // MEASURED over the whole invoke, because that is what the access log reports as the
    // latency of answering this request.
    let started = ::std::time::Instant::now();
    let answered = match resolved(engine, req, &request_id) {
        Err(answered) => *answered,
        Ok((decoration, Answer::Now(response))) => decorated(*response, &decoration),
        Ok((decoration, Answer::Dispatch(dispatch))) => {
            decorated(dispatch::perform(engine, ctx, *dispatch).await, &decoration)
        }
    };
    let served = ::service_common::query_error_envelope::with_stated_request_id_header(
        answered,
        &request_id,
    );
    let latency = started.elapsed();
    // What the STAGE publishes about the request it just answered ([`request_metrics`]), and
    // what it LOGS about it ([`access_logs`]), delivered to the destination the stage names.
    // After the response is computed and never before: a counter counts a request that was
    // answered, and a record is about one.
    request_metrics::published(engine, req, &served, latency);
    access_logs::delivered(engine, req, ctx, &request_id, &served, latency).await;
    served
}

/// [`serve`] from a seam that carries NO request context, so the route's integration cannot
/// be performed over the transport the request being served would have carried: every step up
/// to the integration is the same, and the integration is performed over the transport this
/// service REMEMBERED instead ([`crate::state::invoke_transport`]).
///
/// `crate::service`'s module docs state which requests are served here and why. The one that
/// is — an invoke whose browser-set headers read as cross-site — is exactly the request an
/// api's `CorsConfiguration` exists for, so it is served like any other: the route's
/// integration is performed and its response is the answer. Only an invoke that arrives
/// before this service has ever been handed a transport, or one made on a runtime that
/// cannot drive it, has its integration reported as the class it falls into instead
/// ([`dispatch::unperformable`]).
///
/// `identity` is this service's own vended identity, which the context an integration is
/// performed under names — the service's, not the request's, so it is the same identity a
/// named invoke's context carries.
pub fn serve_without_transport<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    identity: &::smithy_server_runtime::ServiceIdentity,
    req: &RtRequest,
) -> RtResponse {
    let request_id = proxy_event::mint_request_id();
    let started = ::std::time::Instant::now();
    let answered = match resolved(&**engine, req, &request_id) {
        Err(answered) => *answered,
        Ok((decoration, Answer::Now(response))) => decorated(*response, &decoration),
        Ok((decoration, Answer::Dispatch(dispatch))) => decorated(
            performed_over_the_remembered_transport(engine, identity, req, *dispatch),
            &decoration,
        ),
    };
    // No access-log record is delivered from here: the delivery is a CALL into the
    // CloudWatch Logs this emulator mounts, and this seam has no request of its own to
    // measure or to carry one over. The response still names the request it answered.
    let served =
        ::service_common::query_error_envelope::with_stated_request_id_header(answered, &request_id);
    // The metric publication IS made from here: it is a write into the metric store this
    // emulator's CloudWatch holds on the same engine rather than a call over a transport, so
    // a request served from this seam is counted like any other.
    request_metrics::published(&**engine, req, &served, started.elapsed());
    served
}

/// Perform `dispatch` over the transport this service remembered, and answer what the route's
/// backend produced.
///
/// The context the integration is performed under is this service's own identity, the
/// remembered transport, and the request's own head — rebuilt from the request being served
/// ([`request_head`]), so every step reads the method, the URI and the headers the client
/// sent. It carries no request EXTENSION, which is where the front door would have put an
/// authenticated caller: the requests served from this seam present no credential this
/// emulator verified, so there is none to carry.
///
/// Every such integration holds one [`IntegrationPermit`] for as long as it is in flight,
/// which is what bounds this seam: how many of these waits exist at once, and how many times
/// a chain can come back round through here. An integration admitted past no permit is
/// reported as the class it falls into, with the reason logged and never told to the caller.
fn performed_over_the_remembered_transport<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    identity: &::smithy_server_runtime::ServiceIdentity,
    req: &RtRequest,
    dispatch: dispatch::Dispatch,
) -> RtResponse {
    let remembered = crate::state::service_state(&**engine)
        .and_then(|state| state.lock().invoke_transport.held());
    let (Some(callback), Some(head)) = (remembered, request_head(req)) else {
        ::tracing::warn!(
            api_id = %dispatch.request.api_id,
            stage = %dispatch.request.stage_name,
            route_key = %dispatch.route_key,
            "this invoke was served from a seam with no request context of its own and this \
             service holds no transport to perform the route's integration over"
        );
        return dispatch::unperformable(&**engine, &dispatch);
    };
    // The runtime is resolved BEFORE the dispatch is consumed, so an invoke this seam cannot
    // perform the integration of is still answered everything that needs no transport at all
    // — the stage's throttle, and the redirect an `HTTP_PROXY` upstream is the caller's to
    // reach ([`dispatch::unperformable`]).
    let Some(handle) = drivable_runtime() else {
        ::tracing::warn!(
            api_id = %dispatch.request.api_id,
            stage = %dispatch.request.stage_name,
            route_key = %dispatch.route_key,
            "this invoke reached no runtime that can drive the route's integration while this \
             seam waits for it, so the integration was not performed"
        );
        return dispatch::unperformable(&**engine, &dispatch);
    };
    let Some(permit) = IntegrationPermit::claim() else {
        ::tracing::warn!(
            api_id = %dispatch.request.api_id,
            stage = %dispatch.request.stage_name,
            route_key = %dispatch.route_key,
            in_flight = MAX_INTEGRATIONS_IN_FLIGHT,
            "as many integrations are in flight from this seam as it performs at once, so \
             this route's integration was not performed"
        );
        return dispatch::unperformable(&**engine, &dispatch);
    };
    // Owned, so the work below is a future of its own that the runtime can drive: the engine
    // handle, the identity and the request's head move into it, and the context is built
    // inside it from them.
    let owned_engine = ::std::sync::Arc::clone(engine);
    let owned_identity = identity.clone();
    let performed = async move {
        let ctx = OperationContext::new(&head, &owned_identity, callback);
        dispatch::perform(&*owned_engine, &ctx, dispatch).await
    };
    let answered = awaited(&handle, performed);
    drop(permit);
    match answered {
        Some(response) => *response,
        None => {
            // The task panicked or was cancelled, which is an integration that produced no
            // response. The reason stays in this emulator's own log.
            ::tracing::warn!(
                "the task performing this route's integration produced no answer to map"
            );
            json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None)
        }
    }
}

/// How many route integrations this seam performs AT ONCE, across the whole process.
///
/// The transport an integration is performed over here is a REMEMBERED one, so the call made
/// over it carries the re-entrancy count of the call that handed the transport over rather
/// than of the request being served, and the framework exposes no way to re-stamp a handle
/// with another count — so the router's own backstop on a re-entrant chain
/// (`smithy_server_runtime::CALLBACK_RECURSION_LIMIT`) cannot be advanced from here. This is
/// what takes its place: a chain that comes back round through this seam holds one permit per
/// lap it is still inside, so it is cut at the same number of laps that backstop would have
/// cut it at. The same count bounds how many of these waits exist at once.
const MAX_INTEGRATIONS_IN_FLIGHT: usize =
    ::smithy_server_runtime::CALLBACK_RECURSION_LIMIT as usize;

/// How many integrations this seam currently has in flight.
static INTEGRATIONS_IN_FLIGHT: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One of the [`MAX_INTEGRATIONS_IN_FLIGHT`] integrations this seam performs at once, given
/// back when it is dropped — on every path, including one a performed integration panicked
/// on.
struct IntegrationPermit;

impl IntegrationPermit {
    /// One permit, or `None` when this seam already has as many integrations in flight as it
    /// performs at once.
    fn claim() -> Option<Self> {
        let mut held = INTEGRATIONS_IN_FLIGHT.load(::std::sync::atomic::Ordering::Relaxed);
        loop {
            if held >= MAX_INTEGRATIONS_IN_FLIGHT {
                return None;
            }
            match INTEGRATIONS_IN_FLIGHT.compare_exchange_weak(
                held,
                held.saturating_add(1),
                ::std::sync::atomic::Ordering::AcqRel,
                ::std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Self),
                Err(current) => held = current,
            }
        }
    }
}

impl Drop for IntegrationPermit {
    fn drop(&mut self) {
        INTEGRATIONS_IN_FLIGHT.fetch_sub(1, ::std::sync::atomic::Ordering::AcqRel);
    }
}

/// The head of the request being served, as the [`OperationContext`] an integration is
/// performed under reads it: the method, the URI and every header the client sent.
///
/// `None` for a head the `http` crate will not build, which no request that reached this
/// point has — it arrived over that same crate's types.
fn request_head(req: &RtRequest) -> Option<http::request::Parts> {
    let mut builder = http::Request::builder()
        .method(req.method())
        .uri(req.uri());
    for (name, value) in req.headers().iter() {
        builder = builder.header(name, value);
    }
    Some(builder.body(()).ok()?.into_parts().0)
}

/// The runtime an integration can be performed on from a SYNCHRONOUS seam, or `None` when
/// this process is not on one.
///
/// It is the MULTI-THREAD runtime the emulator boots. A current-thread runtime is driven by
/// the one thread now inside the seam, which is therefore driving nothing while it waits: a
/// task spawned on it could not be polled and its timers would not advance, so an integration
/// is not performed from there at all rather than waited on. This is also what keeps
/// [`awaited`]'s `block_in_place` from the one runtime that refuses it.
fn drivable_runtime() -> Option<::tokio::runtime::Handle> {
    let handle = ::tokio::runtime::Handle::try_current().ok()?;
    (handle.runtime_flavor() == ::tokio::runtime::RuntimeFlavor::MultiThread).then_some(handle)
}

/// Run `work` to completion on `handle` from a SYNCHRONOUS seam, or `None` for work that
/// produced no answer because the task performing it panicked or was cancelled.
///
/// The route's integration is a call into another service of this emulator, which is
/// asynchronous, and the seam that asks for it is not — so the work is handed to the runtime
/// as a TASK rather than driven on the thread that is waiting for it. That is what lets the
/// deadlines inside it fire while this seam waits: the integration's own `TimeoutInMillis`
/// ([`dispatch`]) is on the runtime's own timer, and the task stays cancellable.
/// `block_in_place` hands this worker's queue to a replacement thread while it waits, so the
/// runtime keeps serving every other request.
fn awaited(
    handle: &::tokio::runtime::Handle,
    work: impl ::std::future::Future<Output = RtResponse> + Send + 'static,
) -> Option<Box<RtResponse>> {
    let task = handle.spawn(work);
    ::tokio::task::block_in_place(|| handle.block_on(task))
        .ok()
        .map(Box::new)
}

/// Everything decided from the STORE for one invoke: the api's CORS decoration, and either
/// the gateway's own answer or the integration left to perform.
///
/// `Err` is an answer that carries no decoration at all — an api this endpoint does not
/// serve, which has no CORS configuration to decorate with. It is boxed, like
/// [`Answer::Now`], because a rendered response is a large value to move through a return.
#[allow(clippy::type_complexity)]
fn resolved<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    request_id: &str,
) -> Result<(Vec<(&'static str, String)>, Answer), Box<RtResponse>> {
    let addressed = Addressed::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return Err(Box::new(json_response(
            UNPLACEABLE_STATUS,
            INTERNAL_MESSAGE,
            None,
        )));
    };
    let store = state.lock();
    // Either this endpoint resolves to no api of this emulator at all — the address a client
    // keeps calling after the api behind it was deleted, which this data plane owns
    // ([`claims_an_unresolved_endpoint`]) — or the api was deleted between the claim and
    // now, which is the same fact arriving a moment later. API Gateway answers an
    // `execute-api` endpoint that resolves to no api `404` `{"message": "Not Found"}`, and
    // an endpoint that resolves to nothing has no CORS configuration to decorate it with.
    let Some(resolved) = resolve(&store, &addressed) else {
        return Err(Box::new(json_response(
            NOT_FOUND_STATUS,
            NOT_FOUND_MESSAGE,
            None,
        )));
    };
    // The DEFAULT endpoint this request was addressed at serves nothing while the api turned
    // it off ([`super::data_plane::serves_default_endpoint`]), so the refusal is answered
    // before any stage, deployment, route or authorization of the api is read — and with none
    // of the api's CORS decoration, since an endpoint that serves nothing answers for no part
    // of its configuration.
    if !super::data_plane::serves_default_endpoint(resolved.api) {
        tracing::debug!(
            api_id = %resolved.api.api_id,
            "this api's default execute-api endpoint is disabled, so it serves nothing"
        );
        return Err(Box::new(forbidden()));
    }
    // The api's own CORS configuration decides the decoration of WHATEVER is answered
    // below, so it is computed once, from the api and the request alone, before the stage
    // and the route are resolved; see [`cors_decoration`].
    let decoration = cors_decoration(resolved.api, req);
    Ok((
        decoration,
        resolve_answer(&resolved, &addressed, req, request_id),
    ))
}

/// What the resolution under the store's guard came to: the answer itself, or the
/// integration still to be performed once the guard is released.
///
/// Both arms are BOXED, because both a rendered response and a whole dispatch are large
/// values to move through every return of the walk below.
enum Answer {
    /// The gateway's own answer — a refusal, or a status it decided without a backend.
    Now(Box<RtResponse>),
    /// Everything the route's backend needs, read out of the store.
    Dispatch(Box<dispatch::Dispatch>),
}

/// The answer for a request whose api this data plane HAS resolved: the stage, the
/// deployment released to it, the route and its authorization, then the integration.
fn resolve_answer(
    resolved: &Resolved<'_, '_>,
    addressed: &Addressed<'_>,
    req: &RtRequest,
    request_id: &str,
) -> Answer {
    let api = resolved.api;
    // The stage: the path's first segment when it names one, and otherwise the stage that
    // serves the api's root. A path that names neither is an endpoint that serves nothing.
    let Some((stage, route_path)) = stage_of(api, resolved.invoke_path) else {
        tracing::debug!(
            api_id = %api.api_id,
            path = %resolved.invoke_path,
            "the request names no stage of this api"
        );
        return Answer::Now(Box::new(forbidden()));
    };
    // The deployment the stage serves: "A Deployment must be associated with a Stage for
    // it to be callable over the internet" (the `Deployment` shape's own doc), and what it
    // serves is the configuration that deployment FROZE — a stage pinned to an older
    // deployment answers for the api as it stood then, not as it stands now.
    let served = stage
        .deployment_id
        .as_deref()
        .and_then(|deployment_id| api.deployments.get(deployment_id));
    let Some(deployment) = served else {
        tracing::debug!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            "the request names a stage with no deployment released to it"
        );
        return Answer::Now(Box::new(forbidden()));
    };
    let Some(route) = matching_route(&deployment.configuration.routes, addressed.method, &route_path)
    else {
        tracing::debug!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            method = %addressed.method,
            path = %route_path,
            "no route of this api matches the request"
        );
        return Answer::Now(Box::new(json_response(
            NOT_FOUND_STATUS,
            NOT_FOUND_MESSAGE,
            None,
        )));
    };
    // The route's authorization is applied BEFORE anything the route fronts: a request
    // refused here never reaches the integration below, which is what makes an authorizer's
    // gate observable. A Lambda authorizer's own decision is the one part of it that needs
    // the backend, so it travels with the dispatch below.
    let (authorizer, admitted_jwt) = match authorization(api, stage, route, req) {
        Authorization::Refused(refusal) => return Answer::Now(Box::new(refusal)),
        Authorization::Admitted(admitted_jwt) => (None, admitted_jwt),
        Authorization::DecidedByFunction(authorizer, identity) => {
            (Some((authorizer, identity)), None)
        }
    };
    // And the payload quota, ahead of the dispatch below: a body past it is the gateway's
    // own refusal, so the integration is never reached for it.
    if let Some(refusal) = payload_refusal(api, stage, route, req) {
        return Answer::Now(Box::new(refusal));
    }
    Answer::Dispatch(Box::new(dispatch(
        api,
        stage,
        &deployment.configuration,
        route,
        &route_path,
        req,
        request_id,
        authorizer,
        admitted_jwt,
    )))
}

/// The refusal the request's PAYLOAD SIZE owes it, or `None` for a payload this endpoint
/// delivers to the route's integration.
///
/// An HTTP api's endpoint accepts a request payload up to
/// [`crate::state::MAX_HTTP_API_PAYLOAD_BYTES`], a quota of the data plane that no member
/// of this model adjusts, so the bound is the same for every api, stage and route. A body
/// of exactly the quota is within it and is passed on; one byte more is refused by the
/// gateway with [`PAYLOAD_TOO_LARGE_STATUS`] AHEAD of the dispatch, which is what makes
/// the boundary observable — the integration is invoked for the first and not for the
/// second.
///
/// The size measured is the body as it arrived: the front door buffers every request, so
/// the bytes are in memory and are counted. A body that is NOT in memory is measured by
/// what the request states its length to be ([`CONTENT_LENGTH_HEADER`]); a request that
/// states nothing either is not refused on a size nothing read.
fn payload_refusal(
    api: &Api,
    stage: &Stage,
    route: &Route,
    req: &RtRequest,
) -> Option<RtResponse> {
    let payload_bytes = payload_bytes(req)?;
    if payload_bytes <= crate::state::MAX_HTTP_API_PAYLOAD_BYTES {
        return None;
    }
    tracing::info!(
        api_id = %api.api_id,
        stage = %stage.stage_name,
        route_key = %route.route_key,
        payload_bytes,
        quota = crate::state::MAX_HTTP_API_PAYLOAD_BYTES,
        "refusing an invoke whose payload is past the quota this endpoint accepts, so the \
         route's integration is not invoked"
    );
    Some(json_response(
        PAYLOAD_TOO_LARGE_STATUS,
        PAYLOAD_TOO_LARGE_MESSAGE,
        None,
    ))
}

/// How many bytes of PAYLOAD this request carries, or `None` for a request whose size nothing
/// read.
///
/// The body as it arrived is what is measured: the front door buffers every request, so the
/// bytes are in memory and are counted. A body that is NOT in memory is measured by what the
/// request states its length to be ([`CONTENT_LENGTH_HEADER`]), and a request that states
/// nothing either has no size to measure at all. ONE reading, because both readers of it have
/// to agree about what a request carried: the quota above refuses what is past the bound, and
/// the stage's own metrics report the same figure ([`request_metrics`]).
fn payload_bytes(req: &RtRequest) -> Option<usize> {
    req.body().bytes().map(<[u8]>::len).or_else(|| {
        req.headers()
            .get(CONTENT_LENGTH_HEADER)
            .and_then(|stated| stated.trim().parse::<usize>().ok())
    })
}

/// The decoration an HTTP api's stored `CorsConfiguration` puts on the answer to this
/// request, as response headers, or no headers at all for a request the configuration says
/// nothing about.
///
/// CORS is the api's OWN rule rather than the backend's: `Cors` is a member of the api
/// (`Api$CorsConfiguration`, "a CORS configuration. Supported only for HTTP APIs"), so the
/// gateway applies it to the answer it returns for that api, whatever produced that answer.
/// That is why it is computed here, from the api and the request alone, and applied to the
/// whole answer below ([`decorated`]) — an integration's response, and equally a refusal the
/// gateway itself decided.
///
/// What each member decides, from its own documentation:
///
/// * `AllowOrigins` ("a collection of allowed origins") admits the request's `Origin`, and
///   the admitted origin is what [`ALLOW_ORIGIN_HEADER`] carries — the request's own origin
///   when the collection names it, and [`crate::state::CORS_WILDCARD`] when that is what
///   the collection holds. An origin the collection does not admit gets NO CORS header at
///   all: a user agent that is told nothing refuses the read, which is the configuration's
///   answer for an origin it does not allow;
/// * `AllowMethods` ("a collection of allowed HTTP methods") decides a PREFLIGHT — the
///   [`PREFLIGHT_METHOD`] request that carries [`REQUEST_METHOD_HEADER`], the method the
///   user agent is asking about. A preflight asking about a method the collection does not
///   admit is likewise answered with no CORS header; one it admits carries the collection
///   itself in [`ALLOW_METHODS_HEADER`], beside `AllowHeaders` and `MaxAge`, which are a
///   preflight's answer alone;
/// * `ExposeHeaders` ("a collection of exposed headers") is the ACTUAL response's, since it
///   names what a script may read off that response;
/// * `AllowCredentials` ("whether credentials are included in the CORS request") carries
///   [`ALLOW_CREDENTIALS_VALUE`] on either when it is set, and nothing when it is not —
///   Fetch defines no other value for the header, and the control plane already refuses the
///   pair that would answer a credentialed request with a wildcard
///   (`crate::state::validate_cors_configuration`).
///
/// An origin is compared EXACTLY, as a method is: `AllowOrigins` is "a collection of allowed
/// origins" and nothing in the model or the recorded behaviour of a configured api admits an
/// origin the collection does not hold, so a spelling that differs in any byte — including
/// case — is an origin the configuration did not allow. A collection holding
/// [`crate::state::CORS_WILDCARD`] admits every value, and the origin header then carries the
/// wildcard rather than the request's own origin — the collection named no origin to echo.
fn cors_decoration(api: &Api, req: &RtRequest) -> Vec<(&'static str, String)> {
    let Some(cors) = api.cors_configuration.as_ref() else {
        return Vec::new();
    };
    let origin = req
        .headers()
        .get(ORIGIN_HEADER)
        .map(str::trim)
        .filter(|origin| !origin.is_empty());
    let Some(origin) = origin else {
        return Vec::new();
    };
    let Some(allowed_origin) = admitted_origin(cors.allow_origins.as_deref(), origin) else {
        return Vec::new();
    };
    // A preflight is an OPTIONS request that names the method it is asking about; an
    // OPTIONS request that names none is an ordinary request to the route. The method is
    // compared exactly, because an HTTP method is case-sensitive (RFC 9110 §9.1).
    let asked_about = (req.method() == PREFLIGHT_METHOD)
        .then(|| req.headers().get(REQUEST_METHOD_HEADER))
        .flatten()
        .map(str::trim)
        .filter(|method| !method.is_empty());
    let mut decoration = Vec::new();
    if let Some(asked_about) = asked_about {
        if !admits_method(cors.allow_methods.as_deref(), asked_about) {
            return Vec::new();
        }
        decoration.push((ALLOW_ORIGIN_HEADER, allowed_origin.to_string()));
        if let Some(methods) = joined(cors.allow_methods.as_deref()) {
            decoration.push((ALLOW_METHODS_HEADER, methods));
        }
        if let Some(headers) = joined(cors.allow_headers.as_deref()) {
            decoration.push((ALLOW_HEADERS_HEADER, headers));
        }
        if let Some(max_age) = cors.max_age {
            decoration.push((MAX_AGE_HEADER, max_age.to_string()));
        }
    } else {
        decoration.push((ALLOW_ORIGIN_HEADER, allowed_origin.to_string()));
        if let Some(exposed) = joined(cors.expose_headers.as_deref()) {
            decoration.push((EXPOSE_HEADERS_HEADER, exposed));
        }
    }
    if cors.allow_credentials == Some(true) {
        decoration.push((ALLOW_CREDENTIALS_HEADER, ALLOW_CREDENTIALS_VALUE.to_string()));
    }
    decoration
}

/// The origin [`ALLOW_ORIGIN_HEADER`] answers `origin` with, or `None` for an origin
/// `allowed` does not admit: the request's own origin when the collection names it, and the
/// wildcard when the wildcard is what admitted it.
fn admitted_origin<'r>(allowed: Option<&[String]>, origin: &'r str) -> Option<&'r str> {
    let allowed = allowed?;
    if allowed.iter().any(|entry| entry == origin) {
        return Some(origin);
    }
    allowed
        .iter()
        .any(|entry| entry == crate::state::CORS_WILDCARD)
        .then_some(crate::state::CORS_WILDCARD)
}

/// Whether `allowed` admits the method a preflight asks about — by naming it, or through
/// [`crate::state::CORS_WILDCARD`], "the entry a CORS list carries to stand for every
/// value". A collection that is absent or empty admits no method at all.
///
/// The comparison is exact, because an HTTP method is case-sensitive (RFC 9110 §9.1).
fn admits_method(allowed: Option<&[String]>, method: &str) -> bool {
    allowed.is_some_and(|allowed| {
        allowed
            .iter()
            .any(|entry| entry == crate::state::CORS_WILDCARD || entry == method)
    })
}

/// One CORS list as the single header value it is sent as, or `None` for a list with
/// nothing in it — a header naming no value says less than no header.
fn joined(values: Option<&[String]>) -> Option<String> {
    values
        .filter(|values| !values.is_empty())
        .map(|values| values.join(CORS_LIST_SEPARATOR))
}

/// `response` with `decoration`'s headers on it.
///
/// The decoration states the api's CORS rule and nothing below it sets one of these
/// headers, so each one is set here for the first time; a value the HTTP layer refuses sets
/// nothing, which fails closed.
fn decorated(mut response: RtResponse, decoration: &[(&'static str, String)]) -> RtResponse {
    let headers = response.headers_mut();
    for (name, value) in decoration {
        if headers.try_insert(*name, value.clone()).is_err() {
            // A stored value that cannot be a header value decorates nothing, which fails
            // closed; the header is named so the dropped decoration is diagnosable, as the
            // unreachable upstream in `redirect` is.
            tracing::debug!(
                header = %name,
                "the api's CORS configuration holds a value that cannot be a header value, \
                 so this header is left off the answer"
            );
        }
    }
    response
}

/// The stage of `api` that `invoke_path` names, and the path WITHIN that stage.
///
/// Two spellings reach a stage, and `Api$ApiEndpoint` carries both in one word: "the stage
/// name is TYPICALLY appended to this URI". The first path segment names the stage when the
/// api holds one of that name; otherwise the stage is the one that serves the api's root,
/// [`crate::state::DEFAULT_STAGE_NAME`] — "the stage that serves an api's root" — and the
/// whole path is the route path. A named stage therefore wins over the root stage for a
/// path that begins with its name, which is the only reading under which both remain
/// reachable.
fn stage_of<'s>(api: &'s Api, invoke_path: &str) -> Option<(&'s Stage, String)> {
    if let Some(first) = first_segment(invoke_path) {
        if let Some(stage) = api.stages.get(first) {
            let rest = invoke_path
                .trim_start_matches('/')
                .strip_prefix(first)
                .unwrap_or_default();
            return Some((stage, normalised_path(rest)));
        }
    }
    api.stages
        .get(crate::state::DEFAULT_STAGE_NAME)
        .map(|stage| (stage, normalised_path(invoke_path)))
}

/// The path the api's OWN endpoint was addressed at for this invoke, which is what a proxy
/// event reports (`rawPath`, the `1.0` event's `path`, and `requestContext`'s own `path`).
///
/// It is the stage's invoke URL path, composed from the stage and the path within it rather
/// than taken from the request as it arrived: "the stage name is TYPICALLY appended to this
/// URI to form a complete path to a deployed API stage" (`Api$ApiEndpoint`), so a NAMED
/// stage is reached with its name as the first segment and reports it, while the stage that
/// serves the api's root ([`crate::state::DEFAULT_STAGE_NAME`]) is reached with no stage
/// segment at all and so reports none — its name is a reserved word of this service, never a
/// path segment of a deployment.
///
/// That is what keeps the event's path the one AWS reports for the same route: a client on
/// this machine cannot resolve the api's endpoint, so it carries the api id, the local
/// marker or the root stage's own name in the PATH instead ([`resolve`]), and none of those
/// is part of the path the api was addressed at.
fn endpoint_path(stage: &Stage, route_path: &str) -> String {
    if stage.stage_name == crate::state::DEFAULT_STAGE_NAME {
        return normalised_path(route_path);
    }
    normalised_path(&format!("{}/{route_path}", stage.stage_name))
}

/// `path` with exactly one leading `/` and no trailing one (except for the root itself), so
/// that the same route path is compared however a client spelled it.
fn normalised_path(path: &str) -> String {
    let segments = path_segments(path);
    if segments.is_empty() {
        return "/".to_string();
    }
    format!("/{}", segments.join("/"))
}

/// The route of `routes` that serves `method path`, or `None` when none of them does.
///
/// `routes` are the routes of the DEPLOYMENT the stage serves (frozen when it was
/// released), not the api's current ones: that is what makes a stage pinned to an older
/// deployment keep answering for the configuration it froze.
///
/// The rule is API Gateway's own: a request is served by the MOST SPECIFIC matching route
/// key, and `$default` catches what nothing else matches. Specificity is read off the key
/// itself, segment by segment — a literal segment is more specific than a `{variable}`,
/// which is more specific than the greedy `{proxy+}` that stands for the remainder of the
/// path — and compared from the left, so `GET /orders/{id}` beats `GET /{resource}/{id}`
/// for the same request, and a key naming the request's verb beats the `ANY` catch-all for
/// the same path.
///
/// The work is bounded: one scan of the deployed routes, capped at
/// [`crate::state::MAX_ROUTES_PER_API`] by the create that stored them, and each key's
/// segments are compared once against the request's.
fn matching_route<'s>(
    routes: &'s BTreeMap<String, Route>,
    method: &str,
    path: &str,
) -> Option<&'s Route> {
    let requested = path_segments(path);
    let best = routes
        .values()
        .filter(|route| route.route_key != DEFAULT_ROUTE_KEY)
        .filter_map(|route| specificity(&route.route_key, method, &requested).map(|rank| (rank, route)))
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, route)| route);
    best.or_else(|| {
        routes
            .values()
            .find(|route| route.route_key == DEFAULT_ROUTE_KEY)
    })
}

/// How specifically `route_key` matches `method` and the request's path `segments`, or
/// `None` when it does not match at all.
///
/// The rank compares as a tuple: the per-segment scores first (compared from the left, a
/// literal above a variable above the greedy remainder), then whether the key named the
/// request's verb rather than `ANY`. A longer run of literal matches therefore outranks a
/// shorter one, and the greedy key ranks below every key that matched segment for segment.
fn specificity(route_key: &str, method: &str, segments: &[&str]) -> Option<(Vec<u8>, u8)> {
    let (key_method, key_path) = route_key.split_once(' ')?;
    let exact_method = if key_method.eq_ignore_ascii_case(method) {
        1
    } else if key_method.eq_ignore_ascii_case(ANY_METHOD) {
        0
    } else {
        return None;
    };
    let mut scores = Vec::new();
    let pattern = path_segments(key_path);
    let mut requested = segments.iter();
    for (position, expected) in pattern.iter().enumerate() {
        match path_variable(expected) {
            // The greedy remainder: it stands for one or more segments, so it matches only
            // when at least one is left, and it ends the comparison.
            Some(name) if name.ends_with(GREEDY_VARIABLE_SUFFIX) => {
                if position + 1 != pattern.len() || requested.next().is_none() {
                    return None;
                }
                scores.push(0);
                return Some((scores, exact_method));
            }
            // A single-segment variable matches one non-empty segment, whatever it says.
            Some(_) => {
                requested.next()?;
                scores.push(1);
            }
            None => {
                let actual = requested.next()?;
                if actual != expected {
                    return None;
                }
                scores.push(2);
            }
        }
    }
    // Every pattern segment is spoken for; a request carrying MORE segments than the key
    // is not this key's to serve.
    requested.next().is_none().then_some((scores, exact_method))
}

/// The name a route-key path segment declares when it is a path variable (`{id}` names
/// `id`, the greedy `{proxy+}` names `proxy+`), or `None` for a literal segment.
fn path_variable(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

/// What the route's `AuthorizationType` decided about this request.
enum Authorization {
    /// The request may proceed to the integration, with what the route's authorizer reports
    /// about the caller it admitted — the claims and scopes of an accepted JWT
    /// ([`super::jwt::Accepted`]), and nothing for a route whose type identifies no caller.
    Admitted(Option<jwt::Accepted>),
    /// The request is refused, with the answer the caller receives.
    Refused(RtResponse),
    /// The decision is the authorizer FUNCTION's, which is made once the store's guard is
    /// released ([`request_authorizer`]). Carries the authorizer and the identity-source
    /// values its decision is about and is cached under.
    DecidedByFunction(Authorizer, Vec<String>),
}

/// What the route's `AuthorizationType` owes this request: the refusal it earns, admission
/// to the integration, or the decision its authorizer's function still has to make.
///
/// `NONE` admits every request — "NONE for open access" (`Route$AuthorizationType`).
///
/// `AWS_IAM` needs a SigV4 credential, whose SIGNATURE the emulator itself verifies (a
/// service never checks signatures), so only its presence is read here; a request carrying
/// none is `403` `{"message":"Forbidden"}`. What this does NOT do, stated so a passing test
/// is not read as more than it is: no IAM authorization DECISION is made. AWS evaluates
/// `execute-api:Invoke` for the resolved principal, and this emulator models no caller
/// identity and no policy evaluation, so the refusal an authenticated but unprivileged
/// principal earns is not one it can produce.
///
/// `JWT` is decided in full from the api's own records: the token the authorizer's identity
/// source names is evaluated against that authorizer's `JWTConfiguration` and the route's
/// `AuthorizationScopes` ([`super::jwt::evaluate`]). A token the gate cannot accept is
/// `401` `{"message":"Unauthorized"}` and the integration is NOT reached; a token it
/// accepts whose scopes do not meet the route's is `403` — "otherwise, the invocation is
/// not authorized" (`Route$AuthorizationScopes`).
///
/// `CUSTOM` names a Lambda authorizer, whose decision is that function's answer. The part
/// that is settled BEFORE the function is called is applied here: every identity source must
/// be "present in the request, not null, and non-empty", and "only when this is true does
/// the authorizer invoke the authorizer Lambda function. Otherwise, it returns a 401
/// Unauthorized response without calling the Lambda function"
/// (`Authorizer$IdentitySource`). Past that precondition the decision is the function's, so
/// it travels with the dispatch and is obtained once the store's guard is released
/// ([`request_authorizer`]).
///
/// A route whose authorizer-backed type names no authorizer of this api cannot be
/// authorized at all, which is a fault of the api's configuration and not of the caller:
/// `500`. The writers of a route refuse that association
/// ([`crate::state::UNRESOLVED_AUTHORIZER_MESSAGE`]), so this is a store that lost the
/// authorizer some other way and not a route a caller can create.
fn authorization(
    api: &Api,
    stage: &Stage,
    route: &Route,
    req: &RtRequest,
) -> Authorization {
    let authorization_type = route.authorization_type.as_str();
    if authorization_type == AUTHORIZATION_NONE {
        return Authorization::Admitted(None);
    }
    if authorization_type == AUTHORIZATION_AWS_IAM {
        if carries_sigv4_credential(req) {
            return Authorization::Admitted(None);
        }
        tracing::info!(
            api_id = %api.api_id,
            stage = %stage.stage_name,
            route_key = %route.route_key,
            "refusing an invoke of an AWS_IAM route that presented no credential"
        );
        return Authorization::Refused(forbidden());
    }
    let Some(authorizer) = route
        .authorizer_id
        .as_deref()
        .and_then(|authorizer_id| api.authorizers.get(authorizer_id))
    else {
        tracing::warn!(
            api_id = %api.api_id,
            route_key = %route.route_key,
            authorization_type = %authorization_type,
            authorizer_id = ?route.authorizer_id,
            "the matched route is authorized by an authorizer this api does not hold"
        );
        return Authorization::Refused(json_response(
            UNPLACEABLE_STATUS,
            INTERNAL_MESSAGE,
            None,
        ));
    };
    let query_parameters = query_parameters(req.uri());
    let header = |name: &str| req.headers().get(name).map(str::to_string);
    if authorization_type != AUTHORIZATION_JWT {
        // A `CUSTOM` (Lambda) authorizer: the identity-source precondition, and then the
        // function's own decision. See this function's docs.
        let absent: Vec<&str> = authorizer
            .identity_source
            .iter()
            .filter(|identity_source| {
                !presents_identity_source(
                    identity_source,
                    header,
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
                "refusing an invoke: an identity source of the route's authorizer is \
                 absent, so the authorizer is not invoked"
            );
            return Authorization::Refused(json_response(
                UNAUTHORIZED_STATUS,
                UNAUTHORIZED_MESSAGE,
                None,
            ));
        }
        // Every identity source is there, so the authorizer's function decides: the
        // identity-source VALUES are what the decision is about and what it is cached
        // under ("API Gateway uses the identity sources as the cache key"), so they travel
        // with the dispatch.
        let identity = authorizer
            .identity_source
            .iter()
            .map(|identity_source| {
                presented_identity(
                    identity_source,
                    header,
                    &query_parameters,
                    &stage.stage_variables,
                )
            })
            .collect();
        return Authorization::DecidedByFunction(authorizer.clone(), identity);
    }
    let Some(configuration) = authorizer.jwt_configuration.as_ref() else {
        tracing::warn!(
            api_id = %api.api_id,
            route_key = %route.route_key,
            authorizer_id = %authorizer.authorizer_id,
            "the route's JWT authorizer holds no JwtConfiguration to evaluate a token against"
        );
        return Authorization::Refused(json_response(
            UNPLACEABLE_STATUS,
            INTERNAL_MESSAGE,
            None,
        ));
    };
    let audience = configuration.audience.as_deref().unwrap_or_default();
    let identity_source = authorizer
        .identity_source
        .iter()
        .map(String::as_str)
        .find(|source| !source.trim().is_empty())
        .unwrap_or(DEFAULT_JWT_IDENTITY_SOURCE);
    let presented = identity_source_value(identity_source, header, &query_parameters);
    let decision = jwt::evaluate(
        presented.as_deref(),
        jwt::Expectations {
            issuer: configuration.issuer.as_deref().unwrap_or_default(),
            audience,
            scopes: &route.authorization_scopes,
        },
        now_unix_seconds(),
    );
    match decision {
        // The token the gate accepted is what the integration is told about the caller:
        // its claims and scopes travel with the dispatch ([`super::jwt::accepted`]).
        jwt::Decision::Allow => {
            Authorization::Admitted(Some(jwt::accepted(presented.as_deref())))
        }
        jwt::Decision::Unauthorized(reason) => {
            tracing::info!(
                api_id = %api.api_id,
                stage = %stage.stage_name,
                route_key = %route.route_key,
                authorizer_id = %authorizer.authorizer_id,
                reason = ?reason,
                presented = ?jwt::described(presented.as_deref()),
                "refusing an invoke: the JWT authorizer did not accept the presented token"
            );
            Authorization::Refused(json_response(
                UNAUTHORIZED_STATUS,
                UNAUTHORIZED_MESSAGE,
                None,
            ))
        }
        jwt::Decision::InsufficientScope => {
            tracing::info!(
                api_id = %api.api_id,
                stage = %stage.stage_name,
                route_key = %route.route_key,
                authorizer_id = %authorizer.authorizer_id,
                scopes = ?route.authorization_scopes,
                "refusing an invoke: the presented token claims none of the route's \
                 authorization scopes"
            );
            Authorization::Refused(forbidden())
        }
    }
}

/// The VALUE the request presents for one identity source, as the cache key and the event's
/// `identitySource` carry it.
///
/// The three request-bound spellings are read — a header, a query string parameter and a
/// stage variable, the parts `Authorizer$IdentitySource` names — and anything else presents
/// the empty value, which is what the precondition above already admitted it as (a context
/// parameter, which every call has).
fn presented_identity(
    identity_source: &str,
    header: impl Fn(&str) -> Option<String>,
    query_parameters: &BTreeMap<String, String>,
    stage_variables: &BTreeMap<String, String>,
) -> String {
    if let Some(value) = identity_source_value(identity_source, header, query_parameters) {
        return value;
    }
    super::data_plane::parameter_name(
        identity_source.trim(),
        super::data_plane::STAGE_VARIABLE_IDENTITY_SOURCES,
    )
    .and_then(|name| stage_variables.get(name).cloned())
    .unwrap_or_default()
}

/// Everything the route's integration needs, read out of the store while it is locked: the
/// request as the backend will see it, the facts of this invocation, the integration the
/// route targets, and the authorizer whose decision is still owed.
///
/// A route that targets no integration, or one whose target names none the deployment
/// froze, carries `None`: a route with nothing behind it cannot answer at all, which
/// [`dispatch::perform`] reports as the "produced no response" class.
#[allow(clippy::too_many_arguments)]
fn dispatch(
    api: &Api,
    stage: &Stage,
    deployed: &DeployedConfiguration,
    route: &Route,
    route_path: &str,
    req: &RtRequest,
    request_id: &str,
    authorizer: Option<(Authorizer, Vec<String>)>,
    admitted_jwt: Option<jwt::Accepted>,
) -> dispatch::Dispatch {
    let integration: Option<Integration> = route
        .target
        .as_deref()
        .and_then(crate::state::target_integration_id)
        .and_then(|integration_id| deployed.integrations.get(integration_id))
        .cloned();
    let region = api_region(api);
    dispatch::Dispatch {
        request: proxy_event::ProxyRequest {
            method: req.method().to_string(),
            // The path AWS's own event reports is the one the api's OWN endpoint was
            // addressed at ([`endpoint_path`]), whichever addressing a local client reached
            // this emulator by.
            raw_path: endpoint_path(stage, route_path),
            route_path: route_path.to_string(),
            raw_query_string: query_of(req.uri()).unwrap_or_default().to_string(),
            headers: request_headers(req),
            query_parameters: joined_query_parameters(req.uri()),
            body: req
                .body()
                .bytes()
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
            route_key: route.route_key.clone(),
            api_id: api.api_id.clone(),
            stage_name: stage.stage_name.clone(),
            stage_variables: stage.stage_variables.clone(),
            region: region.clone(),
            // The host an event reports is the api's OWN endpoint, the one
            // `Api$ApiEndpoint` announced, rather than the address a local client had to
            // reach this emulator at: that is the domain the request was made to as far as
            // the api is concerned.
            domain_name: endpoint_host(api),
        },
        invocation: proxy_event::Invocation {
            // The id the whole invoke is reported under, minted by the seam that serves it
            // so the event's `$context.requestId`, the response's own header and the
            // stage's access-log record name ONE request.
            account_id: ::engine::emulator_account_id().to_string(),
            request_id: request_id.to_string(),
            received: SystemTime::now(),
            authorizer_context: None,
            authorizer_principal: None,
            admitted_jwt,
        },
        integration,
        authorizer,
        throttle: route_throttle(stage, &route.route_key),
        route_key: route.route_key.clone(),
    }
}

/// The throttle the serving stage meters this route with: the `RouteSettings` it states for
/// that route key, and otherwise its `DefaultRouteSettings` — "the default route settings
/// for the stage", which is what a route the stage states none of its own for is metered by.
/// `None` for a route neither states a limit for, which is not metered.
fn route_throttle(stage: &Stage, route_key: &str) -> Option<crate::state::route_throttle::Limits> {
    let settings = stage
        .route_settings
        .get(route_key)
        .or(stage.default_route_settings.as_ref())?;
    crate::state::route_throttle::limits_of(
        settings.throttling_rate_limit,
        settings.throttling_burst_limit,
    )
}

/// Every header of the request, by lowercased name, with a header that appeared more than
/// once joined into the one comma-separated value a field list is (RFC 9110 §5.3).
fn request_headers(req: &RtRequest) -> BTreeMap<String, String> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in req.headers().iter() {
        let name = name.to_ascii_lowercase();
        headers
            .entry(name)
            .and_modify(|held| {
                held.push(',');
                held.push_str(value);
            })
            .or_insert_with(|| value.to_string());
    }
    headers
}

/// The host of the api's OWN default endpoint — `{api-id}.execute-api.{region}.amazonaws.com`
/// ([`crate::state::execute_api_host`]) — which is the `domainName` an event reports and
/// the first label of which is its `domainPrefix`.
///
/// Composed from the api's stored id and Region rather than from the endpoint it ANNOUNCED:
/// what this emulator announces is the address it serves the api at, which for a client
/// that reached the emulator's own endpoint carries no api host at all
/// ([`crate::state::announced_execute_api_endpoint`]), while the domain the request was made
/// to as far as the api is concerned is the api's own either way.
fn endpoint_host(api: &Api) -> String {
    crate::state::execute_api_host(&api.api_id, &api.region)
}

/// The Region this api lives in, as the record stores it ([`crate::state::Api::region`]) —
/// the Region whose Lambda endpoint an `AWS_PROXY` integration's invocation is addressed to
/// when the function's own ARN names none, and the Region the `execute-api` resource ARN an
/// `AWS_IAM` route is authorized against carries.
fn api_region(api: &Api) -> String {
    api.region.clone()
}

/// `307 Temporary Redirect` to `location`, with no body of its own: the answer for an
/// upstream the CALLER can reach and this emulator does not connect to.
fn redirect(location: &str) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(TEMPORARY_REDIRECT_STATUS).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_STATUS).expect("500 is a valid status code"),
        ),
        SdkBody::empty(),
    );
    let headers = response.headers_mut();
    if headers.try_insert(LOCATION_HEADER, location.to_string()).is_err() {
        // A stored `IntegrationUri` that cannot be a header value names no reachable
        // upstream, so there is nowhere to send the caller.
        return json_response(UNPLACEABLE_STATUS, INTERNAL_MESSAGE, None);
    }
    let _ = headers.try_insert("content-length", "0");
    response
}

/// Now, as whole seconds since the epoch — the `NumericDate` a token's `exp` and `nbf` are
/// compared against (RFC 7519 §2). A clock before the epoch reads as the epoch, so the
/// comparison is total.
fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns that a wire test cannot vary cheaply: which route
    //! key matches a request, and how an invoke path is split into a stage and a route
    //! path. The served behaviour is pinned end to end in
    //! `tests/all/http_api_jwt_data_plane.rs`.

    use super::*;

    /// The rank of one key against one request, as [`specificity`] reads it.
    fn rank(route_key: &str, method: &str, path: &str) -> Option<(Vec<u8>, u8)> {
        let segments = path_segments(path);
        specificity(route_key, method, &segments)
    }

    #[test]
    fn a_route_key_matches_only_the_requests_it_names() {
        let cases: &[(&str, &str, &str, &str, bool)] = &[
            ("an exact method and path", "GET /orders", "GET", "/orders", true),
            ("a trailing slash on the request", "GET /orders", "GET", "/orders/", true),
            ("another method", "GET /orders", "POST", "/orders", false),
            ("another path", "GET /orders", "GET", "/invoices", false),
            ("a deeper path", "GET /orders", "GET", "/orders/1", false),
            ("the ANY catch-all", "ANY /orders", "DELETE", "/orders", true),
            ("a single-segment variable", "GET /orders/{id}", "GET", "/orders/7", true),
            (
                "a single-segment variable against two segments",
                "GET /orders/{id}",
                "GET",
                "/orders/7/items",
                false,
            ),
            (
                "a single-segment variable against none",
                "GET /orders/{id}",
                "GET",
                "/orders",
                false,
            ),
            ("the greedy remainder", "ANY /{proxy+}", "GET", "/orders/7/items", true),
            ("the greedy remainder over one segment", "ANY /{proxy+}", "GET", "/x", true),
            (
                // Greedy stands for one or more, so the api root is not it.
                "the greedy remainder over no segment",
                "ANY /{proxy+}",
                "GET",
                "/",
                false,
            ),
            ("a literal under a greedy prefix", "GET /files/{path+}", "GET", "/files/a/b", true),
            ("a key naming no path at all", "$connect", "GET", "/", false),
        ];
        for (case, route_key, method, path, matches) in cases {
            let actual = rank(route_key, method, path).is_some();
            assert_eq!(
                actual, *matches,
                "case {case:?}: expected {route_key:?} to match {method} {path}: {matches}"
            );
        }
    }

    #[test]
    fn the_most_specific_matching_key_outranks_the_others() {
        // Each pair: the key that must win, and the key it must beat, for one request.
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            (
                "a literal beats a variable at the same position",
                "GET /orders/new",
                "GET /orders/{id}",
                "GET",
                "/orders/new",
            ),
            (
                "a leftmost literal decides",
                "GET /orders/{id}",
                "GET /{resource}/{id}",
                "GET",
                "/orders/7",
            ),
            (
                "a segment-for-segment match beats the greedy remainder",
                "GET /orders/{id}",
                "ANY /{proxy+}",
                "GET",
                "/orders/7",
            ),
            (
                "the request's own verb beats the ANY catch-all",
                "GET /orders",
                "ANY /orders",
                "GET",
                "/orders",
            ),
        ];
        for (case, winner, loser, method, path) in cases {
            let winning = rank(winner, method, path)
                .unwrap_or_else(|| panic!("case {case:?}: {winner:?} must match {method} {path}"));
            let losing = rank(loser, method, path)
                .unwrap_or_else(|| panic!("case {case:?}: {loser:?} must match {method} {path}"));
            assert!(
                winning > losing,
                "case {case:?}: expected {winner:?} ({winning:?}) to outrank {loser:?} ({losing:?})"
            );
        }
    }

    #[test]
    fn an_invoke_path_keeps_one_leading_slash_and_no_trailing_one() {
        assert_eq!(normalised_path(""), "/");
        assert_eq!(normalised_path("/"), "/");
        assert_eq!(normalised_path("orders"), "/orders");
        assert_eq!(normalised_path("/orders/"), "/orders");
        assert_eq!(normalised_path("//orders//7/"), "/orders/7");
    }

    #[test]
    fn a_path_is_split_into_its_first_segment_and_the_rest() {
        assert_eq!(split_first_segment("/abc/prod/x"), Some(("abc", "/prod/x")));
        assert_eq!(split_first_segment("/abc"), Some(("abc", "/")));
        assert_eq!(split_first_segment("abc/x"), Some(("abc", "/x")));
        assert_eq!(split_first_segment("/"), None);
        assert_eq!(split_first_segment(""), None);
    }
}
