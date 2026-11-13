// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-apigatewayv2-apigatewayv2` service: how it claims requests and how it words the
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
//! - `override_protocol` — the protocol every modelled response is serialized by. EDITED
//!   here: the model's own protocol, with the framing correction the section "How an answer
//!   that carries no content is framed" below states.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## This service's other edited seams: the two `execute-api` data planes
//!
//! Beside its modelled `/v2/…` control plane, this service is addressed on the invoke
//! endpoint of every api it mints — `{api-id}.execute-api.{region}.amazonaws.com`, the URI
//! `Api$ApiEndpoint` reports — where a client's request is no modelled operation at all.
//! Two modules own that half, one per `ProtocolType`, over the readings they share
//! ([`data_plane`]):
//!
//! - [`websocket`] — a WEBSOCKET api, whose data plane is a SOCKET: the core opens one per
//!   api ([`websocket::transport`], reached from the operation that mints the api), that
//!   socket completes the RFC 6455 handshake and carries the frames each route serves
//!   ([`websocket::dispatch`], [`websocket::integration`]), and the address it landed on is
//!   what the api announces as its `ApiEndpoint`. The same handshake arriving at this
//!   emulator's own SERVICE endpoint is claimed ([`websocket::claims_parts`]) and answered
//!   from the api's records there too — as an OPERATION
//!   ([`websocket::serve_with_transport`], which performs the authorizer and the `$connect`
//!   route the handshake's refusals come from), and from
//!   [`Service::override_routing_error`] ([`websocket::serve`]) for the one handshake the
//!   front door would refuse before naming an operation;
//! - [`invoke`] — an ordinary HTTP request to a route of a deployed HTTP api stage: the
//!   claim ([`invoke::claims_parts`]), and the serving ([`invoke::serve`]) as an OPERATION
//!   of this service's own ([`Service::override_operation`]), which is how the router hands
//!   it the `OperationContext` carrying the in-process transport the matched route's
//!   integration is performed over. It declares no auth scheme, so the router serves it
//!   whatever credential it carries, and it names no modelled operation, so the generated
//!   routing table is untouched.
//!
//! An invoke is not named as one of the MODELLED operations, which is how a service asks the
//! router to verify a credential before anything is served, because the router verifies a
//! named operation's SigV4 credential against the signing name of the service that CLAIMED
//! the request, and this service's is its control plane's (`apigateway`). An invoke of a
//! deployed stage is signed under the data plane's own signing name instead — the
//! `execute-api` service AWS publishes the invoke endpoint and the `execute-api:Invoke`
//! action under — so a correctly signed invoke never matches that name and would be refused
//! before the stage is reached, whatever the api's records say. What a route's
//! `AuthorizationType` requires of the request is unchanged: `AWS_IAM` still admits only a
//! request that CARRIES a SigV4 credential ([`invoke::carries_sigv4_credential`]), and
//! refuses one that carries none.
//!
//! One invoke is served from the ROUTING-ERROR seam instead, and
//! [`front_door_refuses_as_cross_site`] states why: the router refuses a request it names an
//! operation for when the browser-set headers say a browser sent it from another site, and
//! it applies that check to no request it does not name one for. So an invoke carrying such
//! a header is left unnamed and answered there ([`invoke::serve_without_transport`]) —
//! rather than refused by the front door before this service sees it. That invoke is a
//! CROSS-ORIGIN request to an api's endpoint, which is exactly the request an api's
//! `CorsConfiguration` exists for, so it is served like any other: the seam carries no
//! request context of its own, so the route's integration is performed over the transport
//! this service remembered instead ([`crate::state::invoke_transport`]).
//!
//! Beside those two there is a THIRD data plane, addressed on the domain every portal this
//! service mints reports (`EndpointConfigurationResponse$PortalDefaultDomainName`):
//!
//! - [`portal`] — an ordinary HTTP request to the website a PUBLISHED portal serves: the
//!   claim ([`portal::claims_parts`]) and the serving ([`portal::serve`], reached from
//!   [`Service::override_routing_error`], since a consumer of a developer portal presents
//!   no AWS credential at all).
//!
//! ## How an answer that carries no content is framed
//!
//! Every operation of this model whose output is empty (`smithy.api#Unit`) answers with no
//! body at all: the deletes, `DisablePortal`, `ResetAuthorizersCache`, `UntagResource`.
//! The recorded AWS answers for those STATE the length such a message has —
//! `content-length: 0` — rather than leaving the framing header off, so a client that
//! closes the message out on an explicit zero length (or a proxy that re-frames it) reads
//! the same message end the origin wrote. It holds whatever status such an answer carries:
//! the `204 No Content` most of them are modelled with, the `202 Accepted` the teardowns
//! that continue after the answer are answered with, and the `200 OK` the authorizer-cache
//! flush is answered with ([`crate::state::wire_status`]).
//!
//! [`Service::override_protocol`] below therefore serves a protocol that is the model's own
//! in every respect except one: a `204` this service answers carries no body, whatever a
//! serializer rendered for it, and every answer that carries no content states
//! `content-length: 0`. A response that carries content is left exactly as the model's
//! protocol rendered it. It is done there, at the one place every modelled response of this
//! service passes through, rather than per operation. The three data planes above frame
//! their own answers, which carry real content, and are not reached through this seam.
//!
//! CAVEAT: a client reached over HTTP/1.1 does not see the header ON A `204`, because the
//! HTTP writer the vendored transport uses removes `content-length` from one (RFC 9110
//! §15.3.5 forbids a body there) — that writer is below anything this crate decides. At
//! every other status, the `202` and the `200` above included, the header reaches the
//! client. What this service states is what the in-process front door (the inter-service
//! callback path and the SDK connector, both of which serve `dispatch`'s response unchanged)
//! delivers, and is pinned by `tests/all/empty_answer_framing.rs`.

mod connections_endpoint;
mod data_plane;
mod invoke;
mod jwt;
mod portal;
// `pub` for the operations that authorize: every operation whose model declares
// `AccessDeniedException` asks `authorization::authorize` before it reads this service's
// store.
pub mod authorization;
// `pub` for the data plane's own half: `websocket::transport::open_for_api` is called by the
// operation that mints a WebSocket api, which is what opens that api's socket and gives its
// `ApiEndpoint` the address to announce.
pub mod websocket;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The status whose answer carries NO content, whatever a serializer rendered for it, and
/// the length an answer that carries no content states. See the module docs.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// How an answer that carries no content is framed: with no body at all, and stating the
/// length it does have. See the module docs.
///
/// A response that carries content is untouched — it keeps the body the protocol rendered
/// and the framing that body already has — so this cannot reframe a modelled output or an
/// error envelope. Whether an answer carries content is read from the BODY rather than from
/// the status, so the empty answer of a teardown answered `202 Accepted` states its length
/// as the `204` answers do; a streaming body, whose length is not known here, states
/// nothing. The insert is FALLIBLE and ignored on failure: a decimal length is always a
/// valid header value, so it cannot fail today, and if it ever did the answer would go out
/// unframed rather than as a transport failure.
fn frame_empty_answer(response: &mut RtResponse) {
    // RFC 9110 §15.3.5 gives a `204` no body at all, so whatever was rendered for one is
    // dropped before the length below is stated.
    if response.status().as_u16() == NO_CONTENT_STATUS {
        *response.body_mut() = aws_smithy_types::body::SdkBody::empty();
    }
    if !response.body().bytes().is_some_and(|body| body.is_empty()) {
        return;
    }
    let _ = response
        .headers_mut()
        .try_insert("content-length", NO_CONTENT_LENGTH);
}

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of this rule, and it is CALLED here rather than re-implemented: the request's route key
/// must match this service's signing name AND its first URI path segment must be one the
/// generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment equality. It
/// is the same rule `service_common::claims_request` applies for a REST service; this
/// crate reaches the framework helper directly, as
/// `service-com-amazonaws-transcribe-transcribe`, `-appconfig-amazonappconfig` and
/// `-ivs-*` do.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// This service's own sentence for one violated member, or `None` for a member it
/// states no sentence of its own for.
///
/// API Gateway words the refusal of some members in the member's OWN terms rather than
/// in the generic "value at path failed to satisfy constraint" form: an api's protocol
/// is refused by listing the protocols there are, and an integration's timeout by naming
/// the window it has to fall in. Each sentence is built from what the MODEL declares for
/// that member — the enum's catalog, the range's bounds — so it cannot drift from the
/// constraint it is explaining.
fn stated_violation(violation: &Violation) -> Option<String> {
    // Only the FIRST violation of a refusal is offered here, so a request that breaks
    // several rules at once is stated in the framework's envelope, which lists them all.
    match (violation.path.as_str(), &violation.kind) {
        // `CreateApi$ProtocolType` / `ProtocolType`, the enum of the protocols this
        // control plane serves. The catalog is the generated `PROTOCOL_TYPE_VALUES`
        // rather than the violation's own list, because the sentence lists the protocols
        // in the catalog's order and the violation carries them in the model's
        // declaration order.
        ("protocolType", ViolationKind::EnumValue(_)) => Some(format!(
            "Invalid protocol specified. Must be one of [{}]",
            crate::ops::types::PROTOCOL_TYPE_VALUES.join(", ")
        )),
        // `CreateIntegration$TimeoutInMillis`, whose shape declares the window
        // (`IntegerWithLengthBetween50And30000`, range 50..30000).
        ("timeoutInMillis", ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)) => {
            Some(format!(
                "Timeout should be between {} ms and {} ms",
                crate::state::MIN_INTEGRATION_TIMEOUT_MILLIS,
                crate::state::MAX_INTEGRATION_TIMEOUT_MILLIS
            ))
        }
        // A routing rule's `Priority`, whose shape declares the order a rule may be
        // evaluated in (`RoutingRulePriority`, range 1..1000000). Stated as the whole
        // range the member has to fall in, however far outside it the value sat.
        ("priority", ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)) => {
            Some(crate::state::routing_rule_priority_range_message())
        }
        // `CreateIntegration$IntegrationType`, the `@required` member that targets the
        // `IntegrationType` enum. A value OUTSIDE that enum names no integration type
        // this service can read, exactly as an absent member does, so both are refused
        // with the one sentence that names the member and what it is required for
        // ([`crate::state::INTEGRATION_TYPE_REQUIRED`]) rather than with the generic
        // constraint clause.
        (
            "integrationType",
            ViolationKind::MissingRequiredMember | ViolationKind::EnumValue(_),
        ) => Some(crate::state::INTEGRATION_TYPE_REQUIRED.to_string()),
        _ => None,
    }
}

/// The `com-amazonaws-apigatewayv2-apigatewayv2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one shared engine, held here because the two data planes ([`websocket`],
    /// [`invoke`]) are decided and served from seams the generated entry does not reach: a
    /// claim and a routing-error answer, both of which read this service's own records.
    engine: Arc<Eng>,
    /// The HTTP api's `execute-api` data plane as an operation, which is how the router
    /// hands it the [`OperationContext`] its integration hop goes over. See
    /// [`HttpApiDataPlane`].
    data_plane: HttpApiDataPlane<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Who judges the `@connections` endpoint a request was addressed through: the stages
        // in those paths are THIS service's records, and the service that serves
        // `@connections` cannot read this store, so the reading is installed for it here —
        // once, when this service is built. See [`connections_endpoint`].
        connections_endpoint::install(&*engine);
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            data_plane: HttpApiDataPlane {
                engine: Arc::clone(&engine),
            },
            engine,
        }
    }
}

/// The HTTP `execute-api` data plane as an [`ErasedOperation`], so the router hands it the
/// [`OperationContext`] carrying the in-process transport the matched route's integration
/// is performed over ([`invoke`]).
///
/// It holds the SAME shared engine the generated entry threads into every operation, so the
/// data plane reads the one store the control-plane operations write.
struct HttpApiDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for HttpApiDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if invoke::claims_request(&*self.engine, req) {
                return invoke::serve(&*self.engine, req, ctx).await;
            }
            // A WebSocket opening handshake arriving at this emulator's own endpoint: the
            // two steps API Gateway decides BEFORE it answers one — the `$connect` route's
            // authorizer and that route's own integration — are performed over this same
            // transport, and only the `101` an admitted handshake earns is beyond this seam.
            if websocket::claims_request(&*self.engine, req) {
                return websocket::serve_with_transport(&*self.engine, req, ctx).await;
            }
            // Unreachable: `override_operation` names this only for a request the data
            // plane claims.
            svc.definition().routing_error_response(req)
        })
    }

    /// NO auth scheme, which is what the router reads as "served unsigned".
    ///
    /// An invoke of a deployed stage is not a modelled operation of this service, and the
    /// credential it carries is not this service's to verify: the router verifies a named
    /// operation's SigV4 credential against the signing name of the service that CLAIMED
    /// the request, and this service's is its control plane's (`apigateway`), while an
    /// invoke is signed under the data plane's own (`execute-api`). Declaring no scheme is
    /// what lets the stage answer a correctly signed invoke at all, and it leaves the
    /// modelled operations untouched: the router reads the schemes off the MODELLED
    /// operation first, and every control-plane request resolves one.
    ///
    /// What a route's `AuthorizationType` requires of the request is unchanged — `AWS_IAM`
    /// still admits only a request that CARRIES a SigV4 credential
    /// ([`invoke::carries_sigv4_credential`]), and refuses one that carries none.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// Whether the FRONT DOOR refuses this request before a named operation of this service is
/// reached, because its browser-set headers say a browser sent it from another site.
///
/// The router applies that check to every request it NAMES AN OPERATION for, and to no
/// request it does not, with the same reading this calls
/// (`emulator_server::classify_browser_request` over same-origin-only). So an invoke
/// carrying such a header is left unnamed and answered through
/// [`Service::override_routing_error`] below — the one seam that still answers it — where
/// the api's own `CorsConfiguration` decorates the answer, every refusal the gateway owes it
/// is computed, and the route's integration is performed over the transport this service
/// remembered, since that seam is handed no request context to carry one
/// ([`invoke::serve_without_transport`]).
fn front_door_refuses_as_cross_site(req: &RtRequest) -> bool {
    let header = |name: &str| req.headers().get(name);
    matches!(
        ::emulator_server::classify_browser_request(
            header("origin"),
            header("sec-fetch-site"),
            &::emulator_server::RemoteOrigins::SameOriginOnly,
            false,
        ),
        ::emulator_server::BrowserVerdict::Forbid(_)
    )
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that
        // carries no content is emptied of whatever was rendered for it and framed.
        frame_empty_answer(&mut response);
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let mut response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries a JSON error envelope at a 4xx, so
        // nothing here carries an empty body to frame; the same call is made on both paths
        // rather than leaving one of them to answer differently.
        frame_empty_answer(&mut response);
        Ok(response)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}

// `Service::override_operation` below names ONE operation of this service's own: the HTTP
// `execute-api` data plane, which is how the router hands an invoke the
// `OperationContext` the matched route's integration is performed over. Every modelled
// request still resolves through the generated routing table, which this override is asked
// ahead of and declines. An invoke the front door would refuse before a named operation is
// reached is left unnamed and answered by `override_routing_error` below; the module docs
// state the whole rule.
impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `apigateway` — a signing name it SHARES with the Amazon
        // API Gateway control plane
        // (`services/service-com-amazonaws-apigateway-backplanecontrolservice`, `sdkId`
        // `API Gateway`, the same `apigateway` endpoint prefix and signing name), a
        // DIFFERENT service that AWS separates by URI path space: every operation of THIS
        // model is bound under `/v2/…` (`/v2/apis`, `/v2/domainnames`, `/v2/vpclinks`,
        // `/v2/tags`, …), while the control plane's are bound under `/restapis`,
        // `/apikeys`, `/account`, … The router resolves a request's routing key from the
        // SigV4 credential scope, so both services resolve to `apigateway`; claiming on
        // the signing name ALONE makes both claim every such request, which the router
        // answers `500 AmbiguousService` — a code no AWS model carries — leaving BOTH
        // services unreachable.
        //
        // So the claim adds the framework's path conjunct over this service's own
        // model-derived path space — see [`can_handle_parts`], which for this model is the
        // single segment `v2`. None of the control plane's segments is `v2`, so it alone
        // claims its traffic and each service answers only its own.
        if can_handle_parts(parts) {
            return true;
        }
        // And the two DATA planes: a request addressed to a stage of an api THIS service
        // holds. Such a request carries no credential scope naming this service (or
        // carries one while naming no `/v2/…` route), and its path is no modelled
        // operation's, so the front door could resolve no target service for it and
        // answered `404 UnknownService` — for the endpoint this service itself reported in
        // `ApiEndpoint`. Each claim is narrowed to an api this service HOLDS, read from the
        // one shared store, and to that api's own `ProtocolType`, so exactly one service
        // and exactly one of these modules claims the request; see [`websocket`] and
        // [`invoke`] for the whole rule and for what each request is answered with.
        websocket::claims_parts(&*self.engine, parts)
            || invoke::claims_parts(&*self.engine, parts)
            || portal::claims_parts(&*self.engine, parts)
    }

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // An invoke of a deployed HTTP api stage is named as an operation so the router
        // hands it the `OperationContext`: the route's integration is a hop to another
        // emulated service, and that context carries the only in-process transport it can
        // be made over. It names no MODELLED operation by construction (its path is a
        // stage's, not a `/v2/…` route's), so this takes nothing away from the generated
        // routing table, and it declares no auth scheme, so the router serves it whatever
        // credential it carries.
        //
        // Except where the front door would refuse it first: see
        // [`front_door_refuses_as_cross_site`].
        if front_door_refuses_as_cross_site(req) {
            return None;
        }
        (invoke::claims_request(&*self.engine, req)
            || websocket::claims_request(&*self.engine, req))
            .then_some(&self.data_plane as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) BEFORE any handler
        // runs, so it is where this service names the error shape such a refusal
        // carries. This model declares exactly one error for a request it cannot
        // accept — `BadRequestException`, "The request is not valid, for example, the
        // input is incomplete or incorrect", bound to HTTP 400 on every operation that
        // writes — and declares no `ValidationException` anywhere, so the framework's
        // default code names a shape no client of this service can resolve.
        //
        // The message is this service's own sentence for the member and the constraint
        // that failed where the service has one ([`stated_violation`]), and otherwise
        // the framework's, which already names both.
        //
        // A refusal of the caller's IDENTITY is not an invalid request and keeps the
        // framework's 403 wording.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let message = violations
            .first()
            .and_then(stated_violation)
            .or_else(|| aws_smithy_schema_server::envelope(violations))
            .unwrap_or_else(|| "The request is not valid.".to_string());
        Some(Refused {
            code: "BadRequestException".to_string(),
            message,
        })
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here. See the module docs: it is the model's own protocol, with
        // one correction — an answer that carries no content carries no body and states
        // `content-length: 0`, rather than leaving the framing header off.
        Some(self)
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
        // A WebSocket opening handshake IS such a request: it names no modelled operation
        // (its path is a stage's, not a `/v2/…` route's). It is NAMED as an operation above,
        // so the two steps a handshake's answer needs an in-process transport for — the
        // `$connect` route's authorizer and that route's own integration — are performed;
        // this seam answers the one handshake the front door would otherwise refuse itself
        // (a cross-site one), from the api's records alone.
        //
        // So is an invoke of an HTTP api's stage, whatever credential it carries: reaching
        // it BEFORE authentication is what lets a client holding only a bearer token, or
        // one signing for the data plane's own signing name, be answered at all.
        if websocket::claims_request(&*self.engine, req) {
            return Some(websocket::serve(&*self.engine, req));
        }
        if invoke::claims_request(&*self.engine, req) {
            return Some(invoke::serve_without_transport(
                &self.engine,
                self.definition().identity(),
                req,
            ));
        }
        // And a visit to the website a published portal serves on its own domain, which
        // likewise names no modelled operation and carries no AWS credential.
        if portal::claims_request(&*self.engine, req) {
            return Some(portal::serve(&*self.engine, req));
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
