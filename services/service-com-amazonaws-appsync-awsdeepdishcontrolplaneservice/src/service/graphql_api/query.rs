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
//! from the core's own predicates — the `Host` allowlist that guards against DNS rebinding and
//! the WebSocket refusal (the real-time endpoint is the other socket) — and the request is read
//! under one deadline and one body ceiling, both the core's, so a slow or oversized client
//! cannot hold a connection permit open. The `Origin` a request declares is NOT one of them and
//! refuses nothing: this endpoint is an address a client of any origin calls, so it answers the
//! CORS preflight itself ([`preflight_response`]) and serves the real request whatever origin
//! it declares, decided by the material it presents like every other call, with every answer
//! its own route produced stating what a page may read of it ([`cross_origin_permission`]).
//! Two things bound what that admits. A request a BROWSER ITSELF stamped as made from another
//! site ([`stamped_cross_site`]) is refused before anything of the API is reached unless the
//! operator asked for that site ([`REMOTE_ORIGINS_ENV`], whose default is a page served from
//! this machine), so no site the developer happens to be visiting drives an API of this
//! emulator. And material this service cannot CHECK — everything but a key it minted itself —
//! is dropped from a request declaring an origin of another site ([`checkable_material`]), so
//! no such client authors a principal of the developer's account for itself.
//!
//! Every answer this endpoint writes — the result, and each refusal above — carries the
//! REQUEST ID of the request it answers in [`REQUEST_ID_FIELD`], minted per request by the
//! API's own data plane (`crate::state::GraphqlDataPlane::mint_request_id`). It is the value
//! a client correlates its answer with what the API LOGGED about producing it by, because
//! every record an API with a `logConfig` writes carries that same id
//! ([`super::field_logs`]). A request the API itself answered is also a request it LOGS and
//! one it COUNTS in its own `AWS/AppSync` series
//! ([`super::metrics::GraphqlApiMetrics::request_answered`]); four are not, because AppSync
//! produced no answer of its own for them: one the WEB ACL decided at the edge, one this
//! endpoint never read (shed for want of a permit, past the body ceiling, or past the read
//! deadline), one refused by the transport-safety rules before any of it was dispatched, and
//! one answered at the CORS preflight, which reaches neither the API's authorization nor a
//! document. So no `5xx` answer this endpoint writes is one the API itself produced — each of
//! them is a request it never read or a contained panic — and its `5XXError` series carries a
//! zero for every request it did answer.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::Arc;
use ::std::time::{Duration, Instant, SystemTime};

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::serde_json::{Map as JsonMap, Value as Json};
use ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge as waf_edge;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::execution::{self, GraphqlRequest};
use super::{authorizer, caller, field_auth, field_logs, PlaneContext};
use crate::service::event_api::{self, PresentedAuth};

/// The response field every answer this endpoint writes carries the request's own id in, as
/// AppSync names it.
pub const REQUEST_ID_FIELD: &str = "x-amzn-requestid";

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
            // The id of THIS request, minted by the API's own data plane before anything of
            // the request is read, so every answer below — including one this endpoint
            // refuses and a contained panic — names the request it is of.
            let request_id = context.state.lock().graphql_data_plane.mint_request_id();
            // Every request's whole answer is driven through the core's panic containment,
            // because this socket is served by this module rather than by the front door's
            // router: there is no middleware layer to carry it.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            // What the answer may state about reading it from a PAGE, read before the request
            // is consumed: `None` for a request this endpoint routes nowhere
            // ([`cross_origin_permission`]).
            let permission = cross_origin_permission(
                request.method(),
                request.uri().path(),
                request.headers(),
            );
            let answered = contained(
                facts,
                answer(context, in_flight, request, request_id.clone()),
            )
            .await;
            Ok::<_, ::std::convert::Infallible>(stamped(answered, &request_id, permission))
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

/// `answered` with the request's own id written on it, and — where the request reached a
/// route of the API — with what a page may read of the answer.
///
/// A minted id is a UUID, so it is always a value a response field can carry; an answer that
/// somehow carries one already keeps it rather than being stamped twice. The cross-origin
/// permission is on every answer the API's own route produced, because a page that made the
/// request has to be able to READ it — an error of this endpoint as much as a result — and on
/// none of the answers to a request that reached no operation of the API, which no route of it
/// produced ([`cross_origin_permission`] decides both).
fn stamped(
    mut answered: ::http::Response<Full<Bytes>>,
    request_id: &str,
    permission: Option<Vec<(&'static str, ::http::HeaderValue)>>,
) -> ::http::Response<Full<Bytes>> {
    let name = ::http::HeaderName::from_static(REQUEST_ID_FIELD);
    if let Ok(value) = ::http::HeaderValue::from_str(request_id) {
        answered.headers_mut().entry(name).or_insert(value);
    }
    for (name, value) in permission.into_iter().flatten() {
        answered
            .headers_mut()
            .entry(::http::HeaderName::from_static(name))
            .or_insert(value);
    }
    answered
}

/// What the answer to this request states about reading it FROM A PAGE — the origin it permits
/// and the response fields a script may read off it — or `None` for a request this endpoint
/// routes nowhere ([`reaches_the_api`]), whose answer no route of the API produced.
///
/// The origin permitted is [`ALLOWED_ORIGIN`] — ANY — on every answer alike, because the
/// `Origin` a request carries decides no request's admission: the result a page is served, the
/// refusal its material earned and the preflight's own answer all state the same permission,
/// which is what lets a page read the GraphQL envelope whatever site it was served from. The
/// ANSWER can still depend on that field, because material this service cannot check
/// authorizes no page of another site ([`checkable_material`]), so the answer says so
/// ([`VARY_FIELD`]), as the core's own HTTP listeners do: a cache holding one answer must not
/// hand it to a page of another origin.
/// A page may read only the safelisted response fields of a cross-origin answer unless the
/// answer says otherwise (Fetch, §3.2.3's `Access-Control-Expose-Headers`), and the two fields
/// this endpoint writes ABOUT the answer — the request's own id and the class of a refusal —
/// are not among them, so they are named under the very names this endpoint writes them under.
fn cross_origin_permission(
    method: &::http::Method,
    path: &str,
    headers: &::http::HeaderMap,
) -> Option<Vec<(&'static str, ::http::HeaderValue)>> {
    if !reaches_the_api(method, path, headers) {
        return None;
    }
    Some(vec![
        (
            ALLOW_ORIGIN_FIELD,
            ::http::HeaderValue::from_static(ALLOWED_ORIGIN),
        ),
        (
            EXPOSE_HEADERS_FIELD,
            ::http::HeaderValue::from_static(EXPOSED_FIELDS),
        ),
        (
            VARY_FIELD,
            ::http::HeaderValue::from_static(::http::header::ORIGIN.as_str()),
        ),
    ])
}

/// Whether the request addresses a ROUTE of this endpoint at all: the path the API is served
/// at, addressed with anything but an `OPTIONS` that is no preflight — the preflight being the
/// only `OPTIONS` this endpoint routes ([`is_preflight`]).
///
/// The ONE reading of that question, so what an answer states about reading it from a page
/// cannot drift from the route that produced the answer: [`served_request`] answers each
/// request of a route it does route, and [`cross_origin_permission`] states nothing about one
/// it does not.
fn reaches_the_api(
    method: &::http::Method,
    path: &str,
    headers: &::http::HeaderMap,
) -> bool {
    serves_path(path) && (method != ::http::Method::OPTIONS || is_preflight(method, headers))
}

/// Whether `path` is the one this endpoint serves its API at. The ONE reading of that
/// question, taken by the route ([`reaches_the_api`]) and by the answer
/// ([`served_request`]), so a change to the path cannot leave an answer stating a permission
/// for a route nothing serves.
fn serves_path(path: &str) -> bool {
    path == GRAPHQL_PATH
}

/// Whether the request is a CORS PREFLIGHT: an `OPTIONS` asking about the method a browser is
/// about to send (Fetch, §4.8's "CORS-preflight request"). The ONE reading of that question,
/// which the answer ([`preflight_response`]) and the route ([`reaches_the_api`]) both take.
fn is_preflight(method: &::http::Method, headers: &::http::HeaderMap) -> bool {
    method == ::http::Method::OPTIONS && headers.contains_key(REQUEST_METHOD_FIELD)
}

/// Answer one request to the GraphQL endpoint.
async fn answer(
    context: PlaneContext,
    in_flight: Arc<Semaphore>,
    request: ::http::Request<::hyper::body::Incoming>,
    request_id: String,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_dispatch(&parts, &context.admitted_origins) {
        return refused;
    }
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // One permit per request IN FLIGHT, which is what bounds the memory the bodies on this
        // endpoint hold. The permit is WAITED for rather than demanded: an AppSync API serves
        // the requests a client makes at the same time rather than refusing one because
        // another is still in flight, so a request that finds every permit taken takes its
        // turn — under this request's own deadline, so the wait is bounded and a client that
        // never gets one is answered rather than held. Held for the whole read and the
        // execution, and released with them.
        let Ok(_permit): Result<OwnedSemaphorePermit, _> =
            Arc::clone(&in_flight).acquire_owned().await
        else {
            return error_response(503, "ServiceUnavailableException", SERVER_BUSY);
        };
        let Ok(collected) = limited.collect().await else {
            return error_response(413, BAD_REQUEST_ERROR_TYPE, BODY_TOO_LARGE);
        };
        served_request(&context, &parts, &collected.to_bytes(), &request_id).await
    })
    .await;
    served.unwrap_or_else(|_| error_response(408, BAD_REQUEST_ERROR_TYPE, REQUEST_TIMED_OUT))
}

/// Answer ONE already-framed request of this API, minting the request id it is answered
/// and logged under — the whole of what this endpoint does with a request once a connection
/// has been read ([`served_request`] and [`stamped`]), under the same PANIC CONTAINMENT
/// ([`contained`]).
///
/// What the socket above reaches through its own connection handling, and what a request
/// that reached the endpoint from INSIDE this emulator is answered by
/// ([`super::in_process`]): both are the same request to the same API, so both are answered
/// by the same code — the web ACL at the edge, the API's authorization, the document's
/// execution and the records the API logs about having served it — and both are answered
/// through the containment, so a panic anywhere under it is this endpoint's own `500`
/// rather than something that unwinds into the caller. That matters most for the IN-PROCESS
/// caller: its stack is another service's delivery pass, which a panic escaping here would
/// take with it.
///
/// The request is REBUILT as an owned one for that: the containment guards a future of its
/// own ('static), and what it needs of the request is the method, the target, the fields and
/// the body — the request as this endpoint reads it.
pub async fn served_buffered_request(
    context: &PlaneContext,
    parts: &::http::request::Parts,
    body: &[u8],
) -> ::http::Response<Full<Bytes>> {
    let request_id = context.state.lock().graphql_data_plane.mint_request_id();
    let mut rebuilt = ::http::Request::new(());
    *rebuilt.method_mut() = parts.method.clone();
    *rebuilt.uri_mut() = parts.uri.clone();
    *rebuilt.version_mut() = parts.version;
    *rebuilt.headers_mut() = parts.headers.clone();
    let facts = ::emulator_server::RequestFacts::from_request(&rebuilt);
    let (owned, ()) = rebuilt.into_parts();
    let context = context.clone();
    let body = body.to_vec();
    let answered = {
        let request_id = request_id.clone();
        contained(facts, async move {
            served_request(&context, &owned, &body, &request_id).await
        })
        .await
    };
    stamped(answered, &request_id, None)
}

/// Answer one framed request: the GraphQL route, and the refusals every other request to
/// this endpoint meets.
async fn served_request(
    context: &PlaneContext,
    parts: &::http::request::Parts,
    body: &[u8],
    request_id: &str,
) -> ::http::Response<Full<Bytes>> {
    // When this request began and the clock its latency is measured on: both are what the
    // API's own records of it state ([`field_logs`]).
    let started = SystemTime::now();
    let clock = Instant::now();
    if !serves_path(parts.uri.path()) {
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            404,
            NOT_FOUND_ERROR_TYPE,
            NO_SUCH_ROUTE,
        )
        .await;
    }
    // The CORS PREFLIGHT a browser makes before the real request, which this endpoint
    // answers itself: nothing of the API is reached by it, so it presents no authorization
    // material, carries no document and is not a request the API executed
    // ([`preflight_response`] states the rule in full). It is also the ONLY `OPTIONS` this
    // endpoint routes, so one that asks about no method at all addresses nothing it serves.
    if is_preflight(&parts.method, &parts.headers) {
        return preflight_response(parts);
    }
    if parts.method == ::http::Method::OPTIONS {
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            404,
            UNKNOWN_OPERATION_ERROR_TYPE,
            NO_SUCH_ROUTE,
        )
        .await;
    }
    if parts.method != ::http::Method::POST {
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            405,
            BAD_REQUEST_ERROR_TYPE,
            A_QUERY_IS_A_POST,
        )
        .await;
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
    // The material this endpoint may DECIDE this call on: what the request presents, less
    // the material it cannot check from a client declaring an origin of another site
    // ([`checkable_material`]).
    let material = checkable_material(&parts.headers);
    let auth = PresentedAuth::from_fields(&material);
    let authorized = {
        let store = context.state.lock();
        super::authorized(&store, &context.api_id, &auth)
    };
    if !authorized {
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            401,
            event_api::UNAUTHORIZED_ERROR_TYPE,
            event_api::UNAUTHORIZED_MESSAGE,
        )
        .await;
    }
    // WHO the call speaks for, resolved once: it is both what a resolver reads as
    // `$context.identity` and what the API's fields are authorized for ([`caller`]). A call
    // that presents no SigV4 material names no principal, which is every call made under
    // another of the API's authorization modes.
    let signed = caller::presented_caller(&context.engine, &material);
    let mut identity = signed
        .as_ref()
        .map_or(Json::Null, |caller| caller.identity.clone());
    // What the API's TOKEN modes decide about this call, which is taken BEFORE any field of it
    // resolves: the value the request presents in its `authorization` field is judged by the
    // mode that issued it — verified against the user pool the API configured, or against the
    // OpenID Connect provider it configured — and what the admitted token CLAIMS is who the
    // call speaks for ([`super::token_decision`]). An API that set up no token mode decides
    // nothing here. The groups an admitted user pool token claims are kept, because the API's
    // schema may guard its fields by them.
    let mut user_pool_groups: Option<Vec<String>> = None;
    match super::token_decision(
        context,
        presented_material(&material),
        caller::source_ip(&parts.headers),
        started,
    ) {
        super::TokenDecision::NotApplicable => {}
        super::TokenDecision::Refused(refused) => {
            ::tracing::info!(
                api_id = %context.api_id,
                reason = %refused.reason,
                "the token this call presented authorizes no call on this API"
            );
            return refusal_logged(
                context,
                request_id,
                started,
                clock,
                UNAUTHORIZED_STATUS,
                event_api::UNAUTHORIZED_ERROR_TYPE,
                refused.message,
            )
            .await;
        }
        super::TokenDecision::Admitted {
            identity: admitted,
            groups,
        } => {
            identity = admitted;
            user_pool_groups = groups;
        }
    }
    let Ok(document) = ::serde_json::from_slice::<Json>(body) else {
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            400,
            BAD_REQUEST_ERROR_TYPE,
            BODY_IS_A_DOCUMENT,
        )
        .await;
    };
    let Some(query) = document.get(QUERY_MEMBER).and_then(Json::as_str) else {
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            400,
            BAD_REQUEST_ERROR_TYPE,
            BODY_CARRIES_A_QUERY,
        )
        .await;
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
        identity,
        // The principal the call signed as, carried into the execution because it is also
        // what admits a call a resolver's data source makes under the CALLER's own
        // credentials — the same answer the API's own fields are judged against.
        caller: signed.clone(),
        unauthorized: Vec::new(),
        // The groups an admitted user pool token claims, which the schema's own group guards
        // are judged against as each field resolves ([`user_pool::refused_fields`]).
        user_pool_groups: user_pool_groups.clone(),
        // Filled in below, once every mode that decides a call has had its say: an API that
        // set up more than one mode says per field which of them may reach it
        // ([`field_auth`]).
        auth_mode: None,
    };
    let mut request = request;
    // What the API's `AWS_LAMBDA` authorization mode decides about this call, which is taken
    // BEFORE any field of it resolves: the authorizer is invoked with the token presented and
    // what it returned admits the call, withholds fields of it, or refuses it outright
    // ([`authorizer`]). An API that configured no Lambda authorizer decides nothing here.
    let presented = authorizer::Presented {
        token: presented_material(&material),
        request_id,
        query: &request.query,
        operation_name: request.operation_name.as_deref(),
        variables: &request.variables,
    };
    let mut lambda_admitted = false;
    match authorizer::decided(context, &presented).await {
        authorizer::Decided::NotApplicable => {}
        authorizer::Decided::Refused => {
            ::tracing::info!(
                api_id = %context.api_id,
                "this API's Lambda authorizer did not authorize this call"
            );
            return refusal_logged(
                context,
                request_id,
                started,
                clock,
                UNAUTHORIZED_STATUS,
                event_api::UNAUTHORIZED_ERROR_TYPE,
                event_api::UNAUTHORIZED_MESSAGE,
            )
            .await;
        }
        authorizer::Decided::Admitted { identity, denied } => {
            // What the authorizer published about the caller IS this call's identity, and the
            // fields it withheld resolve to nothing.
            request.identity = identity;
            request.unauthorized.extend(denied);
            lambda_admitted = true;
        }
    }
    // WHICH of the API's authorization modes this call is of, resolved once now that every
    // mode that decides a call has had its say: it is what the schema's own `@aws_*`
    // directives are judged against as each field resolves ([`field_auth`]).
    request.auth_mode = context
        .state
        .lock()
        .graphql_apis
        .get(&context.api_id)
        .and_then(|api| {
            field_auth::resolved_mode(
                api,
                &field_auth::Presented {
                    api_key: auth.api_key.as_deref(),
                    authorization: auth.authorization.as_deref(),
                    signed: signed.is_some(),
                    user_pool_admitted: user_pool_groups.is_some(),
                    lambda_admitted,
                },
            )
        });
    // What IAM says about the fields this call selects. A call NONE of whose fields it
    // permits is refused outright; one it permits some of is served, with the rest reported
    // as fields the caller may not access.
    let judged = judged_fields(context, &arn, signed.as_ref(), &request);
    if judged.refuses_the_call() {
        ::tracing::info!(
            api_id = %context.api_id,
            "this caller's IAM policies permit no field this call selects"
        );
        context
            .metrics
            .request_answered(UNAUTHORIZED_STATUS, clock.elapsed());
        return error_response(
            UNAUTHORIZED_STATUS,
            event_api::UNAUTHORIZED_ERROR_TYPE,
            PERMISSION_DENIED,
        );
    }
    // Beside the fields the authorizer withheld, where there was one.
    request.unauthorized.extend(judged.refused);
    // A document is EXECUTED against the API's schema, so an API that serves none has nothing
    // to execute this request against: the request is refused rather than served — no `data`
    // at all and one error of the class AppSync names an API whose schema cannot be served
    // by, which is the same class the control-plane read of that schema is refused with. That
    // is the state a MERGED API is left in once the associations whose merges carried its
    // types are gone, and the one an API no schema has ever been created on is in.
    if execution::serves_no_schema(context) {
        ::tracing::info!(
            api_id = %context.api_id,
            "this API serves no schema, so there is nothing to execute this request against"
        );
        return refusal_logged(
            context,
            request_id,
            started,
            clock,
            SCHEMA_EXCEPTION_STATUS,
            SCHEMA_EXCEPTION_ERROR_TYPE,
            NO_SCHEMA_CONFIGURED,
        )
        .await;
    }
    let answered = execution::execute(context, &request).await;
    field_logs::delivered(
        context,
        &field_logs::Served {
            request_id,
            status: 200,
            started,
            latency: clock.elapsed(),
            answer: Some(&answered),
        },
    )
    .await;
    // What the API publishes about having answered this request, which is the ONLY way a
    // client observes its request rate, its latency and its client-error rate.
    context.metrics.request_answered(200, clock.elapsed());
    json_response(200, &answered.document())
}

/// One refusal the API's own logs report: the answer, and the record the API logs about
/// having refused the request.
///
/// A request the API itself refused is one it SERVED — the status it was answered with is
/// what a request-level record states — so it is logged like any other, with no execution to
/// report because none ran, and counted like any other
/// ([`super::metrics::GraphqlApiMetrics::request_answered`]).
async fn refusal_logged(
    context: &PlaneContext,
    request_id: &str,
    started: SystemTime,
    clock: Instant,
    status: u16,
    error_type: &str,
    message: &str,
) -> ::http::Response<Full<Bytes>> {
    field_logs::delivered(
        context,
        &field_logs::Served {
            request_id,
            status,
            started,
            latency: clock.elapsed(),
            answer: None,
        },
    )
    .await;
    context.metrics.request_answered(status, clock.elapsed());
    error_response(status, error_type, message)
}

/// What IAM decided about the FIELDS one call selects.
struct Judged {
    /// The `(typeName, fieldName)` pairs the caller's policies do not permit.
    refused: Vec<(String, String)>,
    /// Whether any field the call selects IS permitted.
    any_permitted: bool,
}

impl Judged {
    /// Whether the whole call is refused: the caller may reach no field of it at all, which
    /// is the request AppSync answers `401` rather than serving a result with no data in it.
    fn refuses_the_call(&self) -> bool {
        !self.any_permitted && !self.refused.is_empty()
    }
}

/// Judge each field this call selects for `appsync:GraphQL`.
///
/// AppSync's `AWS_IAM` mode is the IAM layer in front of the API: "you can use IAM policies
/// to restrict access to … individual fields", so each field the call selects of the
/// operation's root type is judged on that field's own ARN ([`caller::field_resource`])
/// BEFORE any resolver runs. A field no statement allows, and one an explicit `Deny` covers,
/// is a field the caller may not access: it resolves to nothing and is reported as an error
/// of its own path, and no resolver of it runs. A call that may reach NO field it selects is
/// refused outright.
///
/// Judged only for a call the mode applies to: the API has to have configured `AWS_IAM` (as
/// its `authenticationType` or as one of its additional providers), and the call has to
/// present the SigV4 material that mode decides. Everything else permits every field, as
/// every other authorization mode decides a call as a whole.
fn judged_fields(
    context: &PlaneContext,
    arn: &str,
    signed: Option<&caller::SignedCaller>,
    request: &GraphqlRequest,
) -> Judged {
    let permitted = Judged {
        refused: Vec::new(),
        any_permitted: true,
    };
    let Some(signed) = signed else {
        return permitted;
    };
    if !context
        .state
        .lock()
        .graphql_apis
        .get(&context.api_id)
        .is_some_and(|api| super::configures(api, caller::IAM_AUTH_TYPE))
    {
        return permitted;
    }
    let selected = execution::selected_root_fields(context, request);
    let mut judged = Judged {
        refused: Vec::new(),
        any_permitted: false,
    };
    for (type_name, field_name) in selected {
        if caller::permits(
            signed,
            &caller::field_resource(arn, &type_name, &field_name),
        ) {
            judged.any_permitted = true;
            continue;
        }
        ::tracing::info!(
            api_id = %context.api_id,
            %type_name,
            %field_name,
            "this caller's IAM policies do not let it perform appsync:GraphQL on the field"
        );
        judged.refused.push((type_name, field_name));
    }
    judged
}

/// The material this endpoint may DECIDE one request on: every field it presents for one of
/// the API's authorization modes, less what a request whose `Origin` names ANOTHER SITE
/// presents that this endpoint cannot check.
///
/// The `Origin` decides no request's admission on this endpoint — an AppSync API serves a
/// client whatever origin it declares ([`refused_before_dispatch`]) — and this is not that
/// decision: it is WHICH of the material presented this endpoint is able to CHECK. This
/// service holds the secret of exactly one of the providers a `GraphqlApi` configures, the
/// `API_KEY` whose keys it minted itself (`CreateApiKey`), and it checks those: a request
/// carrying a key of the API is decided by that key here as everywhere. Material of every
/// other provider — a signature, a Cognito or OIDC token, a Lambda authorizer's input — is
/// taken as PRESENTED on a data-plane socket, because this emulator verifies a signature at
/// its front door and never on one of these sockets ([`caller`]), so a PAGE could author it
/// for itself: a page of a site the developer merely visited would otherwise name a principal
/// of their account by writing the field. It presents no material this endpoint can check, so
/// the API's own rule ([`super::authorized`]) refuses the call exactly as it refuses one that
/// presents nothing — the `401` AppSync answers material it does not accept — and the modes
/// that read that field decide nothing about it.
fn checkable_material(headers: &::http::HeaderMap) -> ::http::HeaderMap {
    let mut material = headers.clone();
    let names_another_site = headers
        .get(::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| !event_api::served_from_this_machine(origin));
    if names_another_site && material.remove(::http::header::AUTHORIZATION).is_some() {
        ::tracing::info!(
            "a request declaring an origin of another site presents no material this endpoint \
             can check in its authorization field, so the call is decided without it"
        );
    }
    material
}

/// The sites a page a BROWSER serves may drive a GraphQL API's `GRAPHQL` endpoint from, as the
/// operator asked for them: resolved ONCE when an API's sockets are opened
/// (`super::open_for_api`, through [`configured_admitted_origins`]), never in the request path.
#[derive(Debug, Clone)]
pub enum AdmittedOrigins {
    /// A page served from THIS MACHINE and nothing else — the DEFAULT, which is the policy
    /// every listener of this emulator applies (`emulator_server::RemoteOrigins`'s own
    /// same-origin default): with nothing configured, no site the developer happens to be
    /// visiting can drive an API of this emulator from their browser.
    ThisMachine,
    /// A page served from this machine, and one served from a site the operator NAMED
    /// ([`REMOTE_ORIGINS_ENV`]).
    TheseSites(Vec<String>),
    /// A page served from ANY site, which is what the endpoint being emulated serves and what
    /// the operator asks for with [`EVERY_SITE`]: AppSync keeps no origin allowlist, so this
    /// is the posture a browser-based client of the real service meets.
    AnySite,
}

impl AdmittedOrigins {
    /// Whether a page served from `origin` may drive the endpoint. Asked only of a request a
    /// BROWSER stamped as one it made from another site ([`stamped_cross_site`]); every other
    /// client is admitted whatever origin it declares.
    fn admits(&self, origin: Option<&str>) -> bool {
        match self {
            Self::AnySite => true,
            Self::ThisMachine => origin.is_some_and(event_api::served_from_this_machine),
            Self::TheseSites(named) => origin.is_some_and(|origin| {
                event_api::served_from_this_machine(origin)
                    || named.iter().any(|named| named == origin)
            }),
        }
    }
}

/// The setting an operator widens this emulator's GraphQL endpoints with: the sites,
/// comma-separated, a page a BROWSER serves may drive an API's data plane from besides a page
/// served from this machine, or [`EVERY_SITE`] for the posture the endpoint being emulated has.
///
/// Unset — the default — admits none of them, so no site the developer happens to be visiting
/// can drive an API of this emulator from their browser; setting it is announced
/// ([`announced_once`]).
pub const REMOTE_ORIGINS_ENV: &str = "APPSYNC_GRAPHQL_REMOTE_ORIGINS";

/// What an operator names in [`REMOTE_ORIGINS_ENV`] to ask for the posture the endpoint being
/// emulated has: a page of EVERY site admitted, as AppSync admits one.
pub const EVERY_SITE: &str = "*";

/// The sites [`REMOTE_ORIGINS_ENV`] asks this emulator to serve a page from, read ONCE when an
/// API's sockets are opened (`super::open_for_api`) and never in the request path.
#[must_use]
pub fn configured_admitted_origins() -> AdmittedOrigins {
    let named: Vec<String> = ::std::env::var(REMOTE_ORIGINS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(|site| site.trim().to_string())
        .filter(|site| !site.is_empty())
        .collect();
    if named.is_empty() {
        return AdmittedOrigins::ThisMachine;
    }
    announced_once(&named.join(","));
    if named.iter().any(|site| site == EVERY_SITE) {
        return AdmittedOrigins::AnySite;
    }
    AdmittedOrigins::TheseSites(named)
}

/// Whether the widened posture of this emulator's GraphQL endpoints has been announced, which
/// it is ONCE per process ([`announced_once`]).
static ANNOUNCED_THE_POSTURE: ::std::sync::atomic::AtomicBool =
    ::std::sync::atomic::AtomicBool::new(false);

/// Announce that the operator asked these endpoints to serve a page a browser serves from
/// somewhere other than this machine — said when the first API's sockets are opened, before
/// any of them has served a request, and said once, because what it reports is a posture of
/// the emulator rather than a fact about any one API.
///
/// Said at a level a developer sees, because what it widens is who can drive and read the data
/// plane of every GraphQL API of this emulator. Nothing is said for the default, which widens
/// nothing: with this setting unset, only a page served from this machine drives one.
fn announced_once(sites: &str) {
    if ANNOUNCED_THE_POSTURE.swap(true, ::std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    ::tracing::warn!(
        %sites,
        setting = %REMOTE_ORIGINS_ENV,
        "a page a browser serves from these sites may drive and read every GraphQL API of this \
         emulator; unset this setting to admit only a page served from this machine"
    );
}

/// The material a request presents for an authorization mode to decide: the value of the
/// request's own `authorization` field, which is where AppSync reads a Lambda authorizer's
/// input from — and also where a SIGNED call presents its signature, which the mode that reads
/// one decides instead ([`authorizer::decided`]).
fn presented_material(headers: &::http::HeaderMap) -> &str {
    headers
        .get(::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
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
        return Some(with_headers(
            edge_response(custom),
            &decision.response_headers,
        ));
    }
    // A `Block` with no custom response is the refusal AppSync's own edge words. A `Captcha`
    // or `Challenge` with none is answered out of WAF's own envelope: the status WAF serves
    // its interstitial under, and the headers WAF stamps on a response it generates itself
    // (`EdgeDecision::response_headers`), which is what the client's CAPTCHA or challenge
    // integration reads. The interstitial ITSELF is a script WAF serves from its own
    // endpoint, which this emulator serves none of, so the answer carries the status and the
    // same envelope rather than a page nothing produced.
    Some(with_headers(
        error_response(
            decision.action.default_status().unwrap_or(FORBIDDEN_STATUS),
            WAF_FORBIDDEN_ERROR_TYPE,
            WAF_FORBIDDEN_MESSAGE,
        ),
        &decision.response_headers,
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
    with_headers(response, &fields)
}

/// `response` carrying `headers` as well: the headers WAF stamps on a response it
/// generates itself, on top of whatever that response already carries.
///
/// A header name or value a web ACL names is not restricted to what a response can carry,
/// so one the wire cannot hold is left off the answer rather than failing the request.
fn with_headers(
    mut response: ::http::Response<Full<Bytes>>,
    headers: &[(String, String)],
) -> ::http::Response<Full<Bytes>> {
    for (name, value) in headers {
        match (
            ::http::HeaderName::from_bytes(name.as_bytes()),
            ::http::HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(value)) => {
                response.headers_mut().insert(name, value);
            }
            _ => ::tracing::warn!(
                %name,
                "the web acl's response names a header a response cannot carry, and it is \
                 left off the answer"
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
/// from the core's own predicates so this endpoint and a core listener decide them alike: the
/// `Host` allowlist that guards against DNS rebinding, the WebSocket refusal, and the page a
/// BROWSER STAMPED as one it serves from somewhere this emulator was not asked to serve one
/// from.
///
/// The `Origin` a request declares is NOT one of them, and refuses nothing: an AppSync API's
/// `GRAPHQL` endpoint is an address a client of any origin calls, and that header is one any
/// client may set, so a request carrying it is admitted on the material it presents like every
/// other ([`checkable_material`] decides which of that material this endpoint can check). What
/// is refused here is the request a BROWSER ITSELF classified as made from another site
/// ([`stamped_cross_site`], the field a script cannot write), and only while the operator has
/// not asked for that site ([`AdmittedOrigins`], whose default is a page of this machine):
/// that is the request a site the developer happens to be visiting makes, and refusing it is
/// what keeps that site from driving the developer's own emulator. The CORS PREFLIGHT is
/// answered before this check, because a preflight presents nothing and reaches nothing of the
/// API ([`preflight_response`]), so a page always learns what it may send.
fn refused_before_dispatch(
    parts: &::http::request::Parts,
    admitted: &AdmittedOrigins,
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
    if is_preflight(&parts.method, &parts.headers) {
        return None;
    }
    let origin = header("origin");
    if stamped_cross_site(header("sec-fetch-site")) && !admitted.admits(origin) {
        ::tracing::info!(
            setting = %REMOTE_ORIGINS_ENV,
            "refusing a request a browser made from a page this emulator was not asked to \
             serve a GraphQL API to"
        );
        return Some(error_response(
            FORBIDDEN_STATUS,
            event_api::UNAUTHORIZED_ERROR_TYPE,
            event_api::UNAUTHORIZED_MESSAGE,
        ));
    }
    None
}

/// Whether a BROWSER stamped this request as one it made from another site: `Sec-Fetch-Site`
/// naming `cross-site` or `same-site` (Fetch, §3.2.1), the browser's own classification of
/// where the request came from.
///
/// It is the browser that writes that field and a script cannot, which is what makes it the
/// one reading of "a page of another site is driving this" that a page cannot talk its way out
/// of — where a request's `Origin` is a field any client may set, and where the recorded
/// behaviour of the endpoint being emulated is that it admits a request whatever origin it
/// declares. A request carrying no such stamp is no browser's, or an old browser's, and is
/// admitted: what it may then present is still only material this service can check
/// ([`checkable_material`]).
fn stamped_cross_site(sec_fetch_site: Option<&str>) -> bool {
    sec_fetch_site.is_some_and(|stamped| {
        stamped.eq_ignore_ascii_case("cross-site") || stamped.eq_ignore_ascii_case("same-site")
    })
}

/// The answer to one CORS PREFLIGHT on this endpoint — the request [`is_preflight`] admits,
/// which is the only `OPTIONS` this endpoint routes.
///
/// A browser asks, with `OPTIONS` and `Access-Control-Request-Method`, whether the request it
/// is about to make may be made (Fetch, §4.8: the "CORS-preflight request"). An AppSync API's
/// `GRAPHQL` endpoint answers that question itself, before anything that decides the real
/// request: the preflight carries no authorization material — a browser sends none on it —
/// and no document, so it is neither authorized nor executed, and nothing of the API is
/// reached. What it answers is what the real request is permitted to carry: the METHOD and
/// the FIELDS the browser asked about, which this endpoint places no restriction of its own
/// on ([`EVERY_FIELD`] where it asked about none), and how long the answer may be reused. What
/// a page may then READ of an answer is stated on every answer of this endpoint
/// ([`cross_origin_permission`]), this one included.
fn preflight_response(parts: &::http::request::Parts) -> ::http::Response<Full<Bytes>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());
    // No BODY: the answer is the permission itself, and a preflight's answer carries no
    // payload for a browser to read.
    let mut answered = ::http::Response::builder()
        .status(200)
        .header(::http::header::CONTENT_LENGTH, "0")
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(Bytes::new())));
    let allowed = [
        (
            ALLOW_METHODS_FIELD,
            header(REQUEST_METHOD_FIELD).unwrap_or_default().to_string(),
        ),
        (
            ALLOW_HEADERS_FIELD,
            header(REQUEST_HEADERS_FIELD)
                .unwrap_or(EVERY_FIELD)
                .to_string(),
        ),
        (MAX_AGE_FIELD, PREFLIGHT_MAX_AGE_SECONDS.to_string()),
    ];
    for (name, value) in allowed {
        if let Ok(value) = ::http::HeaderValue::from_str(&value) {
            answered
                .headers_mut()
                .insert(::http::HeaderName::from_static(name), value);
        }
    }
    answered
}

/// The fields a browser asks a preflight with, and the ones the answer states the permission
/// in (Fetch, §3.2.2 and §3.2.3).
const REQUEST_METHOD_FIELD: &str = "access-control-request-method";
const REQUEST_HEADERS_FIELD: &str = "access-control-request-headers";
const ALLOW_ORIGIN_FIELD: &str = "access-control-allow-origin";
const ALLOW_METHODS_FIELD: &str = "access-control-allow-methods";
const ALLOW_HEADERS_FIELD: &str = "access-control-allow-headers";
const EXPOSE_HEADERS_FIELD: &str = "access-control-expose-headers";
const MAX_AGE_FIELD: &str = "access-control-max-age";

/// The field an answer names the request fields its own value DEPENDS on in (RFC 9110 §12.5.5),
/// which an answer of this endpoint does: material this service cannot check authorizes no page
/// of another site ([`checkable_material`]).
const VARY_FIELD: &str = "vary";

/// What a preflight's answer permits when the browser asked about no FIELD in particular:
/// every field, as Fetch §3.2.3 spells that — a list of field names, which is why it is not
/// the permitted ORIGIN's own spelling of "any" ([`ALLOWED_ORIGIN`]) even though both are
/// written the same.
const EVERY_FIELD: &str = "*";

/// The response fields a page may read off an answer of this endpoint: the request's own id
/// ([`REQUEST_ID_FIELD`]) and the class of a refusal ([`ERROR_TYPE_FIELD`]), which are the two
/// this endpoint writes about the answer itself.
const EXPOSED_FIELDS: &str = "x-amzn-requestid,x-amzn-errortype";

/// How long a browser may reuse this endpoint's preflight answer without asking again — the
/// two days AppSync's own preflight states, which is the maximum a browser caches one for
/// (`Access-Control-Max-Age`, Fetch §3.2.3: "the number of seconds the information provided
/// … can be cached"). A value of the ENDPOINT rather than of any request, so it is the same
/// on every preflight it answers.
const PREFLIGHT_MAX_AGE_SECONDS: u32 = 172_800;

/// The origin this endpoint permits a page to read its answer from: ANY, on every answer its
/// own route produced — the result, the refusal the material presented earned, and the
/// preflight alike ([`cross_origin_permission`]). The `Origin` a request carries is no input
/// to it, so no answer of this endpoint names one site rather than another.
const ALLOWED_ORIGIN: &str = "*";

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
    let mut response = json_response(
        status,
        &event_api::publish::errors_document(error_type, message),
    );
    // AppSync names the type of a data-plane error in a field of the answer as well as in
    // the error itself, which is where a client that reads neither body nor status looks.
    if let Ok(value) = ::http::HeaderValue::from_str(error_type) {
        response
            .headers_mut()
            .insert(::http::HeaderName::from_static(ERROR_TYPE_FIELD), value);
    }
    response
}

/// The field AppSync names the class of a data-plane error in.
const ERROR_TYPE_FIELD: &str = "x-amzn-errortype";

/// The members a GraphQL request carries.
const QUERY_MEMBER: &str = "query";
const VARIABLES_MEMBER: &str = "variables";
const OPERATION_NAME_MEMBER: &str = "operationName";

/// The `errorType` values this endpoint's refusals carry, as AppSync's own errors name them.
const BAD_REQUEST_ERROR_TYPE: &str = "BadRequestException";
const NOT_FOUND_ERROR_TYPE: &str = "NotFoundException";

/// The `errorType` AppSync names a request that reaches no operation of the API with: an
/// `OPTIONS` that is not the CORS preflight, which is the only `OPTIONS` this endpoint routes.
const UNKNOWN_OPERATION_ERROR_TYPE: &str = "UnknownOperationException";

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

/// The sentence AppSync states when the caller's IAM policies do not permit the call: the
/// refusal of the `AWS_IAM` authorization mode, which names neither the field it refused nor
/// the policy that refused it.
pub const PERMISSION_DENIED: &str = "Permission denied";

/// The status a call this API's Lambda authorizer did not authorize is answered with: the
/// request reached no field of the API, so it is refused rather than served — the status every
/// unauthorized call on a GraphQL API's data plane is answered with.
const UNAUTHORIZED_STATUS: u16 = 401;

/// How a request on an API that serves NO schema is refused: the class AppSync names such a
/// failure by — the same `GraphQLSchemaException` the control-plane read of an unservable
/// schema is refused with (`GetIntrospectionSchema`) — the sentence it states, and the status
/// it carries. A request nothing was wrong with reached an API that cannot answer it, which is
/// a failure of the service's own side of the exchange rather than of the request.
const SCHEMA_EXCEPTION_ERROR_TYPE: &str = "GraphQLSchemaException";
const NO_SCHEMA_CONFIGURED: &str = "Schema is not configured on api.";
const SCHEMA_EXCEPTION_STATUS: u16 = 502;

#[cfg(test)]
mod tests {
    use super::*;

    /// What an answer of this endpoint states about reading it from a page, over the origins a
    /// request declares and the routes it addresses: the same permission whatever the origin,
    /// and none at all for a request that reaches no operation of the API.
    #[test]
    fn every_answer_of_a_route_of_this_endpoint_permits_any_origin_to_read_it() {
        let origins: &[Option<&'static str>] = &[
            None,
            Some("http://localhost:5173"),
            Some("http://127.0.0.1:3000"),
            Some("https://studio.apollographql.com"),
            Some("https://a-site-nobody-asked-for.example"),
            // The opaque origin of a sandboxed frame.
            Some("null"),
        ];
        for origin in origins {
            let mut headers = ::http::HeaderMap::new();
            if let Some(origin) = *origin {
                headers.insert(
                    ::http::header::ORIGIN,
                    ::http::HeaderValue::from_static(origin),
                );
            }
            let permitted = |method: &::http::Method, path: &str, headers: &::http::HeaderMap| {
                cross_origin_permission(method, path, headers).map(|fields| {
                    fields
                        .into_iter()
                        .map(|(name, value)| {
                            (name, value.to_str().unwrap_or_default().to_string())
                        })
                        .collect::<Vec<_>>()
                })
            };
            let served = permitted(&::http::Method::POST, GRAPHQL_PATH, &headers);
            assert_eq!(
                served,
                Some(vec![
                    (ALLOW_ORIGIN_FIELD, ALLOWED_ORIGIN.to_string()),
                    (EXPOSE_HEADERS_FIELD, EXPOSED_FIELDS.to_string()),
                    (VARY_FIELD, ::http::header::ORIGIN.as_str().to_string()),
                ]),
                "case {origin:?}: expected the same permission on every answer of the route, \
                 got {served:?}"
            );
            // A CORS preflight is a route of this endpoint; an `OPTIONS` that asks about no
            // method, and any other path, are not.
            let mut preflight = headers.clone();
            preflight.insert(
                ::http::HeaderName::from_static(REQUEST_METHOD_FIELD),
                ::http::HeaderValue::from_static("POST"),
            );
            assert_eq!(
                permitted(&::http::Method::OPTIONS, GRAPHQL_PATH, &preflight),
                served,
                "case {origin:?}: a preflight is answered with the same permission"
            );
            for (method, path) in [
                (::http::Method::OPTIONS, GRAPHQL_PATH),
                (::http::Method::POST, "/"),
                (::http::Method::GET, "/graphql/other"),
            ] {
                let answered = permitted(&method, path, &headers);
                assert_eq!(
                    answered, None,
                    "case {origin:?}: {method} {path} reaches no operation of the API, so its \
                     answer states no permission, got {answered:?}"
                );
            }
        }
    }

    /// Which of the material presented this endpoint may DECIDE on, over the origin a request
    /// declares and what it carries: the key this service minted is material it can check, and
    /// the field every other provider presents in is material a client of another origin could
    /// have authored, so it is not decided on.
    #[test]
    fn a_client_of_another_origin_presents_only_material_this_service_can_check() {
        /// One row: what the request carries, the origin it declares, and whether the
        /// authorization field survives. The key always does.
        struct Row {
            carried: &'static [(&'static str, &'static str)],
            origin: Option<&'static str>,
            keeps_authorization: bool,
        }
        const BOTH: &[(&str, &str)] = &[("x-api-key", "da2-minted"), ("authorization", "signed")];
        const SIGNED: &[(&str, &str)] = &[("authorization", "signed")];
        let rows: &[Row] = &[
            // A client declaring no origin at all, and one declaring an origin of this
            // machine: every field it presents is material this endpoint decides on.
            Row {
                carried: BOTH,
                origin: None,
                keeps_authorization: true,
            },
            Row {
                carried: BOTH,
                origin: Some("http://localhost:5173"),
                keeps_authorization: true,
            },
            Row {
                carried: SIGNED,
                origin: Some("http://127.0.0.1:3000"),
                keeps_authorization: true,
            },
            // And one declaring an origin of another site — whether or not the operator asked
            // for that site, since what this decides is what the endpoint can CHECK.
            Row {
                carried: BOTH,
                origin: Some("https://sim2real.example"),
                keeps_authorization: false,
            },
            Row {
                carried: BOTH,
                origin: Some("https://studio.apollographql.com"),
                keeps_authorization: false,
            },
            Row {
                carried: SIGNED,
                origin: Some("null"),
                keeps_authorization: false,
            },
        ];
        for row in rows {
            let (carried, origin) = (row.carried, row.origin);
            let mut headers = ::http::HeaderMap::new();
            for (name, value) in carried {
                headers.insert(
                    ::http::HeaderName::from_static(name),
                    ::http::HeaderValue::from_static(value),
                );
            }
            if let Some(origin) = origin {
                headers.insert(
                    ::http::header::ORIGIN,
                    ::http::HeaderValue::from_static(origin),
                );
            }
            let presented = PresentedAuth::from_fields(&checkable_material(&headers));
            let carried_value = |field: &str| {
                carried
                    .iter()
                    .find(|(name, _)| *name == field)
                    .map(|(_, value)| *value)
            };
            assert_eq!(
                presented.api_key.as_deref(),
                carried_value("x-api-key"),
                "case {carried:?} from {origin:?}: a key this service minted is material it can \
                 check, so it is always decided on, got {:?}",
                presented.api_key
            );
            let expected = carried_value("authorization").filter(|_| row.keeps_authorization);
            assert_eq!(
                presented.authorization.as_deref(),
                expected,
                "case {carried:?} from {origin:?}: expected the authorization field \
                 {expected:?}, got {:?}",
                presented.authorization
            );
        }
    }

    /// WHO may drive the endpoint from a page a browser serves, over the stamp a browser writes
    /// and the sites an operator asked for: with nothing configured a page of this machine and
    /// nothing else, with sites named those too, and with every site asked for, any.
    #[test]
    fn a_browser_stamped_cross_site_request_is_admitted_only_from_the_sites_asked_for() {
        // The stamps a browser writes (Fetch, §3.2.1's `Sec-Fetch-Site`), and whether each
        // says the request came from another site.
        let stamps: &[(Option<&str>, bool)] = &[
            (None, false),
            (Some("none"), false),
            (Some("same-origin"), false),
            (Some("same-site"), true),
            (Some("cross-site"), true),
            (Some("Cross-Site"), true),
        ];
        for (stamp, cross_site) in stamps {
            assert_eq!(
                stamped_cross_site(*stamp),
                *cross_site,
                "case {stamp:?}: expected cross_site={cross_site}"
            );
        }

        let nothing_asked = AdmittedOrigins::ThisMachine;
        let asked = AdmittedOrigins::TheseSites(vec![
            "https://studio.apollographql.com".to_string(),
            "https://console.example.aws".to_string(),
        ]);
        let every_site = AdmittedOrigins::AnySite;
        // (origin, admitted with nothing configured, with `asked`, with every site asked for)
        let rows: &[(Option<&str>, bool, bool, bool)] = &[
            (Some("http://localhost:5173"), true, true, true),
            (Some("http://127.0.0.1:3000"), true, true, true),
            (Some("https://localhost"), true, true, true),
            (Some("http://[::1]:8080"), true, true, true),
            (Some("https://studio.apollographql.com"), false, true, true),
            (Some("https://console.example.aws"), false, true, true),
            (Some("https://sim2real.example"), false, false, true),
            // An exact list admits exactly what it names: neither a prefix of an origin it
            // names nor a name merely ending in one.
            (
                Some("https://studio.apollographql.com.attacker.example"),
                false,
                false,
                true,
            ),
            (Some("https://notlocalhost"), false, false, true),
            // The opaque origin of a sandboxed frame names no site at all, and a stamped
            // request carrying no origin names none either.
            (Some("null"), false, false, true),
            (None, false, false, true),
        ];
        for (origin, alone, configured, any) in rows {
            for (posture, expected) in [
                (&nothing_asked, alone),
                (&asked, configured),
                (&every_site, any),
            ] {
                assert_eq!(
                    posture.admits(*origin),
                    *expected,
                    "case {origin:?} with {posture:?}: expected admitted={expected}"
                );
            }
        }
    }

    /// The fields an answer says a page may read are the ones this endpoint WRITES about an
    /// answer, so renaming either cannot silently stop exposing it.
    #[test]
    fn every_field_this_endpoint_writes_about_an_answer_is_one_a_page_may_read() {
        let exposed: Vec<&str> = EXPOSED_FIELDS.split(',').map(str::trim).collect();
        assert_eq!(
            exposed,
            vec![REQUEST_ID_FIELD, ERROR_TYPE_FIELD],
            "the exposed fields must be exactly the two this endpoint writes, got {exposed:?}"
        );
    }
}
