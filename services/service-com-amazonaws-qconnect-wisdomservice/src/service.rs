// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-qconnect-wisdomservice` service: how it claims requests and how it words the
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
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The content-upload plane
//!
//! `StartContentUpload` hands out a URL, and a URL is a promise that it can be
//! FOLLOWED. The URL it mints is SIGNED by this emulator
//! (`crate::state::sign_data_plane_url`) for the upload's own path
//! (`/knowledgeBases/<knowledgeBaseId>/uploads/<uploadId>`) — a path the model binds no
//! operation to, so it reaches this file twice:
//!
//! - [`Service::can_handle`] claims a request to an upload path naming an upload THIS
//!   store holds, so following the URL routes back here even when nothing else in the
//!   request names a service (a loopback endpoint host names no service at all).
//! - [`Service::override_operation`] names the operation that serves it (`data_plane`).
//!   That seam is resolved BEFORE the router authenticates the request, so the request is
//!   authenticated against the scheme that operation declares — which is what puts the
//!   plane behind the same signature verification a modelled operation is behind, and why
//!   the URL carries one.
//!
//! ## The message-template attachment plane
//!
//! `CreateMessageTemplateAttachment` answers with "a pre-signed Amazon S3 URL that can be
//! used to download the attachment file", which is the same promise: the URL it mints is
//! signed the same way, one segment deeper than the modelled route for the attachment
//! itself, and is claimed and served by the same two seams as the content-asset URL. A
//! `GET` answers the file the upload carried and changes nothing.
//!
//! The plane writes ONLY the upload the URL names — the bytes, bounded both per body
//! (`crate::state::MAX_CONTENT_UPLOAD_BYTES`) and across every written body this store
//! holds (`crate::state::MAX_PENDING_UPLOAD_BYTES`) — and refuses a `PUT` that is late
//! (the URL's `urlExpiry` has passed) or that does not carry the headers the start call
//! published. Nothing else about the service is reachable through it. The legacy Amazon
//! Connect Wisdom view of this same endpoint serves its own uploads the same way, out of
//! its own store.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service, ViolationKind};
use std::sync::Arc;

/// The data planes this service publishes URLs into, served behind the emulator's own
/// authentication.
mod data_plane;

/// The error code this endpoint answers a request the boundary in front of the service
/// refuses with: `BadRequestException`, HTTP 400.
///
/// It is NOT a modelled error — no operation in this model (nor in the legacy Amazon
/// Connect Wisdom view of the same endpoint) declares it
/// (`smithy_query(query="operation", operation="CreateAssistant")`:
/// `AccessDeniedException`, `ConflictException`, `ServiceQuotaExceededException`,
/// `ValidationException`) — because the refusal is made where the request body is read
/// against the operation's input schema, in front of the operation's own validation. TWO
/// classes reach it, both recorded for this endpoint:
///
/// * a value outside a modelled enumeration's value set, which the input shape cannot hold
///   at all;
/// * a body value outside a modelled `@length` bound.
///
/// Both are answered with the one sentence [`INVALID_REQUEST_BODY_MESSAGE`] states — the
/// recorded wire answer for each of them.
///
/// Every other refusal class keeps the error its model declares: a value outside a
/// `@pattern`, an absent `@required` member, a value outside a `@range` and a `@length`
/// refusal of a value that travelled in the query string are all the modelled
/// `ValidationException`, with the constraint-stating sentence AWS words it with.
///
/// The legacy Amazon Connect Wisdom view of this endpoint
/// (`services/service-com-amazonaws-wisdom-wisdomservice/src/service.rs`) answers under its
/// own registration key rather than the `wisdom` endpoint identity this view serves, and
/// words its own refusals from its own constants.
///
/// The code itself is owned by `crate::scaffolded::BAD_REQUEST_CODE`, so the class a
/// handler refuses through `crate::scaffolded::Scaffolded::bad_request` and the class the
/// boundary catches here carry ONE code rather than drifting apart.
const BODY_REFUSAL_CODE: &str = crate::scaffolded::BAD_REQUEST_CODE;

/// The sentence this endpoint refuses a body the boundary in front of the service will not
/// read with. It carries NOTHING from the request — no value, member name or bound — which
/// is what the recorded wire answers of this endpoint for both of that boundary's classes
/// carry.
///
/// The sentence itself is the workspace's ONE copy of it
/// (`service_common::front_door::INVALID_REQUEST_BODY`), which several AWS REST services
/// answer a body their front door will not read with, so it cannot drift per crate. What
/// is this service's own is only WHICH classes are refused there — see
/// [`refuses_the_request_body`] — and the legacy Amazon Connect Wisdom view of this
/// endpoint words the enumeration class from the same shared constant.
const INVALID_REQUEST_BODY_MESSAGE: &str = ::service_common::front_door::INVALID_REQUEST_BODY;

/// The member names this model binds to the request QUERY STRING whose values a modelled
/// `@length` bound can refuse: the continuation tokens
/// (`smithy_query(query="operation",operation="ListKnowledgeBases,GetRecommendations,GetNextMessage")`
/// gives `nextToken`, `nextChunkToken` and `nextMessageToken` as `@httpQuery` members of
/// `NextToken`/`NonEmptyString`, both `len 1..`).
///
/// A length refusal of one of these is NOT a refusal of the request BODY — the value
/// travelled in the URI, which the body check never reads — so it keeps the modelled
/// `ValidationException` this service declares for input it validates itself. Every other
/// member a `@length` bound can refuse in this model is a body member: a path label is
/// refused by its `@pattern` and a query integer by its `@range`, and neither of those
/// classes is answered here.
///
/// Matched by NAME because this seam is handed the violation alone, never the operation
/// whose schema bound the member; the one place that costs anything is `QueryAssistant`,
/// which carries `nextToken` in its body, where a length refusal of it therefore also
/// keeps the modelled error. See [`refuses_the_request_body`].
const QUERY_TOKEN_MEMBERS: [&str; 3] = ["nextToken", "nextChunkToken", "nextMessageToken"];

/// Whether these violations include a refusal of the request BODY by the boundary that
/// reads it against the operation's input schema: a value outside a modelled
/// enumeration's value set, which the input shape cannot hold at all, or a `@length`
/// violation on a member the request did not carry in its query string
/// ([`QUERY_TOKEN_MEMBERS`]).
///
/// `any`, not `all`: the body is checked against the operation's input schema BEFORE the
/// service validates anything, so a request whose body that check rejects is answered
/// there whatever else the same request also gets wrong — which keeps the answer
/// independent of the order the members happen to be walked in.
///
/// Only the first path segment is looked at: a violation inside a structure, a list
/// element or a map entry (`searchExpression.filters.1.member.field`) is a violation of
/// the body member that carries it, and no query parameter has members of its own.
fn refuses_the_request_body(violations: &[smithy_server_runtime::Violation]) -> bool {
    violations.iter().any(|violation| {
        let member = violation
            .path
            .split('.')
            .next()
            .unwrap_or(violation.path.as_str());
        match violation.kind {
            // An enumeration's value set is part of the input SHAPE, so the refusal is the
            // boundary's wherever the value travelled.
            ViolationKind::EnumValue(_) => true,
            ViolationKind::MinLength(_) | ViolationKind::MaxLength(_) => {
                !QUERY_TOKEN_MEMBERS.contains(&member)
            }
            _ => false,
        }
    })
}

/// Every member of this model that declares `@timestampFormat("date-time")`, by the
/// absolute member id its generated schema carries.
///
/// The members are the message-template family's instants —
/// `smithy_query(query="shape",shape="MessageTemplateData,ExtendedMessageTemplateData,MessageTemplateSummary,MessageTemplateSearchResultData,MessageTemplateAttachment")`
/// reports `"timestampFormat": "date-time"` on each one. Every OTHER timestamp member of
/// this model declares `epoch-seconds` (the quick-response, import-job, content and
/// content-upload instants) or declares no format at all, and each of those keeps the form
/// the protocol gives it — which is why the correction is named per member here instead of
/// being applied to every format-less timestamp.
///
/// `tests/all/message_template_wire_form.rs` re-derives this list from the model file and
/// fails if a member is missing from it or named wrongly.
pub const DATE_TIME_MEMBERS: &[&str] = &[
    "com.amazonaws.qconnect#ExtendedMessageTemplateData$createdTime",
    "com.amazonaws.qconnect#ExtendedMessageTemplateData$lastModifiedTime",
    "com.amazonaws.qconnect#MessageTemplateAttachment$uploadedTime",
    "com.amazonaws.qconnect#MessageTemplateAttachment$urlExpiry",
    "com.amazonaws.qconnect#MessageTemplateData$createdTime",
    "com.amazonaws.qconnect#MessageTemplateData$lastModifiedTime",
    "com.amazonaws.qconnect#MessageTemplateSearchResultData$createdTime",
    "com.amazonaws.qconnect#MessageTemplateSearchResultData$lastModifiedTime",
    "com.amazonaws.qconnect#MessageTemplateSummary$createdTime",
    "com.amazonaws.qconnect#MessageTemplateSummary$lastModifiedTime",
];

/// This model's OWN sdkId-derived key (`sdkId: QConnect`,
/// `smithy_query(query="service_info")`), beside the shared endpoint identity
/// [`crate::ROUTE_KEY`] names.
///
/// A client addressed at Amazon Q in Connect by its product name — a credential scope or
/// an endpoint host saying `qconnect` — reaches this service too, not only one signing
/// `wisdom`.
const OWN_SDK_ID_KEY: &str = "qconnect";

/// Whether the request these `parts` came from addresses THIS service: its route key is
/// the shared `wisdom` endpoint identity ([`crate::ROUTE_KEY`]) or this model's own
/// [`OWN_SDK_ID_KEY`], AND its first URI path segment is one this model publishes
/// (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here per key rather than re-implemented. It takes a
/// whole `http::Request` while [`Service::can_handle`] is handed only the request's
/// parts, so the two things it reads — the URI and the headers — are re-presented to it
/// as a bodiless request. It reads neither the body nor the method, so nothing it decides
/// on is missing. (`services/common`'s `claims_request` states the same rule over
/// `Parts`; it is not used here only because depending on it edits `Cargo.lock`, which
/// this repository's serialized squash merge refuses whenever another change lands
/// against it — migrating these two crates onto it is a one-line follow-up.)
fn claims_endpoint_traffic(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    [crate::ROUTE_KEY, OWN_SDK_ID_KEY].iter().any(|key| {
        emulator_server::can_handle_based_on_route_key_and_path(
            &request,
            key,
            crate::ROUTE_PATH_SEGMENTS,
        )
    })
}

/// The `com-amazonaws-qconnect-wisdomservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// `CreateSession`'s authorization decision, which is made BEFORE the framework
    /// walk validates the request — see [`Service::override_operation`] below and the
    /// type's own docs in `crate::ops::create_session`. Held here because
    /// `override_operation` hands back a borrowed operation, so the one it may name has
    /// to live as long as the service.
    create_session_authorization: crate::ops::create_session::CreateSessionAuthorization<Eng>,
    /// The three data planes this service publishes URLs into — the content-upload slot,
    /// a content's asset and a message-template attachment's file — as the one
    /// author-written operation that serves them. Held here for the same reason
    /// `create_session_authorization` is: [`Service::override_operation`] hands back a
    /// borrowed operation, so the one it may name has to live as long as the service.
    content_data_plane: data_plane::ContentDataPlane<Eng>,
    /// This service's wire protocol: `restJson1`, with the timestamp members this model
    /// declares `@timestampFormat("date-time")` on ([`DATE_TIME_MEMBERS`]) carried in that
    /// declared form.
    ///
    /// The JSON codec resolves a timestamp's wire form from the member schema alone and the
    /// generated schemas carry no member-level format, so the declared one is supplied at
    /// this seam — the workspace's one place that correction lives
    /// (`service_common::date_time_json`). Held here because
    /// [`Service::override_protocol`] hands back a borrow, so the protocol is built once,
    /// at construction, and never per request.
    protocol: ::service_common::date_time_json::DateTimeRestJson,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because [`Service::can_handle`] decides whether this view holds the
    /// upload, content or attachment a request's path names, and that seam is not an
    /// operation with a `self.engine` of its own; it must read the one store
    /// `StartContentUpload` writes, not a second copy.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            create_session_authorization:
                crate::ops::create_session::CreateSessionAuthorization::new(Arc::clone(&engine)),
            content_data_plane: data_plane::ContentDataPlane::new(Arc::clone(&engine)),
            protocol: ::service_common::date_time_json::DateTimeRestJson::for_members(
                DATE_TIME_MEMBERS,
            ),
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
        }
    }

    /// Whether this request FOLLOWS an upload URL this service handed out: its path is
    /// one the upload plane publishes and it names an upload THIS store holds.
    ///
    /// The id in the path is what makes the CLAIM unambiguous: it was minted by this
    /// store, so the legacy Amazon Connect Wisdom view of this one endpoint — which mints
    /// its upload ids into its own store — claims only the URLs it handed out, and exactly
    /// the view that handed a URL out claims a request following it, whatever endpoint host
    /// or credential scope that request carries. What ADMITS the request is the signature
    /// in the URL's own query, which the router verifies before the plane runs (see
    /// `data_plane`); this decides only which service answers. Claiming every path of
    /// this shape instead would make both views claim it and the router refuse the
    /// request as `500 AmbiguousService`; the cost is that an upload URL this emulator
    /// never handed out is claimed by nobody and gets the router's own unknown-endpoint
    /// refusal.
    fn holds_content_upload(&self, target: &crate::state::ContentUploadTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.content_upload(&target.upload_id).is_some()
    }

    /// Whether this request FOLLOWS an attachment URL this service handed out: its path is
    /// one the attachment plane publishes and it names an attachment THIS store holds — on
    /// the template's working copy or in a version it published, which is every record a
    /// locator this service handed out can still address.
    ///
    /// The same rule as [`Self::holds_content_upload`], and for the same reason: the
    /// minted attachment id in the path was minted by THIS store, so exactly the view that
    /// handed the URL out claims a request following it.
    fn holds_message_template_attachment(
        &self,
        target: &crate::state::MessageTemplateAttachmentTarget,
    ) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        let knowledge_base_id = crate::state::resolve_id(&target.knowledge_base_label);
        let message_template_id = crate::state::resolve_id(&target.message_template_label);
        store
            .stored_message_template_attachment(
                knowledge_base_id,
                message_template_id,
                &target.attachment_id,
            )
            .is_some()
    }

    /// Whether this request FOLLOWS a content-asset URL this service handed out: its path
    /// is one the asset plane publishes and it names a content THIS store holds.
    ///
    /// The same rule as [`Self::holds_content_upload`], and for the same reason: the id in
    /// the path was minted by THIS store, so exactly the view that handed the URL out
    /// claims a request following it.
    fn holds_content(&self, target: &crate::state::ContentAssetTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        let knowledge_base_id = crate::state::resolve_id(&target.knowledge_base_label);
        store.content(knowledge_base_id, &target.content_id).is_some()
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
        // This model is Amazon Q in Connect, and the endpoint identity it publishes is
        // NOT its own name: `smithy_query(query="service_info")` gives it sdkId
        // `QConnect` but sigv4Name/arnNamespace `wisdom` and the endpoint templates
        // `https://wisdom.{Region}.{dnsSuffix}` — AWS serves Q in Connect on Amazon
        // Connect Wisdom's endpoint and signing name. The repo carries that endpoint's
        // other model too, `services/service-com-amazonaws-wisdom-wisdomservice` (sdkId
        // `Wisdom`, the same service shape `WisdomService` 2020-10-19, the same
        // `restJson1` protocol, the same signing name), whose 41 operations are a strict
        // SUBSET of this model's 94 AT THE IDENTICAL METHOD+URI ROUTES. No signal in a
        // request can tell the two apart, and restJson1 carries no `X-Amz-Target`, so the
        // `wisdom` key has to be served by exactly ONE of them or the router refuses
        // every request to the endpoint as `500 AmbiguousService` — a code that exists in
        // no AWS model.
        //
        // It is served HERE: this model is the endpoint's current API and the superset,
        // so a client signing `wisdom` loses no operation (the legacy model's 41 routes
        // are all bound here), while serving it from the subset model would leave this
        // model's other 53 routes — every AI agent, AI prompt, AI guardrail, message
        // template, content association and session message operation — answerable by
        // nobody. The legacy crate answers under its own registration key instead and
        // contends for nothing. See `claims_endpoint_traffic`.
        //
        // A request that FOLLOWS an upload URL this service handed out is claimed by the
        // store that minted it, whatever else the request carries: its credential travels
        // in the URL's own query rather than in a header, and its endpoint host may name
        // no service at all, so the route key cannot route it. The upload path binds no
        // modelled operation, so nothing modelled is diverted here. See
        // [`Self::holds_content_upload`].
        if let Some(target) = crate::state::parse_content_upload_path(parts.uri.path()) {
            return self.holds_content_upload(&target);
        }
        // A request that follows a content-asset URL this service handed out is claimed
        // the same way, by the store that minted the content id in its path. That path
        // binds no modelled operation either — it is one segment deeper than the modelled
        // read of the content — so nothing modelled is diverted here.
        if let Some(target) = crate::state::parse_content_asset_path(parts.uri.path()) {
            return self.holds_content(&target);
        }
        // And so is one that follows a message-template attachment URL, by the store that
        // minted the attachment id in its path. That path is one segment deeper than the
        // modelled route for the attachment, so nothing modelled is diverted here.
        if let Some(target) =
            crate::state::parse_message_template_attachment_path(parts.uri.path())
        {
            return self.holds_message_template_attachment(&target);
        }
        claims_endpoint_traffic(parts)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own `restJson1`, with one correction: a timestamp member this model
        // declares `@timestampFormat("date-time")` on reaches the wire as the RFC 3339
        // STRING that format names rather than as the protocol's default epoch-seconds
        // number. Every other timestamp member keeps the protocol's own form, which for
        // this model's `epoch-seconds` members is the form they declare. See
        // [`DATE_TIME_MEMBERS`] and the field this borrows.
        Some(&self.protocol)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // This seam is ALSO the only point in front of the framework walk, which is
        // deserialize → validate → `handle`: an operation whose answer must not depend
        // on whether the request's members validate has to be decided here, because by
        // the time a handler runs the walk has already refused a request the model's
        // constraints reject with 400 `ValidationException`.
        //
        // ONE class needs that: an AUTHORIZATION refusal. A caller this service cannot
        // authorize for the resource a request's path names is refused for permissions
        // whatever else that request carries — a path label outside its `@pattern`, a
        // body member outside a `@length`, an absent `@required` member — so the
        // authorization decision is evaluated before member validation and its modelled
        // error is what the caller sees. `CreateSession` is the one operation this
        // service decides authorization for (it is the only one that answers the
        // modelled `UnauthorizedException`, against the parent assistant a session lives
        // in); every other operation resolves its resource instead and answers
        // `ResourceNotFoundException`, which is a resource decision and stays BEHIND
        // validation, so nothing else is routed here.
        //
        // The routing key is resolved through the generated table's own protocol, so
        // this names no route pattern of its own.
        //
        // The data planes are named here too, and for a different reason: a request that
        // FOLLOWS one of the URLs this service hands out binds no modelled operation, so
        // the routing table resolves nothing for it. Naming the plane operation here —
        // rather than answering the request from `override_routing_error`, which the
        // router serves BEFORE it authenticates — is what puts those paths behind the
        // emulator's own signature verification. See `data_plane`.
        let path = request_path(req.uri());
        if data_plane::ContentDataPlane::<Eng>::serves(path) {
            return Some(&self.content_data_plane as &dyn ErasedOperation);
        }
        let routing_key = self.definition().protocol().routing_key(req).ok()?;
        self.create_session_authorization
            .refuses(routing_key, req)
            .then_some(&self.create_session_authorization as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it; `None` keeps the framework's default (400 `ValidationException`,
        // which is also what this service's model declares for a constraint it
        // validates itself).
        //
        // Two classes are refused by the boundary in front of the service instead, with
        // the code [`BODY_REFUSAL_CODE`] names and the one sentence
        // [`INVALID_REQUEST_BODY_MESSAGE`] states: a value outside a modelled enumeration's
        // value set, which the input shape cannot hold at all, and a body value outside a
        // modelled `@length` bound. Neither answer carries anything from the request — the
        // offending value, the member name and the bound or value set are all absent from
        // it — which is what the recorded wire answers of this endpoint for those classes
        // carry.
        //
        // Every other class — a `@pattern`, an absent `@required` member, a `@range`, and
        // a length refusal of a value that travelled in the query string — is stated by
        // the workspace's ONE renderer of an AWS constraint refusal,
        // `service_common::aws_validation_refusal`: it keeps the framework's envelope, its
        // member paths and every other requirement, and restates only the two renderings
        // that are AWS's rather than the framework's — a `@pattern` quoted without the
        // whole-value anchors a model wraps the service's expression in, and a collection's
        // `@length` clause stated without a value half. The code and status stay the
        // framework's own for that class, 400 `ValidationException`, which is also what
        // this service's model declares for a constraint it validates itself.
        match refusal {
            Refusal::Invalid(violations) if refuses_the_request_body(violations) => {
                Some(Refused {
                    code: BODY_REFUSAL_CODE.to_string(),
                    message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
                })
            }
            _ => ::service_common::aws_validation_refusal(refusal),
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
        // The data planes are NOT served here, although they name no modelled operation
        // either: the router answers this seam BEFORE it authenticates a request, so a
        // plane served from here would serve stored bytes to a request carrying no
        // credential. They are named from `override_operation` instead, which the router
        // resolves before it authenticates. See `data_plane`.
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

/// The body of an upload-plane refusal that found no upload: one sentence carrying
/// nothing from the request, so a caller cannot probe for uploads it was not handed.
const UPLOAD_NOT_FOUND_MESSAGE: &str = "The requested upload does not exist";

/// The body of an asset-plane refusal that found no content: the same rule as
/// [`UPLOAD_NOT_FOUND_MESSAGE`].
const ASSET_NOT_FOUND_MESSAGE: &str = "The requested content does not exist";

/// The body of an attachment-plane refusal that found no attachment: the same rule as
/// [`UPLOAD_NOT_FOUND_MESSAGE`].
const ATTACHMENT_NOT_FOUND_MESSAGE: &str = "The requested attachment does not exist";

/// The presentation the attachment plane serves a file under when the stored
/// `contentDisposition` is not text a header may carry: the lowercase form of the one
/// value the model's `ContentDisposition` declares. A constant of this module, so the
/// fallback is STATICALLY a legal header value and cannot unwind the request serving it.
const ATTACHMENT_DISPOSITION: &str = "attachment";

/// Whether a stored value can be written into a header: the visible-ASCII text a header
/// value carries, and neither of the two characters that would end the quoted string a
/// `filename` parameter is written in.
fn header_safe_text(value: &str) -> bool {
    value
        .chars()
        .all(|character| matches!(character, ' '..='~') && !matches!(character, '"' | '\\'))
}

/// The media type an asset is served as when the content's own stored media type is not
/// a string a header may carry: the generic binary type, which says only "these are
/// bytes" rather than claiming a type the asset is not.
const GENERIC_ASSET_CONTENT_TYPE: &str = "application/octet-stream";

/// The body of an upload-plane refusal of a write after the URL's `urlExpiry`.
const UPLOAD_EXPIRED_MESSAGE: &str = "Request has expired";

/// The body of an upload-plane refusal of a write that does not carry the headers the
/// start call published in `headersToInclude`.
const UPLOAD_HEADERS_MESSAGE: &str =
    "The request does not include the headers the upload was started with";

/// The body of an upload-plane refusal of a body larger than this emulator stores
/// (`crate::state::MAX_CONTENT_UPLOAD_BYTES`).
const UPLOAD_TOO_LARGE_MESSAGE: &str = "The upload is larger than this endpoint accepts";

/// The body of an upload-plane refusal of a write the store has no room for: the written
/// bodies it holds are bounded in aggregate as well as one by one
/// (`crate::state::MAX_PENDING_UPLOAD_BYTES`). It names no figure of the store and
/// carries nothing from the request, and it says what the caller can do about it.
const UPLOAD_STORE_FULL_MESSAGE: &str =
    "This endpoint is holding as many written upload bytes as it accepts; retry once an \
     upload has been consumed or has expired";

/// One upload-plane error response: the status, the code in `x-amzn-errortype`, and a
/// JSON body carrying the reason.
///
/// The upload plane is plain HTTPS rather than a modelled operation, so it answers in
/// its own small envelope instead of the service's restJson1 one. The message is built
/// from constants, never from the request.
fn upload_error(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message, "code": code }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/json");
    resp.headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    resp
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            // Authority only, no path at all.
            None => "/",
        },
        None => without_query,
    }
}
