// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's `GRAPHQL` endpoint — `GraphqlApi$uris`'s `GRAPHQL` entry — and the
//! GraphQL request it serves.
//!
//! What it serves, over HTTP/1.1 on the API's own socket:
//!
//! > `POST /graphql` with `{"query": …, "variables": …, "operationName": …}` executes that
//! > document against the API's own resolvers ([`super::execution`]) and answers the GraphQL
//! > result: `data` for what resolved, and `errors` for what did not.
//!
//! Two decisions are made before any of the document is read, in this order, because that is
//! the order a request meets them on the way to an AppSync API:
//!
//! 1. The WEB ACL in front of the API, if WAF was asked to put one there. `AssociateWebACL`
//!    binds a web ACL to "an AppSync GraphQL API" by the API's own ARN, and the decision is
//!    WAFv2's own ([`::service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`])
//!    — this module holds no second reading of a rule tree. A terminating action is answered
//!    HERE and nothing of the API is reached: a `Block` with no `CustomResponse` is the
//!    `403` AppSync's own edge answers, carrying the one error AppSync words it with
//!    ([`WAF_FORBIDDEN_ERROR_TYPE`], [`WAF_FORBIDDEN_MESSAGE`]).
//! 2. The AUTHORIZATION the request presents, by the rule AppSync applies to a call on a
//!    GraphQL API ([`super::authorized`]): the same rule the API's real-time endpoint
//!    applies to a connection, over the same members.
//!
//! The transport-safety decisions the core makes for its own HTTP listeners are applied here
//! from the core's own predicates — the `Host` allowlist that guards against DNS rebinding,
//! the WebSocket refusal (the real-time endpoint is the other socket), and the browser
//! (`Origin` / `Sec-Fetch-Site`) check — and the request is read under one deadline and one
//! body ceiling, both the core's, so a slow or oversized client cannot hold a connection
//! permit open.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::Arc;
use ::std::time::Duration;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::serde_json::{Map as JsonMap, Value as Json};
use ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge as waf_edge;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::execution::{self, GraphqlRequest};
use super::PlaneContext;
use crate::service::event_api::{self, PresentedAuth};

/// The path the GraphQL endpoint serves its API at — the path `uris`'s `GRAPHQL` entry
/// carries, which is the same `/graphql` the real-time endpoint is addressed at: the two are
/// distinct endpoints by ADDRESS, not by path.
pub const GRAPHQL_PATH: &str = "/graphql";

/// The `errorType` and the sentence AppSync's edge answers a request a web ACL BLOCKED
/// with: the fixed envelope of a WAF-forbidden GraphQL request, which names nothing of the
/// request or of the rule that blocked it.
pub const WAF_FORBIDDEN_ERROR_TYPE: &str = "WAFForbiddenException";
pub const WAF_FORBIDDEN_MESSAGE: &str = "403 Forbidden";

/// The status that refusal carries, which is also the status WAF serves a `Block` under
/// (`waf_edge::EdgeAction::default_status`).
const FORBIDDEN_STATUS: u16 = 403;

/// Serves one GraphQL API's `GRAPHQL` endpoint: the socket that endpoint's published address
/// names.
pub struct QuerySocket {
    /// What every request on this socket is answered from.
    context: PlaneContext,
    /// The core's TLS server, for a client that dialled `https://`.
    upgrader: Option<TlsUpgrader>,
    /// How many requests this endpoint reads at once, which is what bounds the memory the
    /// bodies in flight on it hold: the core's own HTTP in-flight cap, one permit per
    /// request, taken before a body is read and released when the answer is written.
    in_flight: Arc<Semaphore>,
}

impl QuerySocket {
    /// The endpoint of one API.
    pub fn new(context: PlaneContext, upgrader: Option<TlsUpgrader>) -> Self {
        Self {
            context,
            upgrader,
            in_flight: Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT)),
        }
    }
}

impl NativeHandler for QuerySocket {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let context = self.context.clone();
        let upgrader = self.upgrader.clone();
        let in_flight = Arc::clone(&self.in_flight);
        Box::pin(async move {
            let Some(conn) = event_api::negotiated(conn, upgrader).await else {
                return;
            };
            serve_connection(context, in_flight, conn).await;
        })
    }
}

/// Speak HTTP/1.1 on one accepted connection until the client closes it.
async fn serve_connection(context: PlaneContext, in_flight: Arc<Semaphore>, conn: ListenerConn) {
    let api_id = context.api_id.clone();
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request| {
        let context = context.clone();
        let in_flight = Arc::clone(&in_flight);
        async move {
            // Every request's whole answer is driven through the core's panic containment,
            // because this socket is served by this module rather than by the front door's
            // router: there is no middleware layer to carry it.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                contained(facts, answer(context, in_flight, request)).await,
            )
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(request_deadline())
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this endpoint could not frame,
        // ends here and takes nothing else with it.
        ::tracing::debug!(
            %api_id,
            %error,
            "a connection to a GraphQL API's endpoint ended before its response"
        );
    }
}

/// Drive one request's whole answer under the core's own PANIC CONTAINMENT
/// (`emulator_server::contain_panics`), and word a contained panic in this endpoint's own
/// envelope — for the reason the Event API's HTTP endpoint does it
/// ([`crate::service::event_api::publish`] states it in full).
async fn contained<F>(
    facts: ::emulator_server::RequestFacts,
    answer: F,
) -> ::http::Response<Full<Bytes>>
where
    F: Future<Output = ::http::Response<Full<Bytes>>> + Send + 'static,
{
    let guarded: ::http::Response<Bytes> = ::emulator_server::contain_panics(facts, async move {
        let (parts, body) = answer.await.into_parts();
        let bytes = body
            .collect()
            .await
            .map_or_else(|_| Bytes::new(), |collected| collected.to_bytes());
        ::http::Response::from_parts(parts, bytes)
    })
    .await;
    let (parts, bytes) = guarded.into_parts();
    if parts.status == ::http::StatusCode::INTERNAL_SERVER_ERROR {
        return error_response(
            500,
            event_api::publish::INTERNAL_FAILURE_ERROR_TYPE,
            event_api::publish::INTERNAL_FAILURE_MESSAGE,
        );
    }
    ::http::Response::from_parts(parts, Full::new(bytes))
}

/// Answer one request to the GraphQL endpoint.
async fn answer(
    context: PlaneContext,
    in_flight: Arc<Semaphore>,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_dispatch(&parts) {
        return refused;
    }
    // One permit per request IN FLIGHT, taken before a byte of the body is read: a request
    // that finds none is shed, which is what the core's own HTTP listener answers such a
    // request with.
    let Ok(permit) = Arc::clone(&in_flight).try_acquire_owned() else {
        return error_response(503, "ServiceUnavailableException", SERVER_BUSY);
    };
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and the execution, and released with them.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return error_response(413, BAD_REQUEST_ERROR_TYPE, BODY_TOO_LARGE);
        };
        served_request(&context, &parts, &collected.to_bytes()).await
    })
    .await;
    served.unwrap_or_else(|_| error_response(408, BAD_REQUEST_ERROR_TYPE, REQUEST_TIMED_OUT))
}

/// Answer one framed request: the GraphQL route, and the refusals every other request to
/// this endpoint meets.
async fn served_request(
    context: &PlaneContext,
    parts: &::http::request::Parts,
    body: &[u8],
) -> ::http::Response<Full<Bytes>> {
    if parts.uri.path() != GRAPHQL_PATH {
        return error_response(404, NOT_FOUND_ERROR_TYPE, NO_SUCH_ROUTE);
    }
    if parts.method != ::http::Method::POST {
        return error_response(405, BAD_REQUEST_ERROR_TYPE, A_QUERY_IS_A_POST);
    }
    // The API's own ARN, which is what a web ACL is associated with and what says the API
    // still exists at all.
    let Some(arn) = context
        .state
        .lock()
        .graphql_apis
        .get(&context.api_id)
        .map(|api| api.arn.clone())
    else {
        return error_response(
            404,
            NOT_FOUND_ERROR_TYPE,
            crate::state::API_NOT_FOUND_MESSAGE,
        );
    };
    if let Some(refused) = refused_by_web_acl(context, &arn, parts, body) {
        return refused;
    }
    let auth = PresentedAuth::from_fields(&parts.headers);
    {
        let store = context.state.lock();
        if !super::authorized(&store, &context.api_id, &auth) {
            return error_response(
                401,
                event_api::UNAUTHORIZED_ERROR_TYPE,
                event_api::UNAUTHORIZED_MESSAGE,
            );
        }
    }
    let Ok(document) = ::serde_json::from_slice::<Json>(body) else {
        return error_response(400, BAD_REQUEST_ERROR_TYPE, BODY_IS_A_DOCUMENT);
    };
    let Some(query) = document.get(QUERY_MEMBER).and_then(Json::as_str) else {
        return error_response(400, BAD_REQUEST_ERROR_TYPE, BODY_CARRIES_A_QUERY);
    };
    let request = GraphqlRequest {
        query: query.to_string(),
        operation_name: document
            .get(OPERATION_NAME_MEMBER)
            .and_then(Json::as_str)
            .map(str::to_string),
        variables: document
            .get(VARIABLES_MEMBER)
            .and_then(Json::as_object)
            .cloned()
            .unwrap_or_default(),
        headers: request_fields(&parts.headers),
    };
    let answered = execution::execute(context, &request).await;
    json_response(200, &answered.document())
}

/// The request's own fields as a resolver reads them (`ctx.request.headers`): one member per
/// field, under the lowercase name HTTP gives it.
fn request_fields(headers: &::http::HeaderMap) -> JsonMap<String, Json> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), Json::String(value.to_string())))
        })
        .collect()
}

/// What the web ACL in front of this API decided about this request, or `None` when it let
/// the request through to the API — which is also what a request with no web ACL in front of
/// it meets.
///
/// The decision is WAFv2's own: this hands it the request as that service inspects one
/// ([`waf_edge::InspectedRequest`]) and renders the answer it gets back. The two stores are
/// locked one after the other and never at the same time, and the wafv2 side locks nothing
/// of this service's, so the two cannot deadlock.
fn refused_by_web_acl(
    context: &PlaneContext,
    arn: &str,
    parts: &::http::request::Parts,
    body: &[u8],
) -> Option<::http::Response<Full<Bytes>>> {
    let decision = waf_edge::decide(&*context.engine, arn, &inspected_request(parts, body))?;
    if decision.action.is_allow() {
        return None;
    }
    ::tracing::info!(
        api_id = %context.api_id,
        rule = decision.rule_name.as_deref().unwrap_or("<default action>"),
        "the web acl in front of this API does not let the request through"
    );
    // A terminating action that configures its own `CustomResponse` is served BY WAF: the
    // status, the headers and the body are the web ACL's own, and this endpoint's envelope
    // is not used, because AppSync never produced the response.
    if let Some(custom) = decision.custom_response {
        return Some(edge_response(custom));
    }
    // A `Block` with no custom response is the refusal AppSync's own edge words. A `Captcha`
    // or `Challenge` with none is answered with the status WAF serves its interstitial
    // under; the interstitial ITSELF is a script WAF serves from its own endpoint, which
    // this emulator serves none of, so the answer carries the status and the same envelope
    // rather than a page nothing produced.
    Some(error_response(
        decision.action.default_status().unwrap_or(FORBIDDEN_STATUS),
        WAF_FORBIDDEN_ERROR_TYPE,
        WAF_FORBIDDEN_MESSAGE,
    ))
}

/// The request as WAFv2 inspects it: every component a `FieldToMatch` can name.
///
/// Read by WAFv2's own reading of an inbound request ([`waf_edge::InspectedRequest::of_parts`])
/// — including the address it came from — so this endpoint and every other protected
/// resource's own service cannot differ on what a `FieldToMatch` names. The path and the
/// query string are the ones the request arrived with, because this socket IS the API's
/// endpoint: a client addressing AWS sends the same `/graphql` to the API's own hostname, so
/// there is no emulator addressing prefix to strip.
fn inspected_request(parts: &::http::request::Parts, body: &[u8]) -> waf_edge::InspectedRequest {
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect();
    waf_edge::InspectedRequest::of_parts(
        parts.method.as_str(),
        parts.uri.path(),
        parts.uri.query().unwrap_or_default(),
        headers,
        body.to_vec(),
    )
}

/// The response a web ACL's own `CustomResponse` serves, as it leaves the edge.
fn edge_response(custom: waf_edge::EdgeResponse) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(custom.body);
    let mut response = ::http::Response::new(Full::new(body.clone()));
    *response.status_mut() = ::http::StatusCode::from_u16(custom.status)
        .unwrap_or(::http::StatusCode::FORBIDDEN);
    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(content_type) = custom.content_type {
        fields.push((::http::header::CONTENT_TYPE.to_string(), content_type));
    }
    fields.extend(custom.headers);
    // The length is written LAST, so a configured header cannot leave the answer stating a
    // length that is not the body's.
    fields.push((
        ::http::header::CONTENT_LENGTH.to_string(),
        body.len().to_string(),
    ));
    for (name, value) in fields {
        match (
            ::http::HeaderName::from_bytes(name.as_bytes()),
            ::http::HeaderValue::from_str(&value),
        ) {
            (Ok(name), Ok(value)) => {
                response.headers_mut().insert(name, value);
            }
            _ => ::tracing::warn!(
                %name,
                "the web acl's custom response names a header a response cannot carry, and \
                 it is left off the answer"
            ),
        }
    }
    response
}

/// How long one request on this endpoint may take, from its first header byte to the
/// answer: the core's own HTTP request deadline.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How many bytes of one request this endpoint reads: the bound this service states for a
/// message on a data-plane endpoint (`crate::service::event_api::MAX_MESSAGE_BYTES`), which
/// is well under the core's own per-connection ceiling.
fn body_ceiling() -> usize {
    event_api::MAX_MESSAGE_BYTES
}

/// The transport-safety refusals the core applies to its own HTTP listeners, applied here
/// from the core's own predicates so this endpoint and a core listener decide them alike.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
) -> Option<::http::Response<Full<Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());
    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(error_response(
            421,
            BAD_REQUEST_ERROR_TYPE,
            event_api::HOST_REQUIRED,
        ));
    };
    if !event_api::host_allowed(host) {
        return Some(error_response(
            421,
            BAD_REQUEST_ERROR_TYPE,
            event_api::HOST_NOT_ALLOWED,
        ));
    }
    if ::emulator_server::is_websocket_upgrade(
        header("connection"),
        header("upgrade"),
        header(":protocol"),
    ) {
        return Some(error_response(400, BAD_REQUEST_ERROR_TYPE, NO_WEBSOCKETS));
    }
    if let Some(reason) = event_api::cross_site_browser(header("origin"), header("sec-fetch-site"))
    {
        return Some(error_response(
            403,
            event_api::UNAUTHORIZED_ERROR_TYPE,
            &reason,
        ));
    }
    None
}

/// One answer carrying a document.
fn json_response(status: u16, document: &Json) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(document.to_string());
    ::http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a bare
        // status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

/// One refusal, in the envelope AppSync answers a data-plane error in: a list of errors,
/// each naming its type and stating its sentence — and no `data`, because nothing of the
/// API was reached.
fn error_response(
    status: u16,
    error_type: &str,
    message: &str,
) -> ::http::Response<Full<Bytes>> {
    json_response(
        status,
        &event_api::publish::errors_document(error_type, message),
    )
}

/// The members a GraphQL request carries.
const QUERY_MEMBER: &str = "query";
const VARIABLES_MEMBER: &str = "variables";
const OPERATION_NAME_MEMBER: &str = "operationName";

/// The `errorType` values this endpoint's refusals carry, as AppSync's own errors name them.
const BAD_REQUEST_ERROR_TYPE: &str = "BadRequestException";
const NOT_FOUND_ERROR_TYPE: &str = "NotFoundException";

/// The sentences this endpoint's own refusals state.
const NO_SUCH_ROUTE: &str = "This endpoint serves POST /graphql.";
const A_QUERY_IS_A_POST: &str = "A GraphQL request is made with POST.";
const BODY_IS_A_DOCUMENT: &str = "The request body must be a JSON document.";
const BODY_CARRIES_A_QUERY: &str = "The request must carry a GraphQL document in query.";
const BODY_TOO_LARGE: &str = "This request is larger than this endpoint accepts.";
const REQUEST_TIMED_OUT: &str = "This request took longer than this endpoint allows.";
const SERVER_BUSY: &str = "This endpoint is serving as many requests as it can. Try again.";
const NO_WEBSOCKETS: &str =
    "A WebSocket connection is served at the API's real-time endpoint.";
