// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sns-amazonsimplenotificationservice` service: how it claims requests and how it words the
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
//! This file uses three of them: `override_operation` and `override_routing_error`
//! to add the `<RequestId>` element to every awsQuery error document (below), and
//! `override_refusal` to keep a `@sensitive` member's value out of a validation
//! refusal.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The awsQuery error document
//!
//! SNS speaks awsQuery, whose error document is
//! `<ErrorResponse><Error>…</Error><RequestId>…</RequestId></ErrorResponse>`: the
//! `<RequestId>` is a sibling of `<Error>`, OUTSIDE it, so no error shape's members
//! can carry it and the framework's error serializer — which writes only the error
//! shape's own members, inside `<Error>` — cannot either. It is a property of the
//! RESPONSE, not of the error, so this file adds it where every response this
//! service answers with passes: [`WithRequestId`], the wrapper
//! `override_operation` hands the router in place of each modelled operation, and
//! `override_routing_error` for the one failure that names no operation at all.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;

/// The closing tag of the awsQuery error document, and the only marker that tells
/// an error response apart from a success envelope (whose own request id already
/// sits in `<ResponseMetadata>`).
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The closing tag of the error element the request id is a SIBLING of: what sits
/// between it and [`ERROR_RESPONSE_CLOSE`] is the only place a `<RequestId>` of
/// this document can be.
const ERROR_ELEMENT_CLOSE: &str = "</Error>";

/// The element an awsQuery error document carries beside `<Error>`.
const REQUEST_ID_ELEMENT: &str = "<RequestId>";

/// The header AWS reports the same id in, so a client correlating on the header
/// and one reading the document see one value.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The input members this service's model marks `@sensitive`, spelled the way a
/// validation path spells them (lowerCamel, as
/// `aws_smithy_schema_server::constraints::path::member` builds it).
///
/// Source: `smithy_query(query="operation", operation="CreateSMSSandboxPhoneNumber,
/// VerifySMSSandboxPhoneNumber,DeleteSMSSandboxPhoneNumber,Publish,
/// CheckIfPhoneNumberIsOptedOut,OptInPhoneNumber", model_path=
/// "services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/\
/// com-amazonaws-sns-amazonsimplenotificationservice.json")`: every one of them
/// reports `"sensitive": true` on its phone-number member — `PhoneNumber` on four,
/// `phoneNumber` on two, and a validation path spells both `phoneNumber`. They are
/// also the only input members of this service that both carry the trait and carry
/// a constraint to fail. An entry added here comes from that same query, never from
/// one request's shape.
const SENSITIVE_MEMBER_PATHS: &[&str] = &["phoneNumber"];

/// Whether `path` names, or sits under, a member the model marks `@sensitive`: a
/// value held under a sensitive member is sensitive too, so any segment naming one
/// redacts the clause.
fn is_sensitive_path(path: &str) -> bool {
    path.split('.')
        .any(|segment| SENSITIVE_MEMBER_PATHS.contains(&segment))
}

/// The error code the query protocols name a declared-bound refusal with (`query`
/// in `vendor/smithy-server-runtime/src/pre_dispatch.rs`). Repeated here because
/// [`Refused`] replaces the code and the message TOGETHER, and only the message
/// changes below: a client branches on the code, so it must be the one it would
/// have seen.
///
/// It is the code for a value that failed a DECLARED BOUND and for nothing else.
/// A refusal the framework describes with the same `Refusal::Invalid` but renders
/// under another code — an absent required member, which travels as a codec error
/// and renders as `MalformedInput` — must not be reworded here, and
/// [`without_sensitive_values`] declines any refusal that carries one.
const BOUND_REFUSAL_CODE: &str = "ValidationError";

/// The code SNS names a refusal of the member that ADDRESSES the resource a call acts on
/// with: the `awsQueryError` code of the modelled `InvalidParameterException`
/// (`smithy_query(query="operation", operation="ListSubscriptionsByTopic")` reports
/// `awsQueryError {code: InvalidParameter, httpResponseCode: 400}`), which is the error a
/// generated client maps back to that exception.
const ADDRESSED_MEMBER_REFUSAL_CODE: &str = "InvalidParameter";

/// How SNS states that a required member carried no value at all: `Invalid parameter:`,
/// the member as the request spells it, and the reason. Fixed text around the one
/// variable part, which is the member name.
fn no_value_for_required_parameter(member: &str) -> String {
    format!("Invalid parameter: {member} Reason: no value for required parameter")
}

/// The ARN member a refusal reports as absent, or `None` when no violation is an absent
/// ARN member.
///
/// The ARN member is the one SNS resolves BEFORE anything else about a request — it
/// names the resource the call acts on, and the account that resource belongs to — so
/// its absence is reported by the service itself, as the modelled `InvalidParameter`,
/// rather than by the generic validation layer in front of it. Only such a member is
/// resolved that early, which is why every other absence keeps the validation layer's
/// own sentence below.
fn absent_arn_member(violations: &[Violation]) -> Option<&'static str> {
    violations.iter().find_map(|violation| {
        matches!(violation.kind, ViolationKind::MissingRequiredMember)
            .then(|| crate::state::account_resolution::arn_member_of_path(&violation.path))
            .flatten()
    })
}

/// How this service words a request the framework refused before any handler ran.
///
/// Two classes, told apart by WHICH layer of SNS judges the request:
///
/// 1. the member that addresses the resource is absent — resolved by the service itself,
///    so it answers its own modelled `InvalidParameter`
///    ([`no_value_for_required_parameter`]);
/// 2. anything else the declared rules of the input reject — a value outside a bound, a
///    member absent from a shape the service has no resource to resolve from — is a
///    constraint violation of the REQUEST, stated in the validation layer's own
///    `<n> validation error(s) detected: …` sentence under the code the query protocol's
///    validation layer names ([`BOUND_REFUSAL_CODE`]). That code is the whole point of
///    the arm: a body the service could not READ at all is a different failure
///    (`MalformedInput`), and a request whose members were read and then judged must not
///    be reported as one.
///
/// A `@sensitive` member's value is lifted out of the clause either way
/// ([`without_sensitive_values`]).
fn sns_refusal(violations: &[Violation]) -> Option<Refused> {
    if let Some(member) = absent_arn_member(violations) {
        return Some(Refused {
            code: ADDRESSED_MEMBER_REFUSAL_CODE.to_string(),
            message: no_value_for_required_parameter(member),
        });
    }
    Some(Refused {
        code: BOUND_REFUSAL_CODE.to_string(),
        message: without_sensitive_values(violations)?,
    })
}

/// The validation layer's own sentence for `violations`, with no `@sensitive` member's
/// value in it, or `None` when there is nothing to state.
///
/// The clause is the framework's, with any echoed sensitive value lifted out of it: the
/// requirement a caller has to read ("Member must satisfy regular expression
/// pattern: …") is exactly as stated for every other member, and only the
/// `Value '<the number>' at` opening becomes `Value at`. The pattern replaced is
/// built from the SAME two fields the renderer interpolated, so it matches the
/// echo exactly whatever the value holds.
fn without_sensitive_values(violations: &[Violation]) -> Option<String> {
    let mut message = aws_smithy_schema_server::envelope(violations)?;
    for violation in violations {
        let Some(value) = violation.value.as_deref() else {
            continue;
        };
        if !is_sensitive_path(&violation.path) {
            continue;
        }
        message = message.replace(
            &format!("Value '{value}' at '{}'", violation.path),
            &format!("Value at '{}'", violation.path),
        );
    }
    Some(message)
}

/// `resp` with the `<RequestId>` element an awsQuery error document carries, when
/// it is such a document and does not already have one.
///
/// Every other response is returned untouched: a success envelope already carries
/// its request id inside `<ResponseMetadata>`, and a body that is not an
/// `<ErrorResponse>` document has nowhere this element belongs.
fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let Some(bytes) = resp.body().bytes() else {
        return resp;
    };
    let Ok(body) = std::str::from_utf8(bytes) else {
        return resp;
    };
    // The LAST occurrence: every interpolated value in the document is XML-escaped,
    // so the closing tag cannot appear inside one, and anchoring on the last keeps
    // that true of any document the framework adds around it later.
    let Some(close) = body.rfind(ERROR_RESPONSE_CLOSE) else {
        return resp;
    };
    // Only the SIBLING position counts as "already has one": a `<RequestId>`
    // somewhere inside `<Error>` (a member of some future error shape, say) is a
    // different element and does not answer for this one.
    let siblings = match body[..close].rfind(ERROR_ELEMENT_CLOSE) {
        Some(error_end) => &body[error_end..close],
        None => &body[..close],
    };
    if siblings.contains(REQUEST_ID_ELEMENT) {
        return resp;
    }
    let request_id = crate::state::new_request_id();
    let mut document = String::with_capacity(body.len() + request_id.len() + 24);
    document.push_str(&body[..close]);
    document.push_str(REQUEST_ID_ELEMENT);
    document.push_str(&request_id);
    document.push_str("</RequestId>");
    document.push_str(&body[close..]);
    *resp.body_mut() = SdkBody::from(document);
    // Dropped rather than fatal if the HTTP layer cannot carry it: the document
    // already states the id, and a header is not worth failing a modelled error for.
    let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    resp
}

/// Every modelled operation of this service, answered through
/// [`with_request_id`].
///
/// One value serves them all: the operation a request names is resolved from the
/// request the same way the router resolves it, and `run` delegates to it, so this
/// wrapper adds the response element and decides nothing else — not routing, not
/// auth (its `auth_schemes` stays `None`, which defers to the modelled operation
/// the routing key names), and not the body.
///
/// The lookup happens twice per request — once in `override_operation`, which
/// hands this wrapper back only for a key the routing table answers, and once
/// here. That is deliberate and costs one `HashMap` probe: declining in
/// `override_operation` is what keeps a request naming no modelled action on the
/// ROUTER's own path (its routing-error answer, rendered before authentication)
/// rather than quietly moving it behind this wrapper.
struct WithRequestId<Eng: Send + Sync + 'static> {
    /// The shared engine, held so the gates in front of every operation — the request-rate
    /// budgets of [`crate::state::request_quota`] — reach this service's own store. The
    /// operations themselves reach it through the generated entry, which owns its own handle.
    engine: Arc<Eng>,
}

/// The answer this endpoint gives a request addressing an account it does not serve: the
/// awsQuery error document of the account-resolution refusal
/// ([`crate::state::account_resolution`]), in the same envelope every other refusal of
/// this service is stated in.
fn no_account_response() -> RtResponse {
    let body = aws_smithy_schema_server::query_error_body(
        false,
        crate::state::account_resolution::NO_ACCOUNT_CODE,
        crate::state::account_resolution::NO_ACCOUNT_MESSAGE,
    );
    let status = ::aws_smithy_runtime_api::http::StatusCode::try_from(
        crate::state::account_resolution::NO_ACCOUNT_STATUS,
    )
    .expect("the refusal's status is a valid constant");
    let mut resp = RtResponse::new(status, SdkBody::from(body));
    resp.headers_mut().insert("content-type", "text/xml");
    resp
}

/// The answer this endpoint gives a call past the published request quota of the operation it
/// names: the awsQuery error document of the throttling refusal
/// ([`crate::state::request_quota`]), in the same envelope every other refusal of this
/// service is stated in.
fn throttled_response() -> RtResponse {
    let body = aws_smithy_schema_server::query_error_body(
        false,
        crate::state::request_quota::THROTTLE_CODE,
        crate::state::request_quota::THROTTLE_MESSAGE,
    );
    let status = ::aws_smithy_runtime_api::http::StatusCode::try_from(
        crate::state::request_quota::THROTTLE_STATUS,
    )
    .expect("the refusal's status is a valid constant");
    let mut resp = RtResponse::new(status, SdkBody::from(body));
    resp.headers_mut().insert("content-type", "text/xml");
    resp
}

/// Whether `req` addresses an account this endpoint does not serve, and so is refused
/// before any operation of this service reads it.
fn addresses_another_account(req: &RtRequest, ctx: &OperationContext<'_>) -> bool {
    let Some(body) = req.body().bytes() else {
        // A body this process has not buffered is one no ARN could be read from; the
        // request then travels on to the operation, which judges what it can read.
        return false;
    };
    let region = crate::state::request_region(ctx.parts());
    crate::state::account_resolution::addresses_another_account(body, &region)
}

impl<Eng: Send + Sync + 'static> ErasedOperation for WithRequestId<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The request RATE is decided first of all: a published quota is a property of
            // the endpoint the request reached rather than of any operation's own work, so a
            // call past it is refused before anything of the request is read and performs
            // nothing at all. The operation it names is the request's own routing key, which
            // is the name the quota is published per.
            if let Ok(operation) = svc.definition().protocol().routing_key(req) {
                let region = crate::state::request_region(ctx.parts());
                if crate::state::request_quota::is_throttled(&*self.engine, operation, &region) {
                    return with_request_id(throttled_response());
                }
            }
            // The account the request addresses is resolved BEFORE the operation reads
            // the request: an ARN of another account names no resource this endpoint can
            // resolve, and the refusal reports nothing about what the account it does
            // serve holds. It is applied here, once, because it is the layer in front of
            // every operation rather than a rule of any one of them.
            if addresses_another_account(req, ctx) {
                return with_request_id(no_account_response());
            }
            let definition = svc.definition();
            let modeled = definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key));
            match modeled {
                Some(op) => with_request_id(op.run(svc, req, ctx).await),
                // Unreachable through `override_operation`, which hands this
                // wrapper back only for a request the routing table answers:
                // state the protocol's own routing failure rather than panic.
                None => with_request_id(definition.routing_error_response(req)),
            }
        })
    }
}

/// The media type the signing certificate is served as: it is a PEM certificate file.
const CERTIFICATE_MEDIA_TYPE: &str = "application/x-pem-file";

/// The document published at the URL every delivered notification carries as its
/// `SigningCertURL`: the X.509 certificate that verifies the signature on that notification
/// ([`crate::state::notification_signature`]).
///
/// It is served HERE rather than by a modelled operation because it is not one: SNS
/// publishes the certificate as a plain document a subscriber fetches by URL, which the
/// model's routing key — the `Action` of an awsQuery form — cannot name. That is what
/// `override_operation` is for.
struct SigningCertificate;

impl ErasedOperation for SigningCertificate {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let (status, media_type, body) =
                match crate::state::notification_signature::published_certificate(req.uri()) {
                    Some(certificate) => (200, CERTIFICATE_MEDIA_TYPE, certificate.to_string()),
                    // A URL that names no certificate this process holds leads to no
                    // document, which is what a fetch of it is told.
                    None => (
                        404,
                        "text/plain",
                        "no such signing certificate".to_string(),
                    ),
                };
            let status = ::aws_smithy_runtime_api::http::StatusCode::try_from(status)
                .expect("the certificate response's status is a valid constant");
            let mut resp = RtResponse::new(status, SdkBody::from(body));
            resp.headers_mut().insert("content-type", media_type);
            resp
        })
    }

    /// `None`, so this request is authenticated exactly as the modelled operations are: the
    /// URL a notification carries is signed ([`crate::state::notification_signature`]), so a
    /// caller that fetches it sends a request this emulator verifies like any other.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// The `com-amazonaws-sns-amazonsimplenotificationservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Held, not built per request: `override_operation` returns a reference to it.
    with_request_id: WithRequestId<Eng>,
    /// Held for the same reason: the signing certificate's own answer.
    signing_certificate: SigningCertificate,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // The key the notifications of this service are signed with is the SERVICE's own, so
        // it is generated here rather than on the publish that first needs it.
        crate::state::notification_signature::prepare_signing_key();
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            with_request_id: WithRequestId { engine },
            signing_certificate: SigningCertificate,
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
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The signing certificate of the notifications this service delivers is a plain
        // document fetched by URL, which no modelled action names, so it is answered before
        // the routing table is consulted.
        if crate::state::notification_signature::addresses_certificate(req.method(), req.uri()) {
            return Some(&self.signing_certificate);
        }
        // Serve every modelled operation through [`WithRequestId`], which answers
        // from the generated routing table and then adds the `<RequestId>` element
        // the awsQuery error document carries beside `<Error>` — the one place every
        // response of every operation passes through, so no operation can answer an
        // error document without it. A request the routing table does not answer is
        // left to the router, which renders this service's own routing failure.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.entry.routing_table().get(key)?;
        Some(&self.with_request_id)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The framework refuses a request that fails a modelled rule before any handler
        // runs; which error SNS names such a refusal by depends on which of its layers
        // judged the request, and a `@sensitive` member's value is never reproduced in
        // the sentence. See [`sns_refusal`].
        match refusal {
            Refusal::Invalid(violations) => sns_refusal(violations),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The protocol-correct default (query -> 400 InvalidAction text/xml), plus
        // the `<RequestId>` element every awsQuery error document carries: a request
        // that named no modelled action is answered with the same document shape as
        // one that named a modelled action and failed.
        Some(with_request_id(self.entry.routing_error_response(req)))
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
