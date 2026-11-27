// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-wisdom-wisdomservice` service: how it claims requests and how it words the
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
//! FOLLOWED. The URL it mints (`crate::state::content_upload_url`) is SigV4
//! query-presigned against the base URL this emulator is reached on, under the upload's own
//! path (`/knowledgeBases/<knowledgeBaseId>/uploads/<uploadId>`) — a path the model binds no
//! operation to, so it reaches this file twice:
//!
//! - [`Service::can_handle`] also claims a request to an upload path naming an upload
//!   THIS store holds, so following the URL routes back here even when nothing else in
//!   the request names a service (a presigned URL is followed with no `Authorization`
//!   header, and a loopback endpoint host names no service at all).
//! - [`Service::override_operation`] names [`ContentPlane`] as the operation that serves
//!   it. Naming an operation is what puts the request through the emulator's own front
//!   door, so the credential the presigned URL carries is verified — and a request
//!   carrying none is refused — before the plane runs. (The routing-error seam is reached
//!   BEFORE authentication, which is why the plane is not served from there.)
//!
//! The plane writes ONLY the upload the URL names — the bytes, bounded both per body
//! (`crate::state::MAX_CONTENT_UPLOAD_BYTES`) and across every written body this store
//! holds (`crate::state::MAX_PENDING_UPLOAD_BYTES`) — and refuses a `PUT` that is late (the
//! URL's `urlExpiry` has passed) or that does not carry the headers the start call
//! published. Nothing else about the service is reachable through it. The content-asset
//! location every read of a content reports is published, presigned and served the same
//! way, and it only READS.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service, ViolationKind};
use std::sync::Arc;

/// The error code this endpoint answers a value outside a modelled enumeration's value
/// set with: `BadRequestException`, HTTP 400.
///
/// It is NOT a modelled error — no operation in this model (nor in the Q Connect view of
/// the same endpoint) declares it (`smithy_query(query="operation",
/// operation="CreateAssistant")`: `AccessDeniedException`, `ConflictException`,
/// `ServiceQuotaExceededException`, `ValidationException`) — because the refusal happens
/// where the request body is read into the input shape, before the operation's own
/// validation: a `type` outside `AssistantType`'s value set, a `knowledgeBaseType` outside
/// `KnowledgeBaseType`'s, and so on are all answered with this one code, while a
/// constraint this service validates itself (an absent required member, a `@length` or
/// `@pattern` violation) keeps the modelled `ValidationException`.
///
/// The code itself is owned by `crate::scaffolded::BAD_REQUEST_CODE`, so the class a
/// handler refuses through `crate::scaffolded::Scaffolded::bad_request` and the class the
/// boundary catches here carry ONE code rather than drifting apart.
///
/// The Q Connect view of this SAME endpoint — which is the view the shared `wisdom` key
/// is served from (`services/service-com-amazonaws-qconnect-wisdomservice/src/service.rs`)
/// — words this class identically, from its own copy of this constant and of
/// `override_refusal`: the two are one endpoint, so a client must not get a different code
/// for the same input depending on which key it addressed. Change one and change the other.
const ENUM_VALUE_REFUSAL_CODE: &str = crate::scaffolded::BAD_REQUEST_CODE;

/// The sentence this endpoint refuses a body the boundary in front of the service will not
/// read with. It carries NOTHING from the request — no value, member name or value set —
/// which is what the recorded wire answer of this endpoint for that class carries, and it
/// is the whole message: the framework's `Value '…' at '…' failed to satisfy constraint:
/// Member must satisfy enum value set: […]` sentence belongs to a different validation
/// layer and is not what this endpoint answers.
///
/// The sentence itself is the workspace's ONE copy of it
/// (`service_common::front_door::INVALID_REQUEST_BODY`), which several AWS REST services
/// answer a body their front door will not read with, so it cannot drift per crate. What
/// is this service's own is only WHICH class is refused there, below.
///
/// The Q Connect view of this SAME endpoint words the enumeration class from the same
/// shared constant
/// (`services/service-com-amazonaws-qconnect-wisdomservice/src/service.rs`), so a client
/// gets the same sentence for that input whichever key it addressed. The two views are
/// aligned for THAT class only: the Q Connect view also answers a body value outside a
/// modelled `@length` bound there, from a recording of the shared endpoint identity it
/// serves, while this view still answers that class with the modelled `ValidationException`
/// — no recorded answer under THIS registration key covers it, and folding it in belongs
/// to the issue that owns that class rather than to this one.
const INVALID_REQUEST_BODY_MESSAGE: &str = ::service_common::front_door::INVALID_REQUEST_BODY;

/// The key THIS service answers under, which is NOT the `wisdom` key its model's
/// `aws.auth#sigv4` name resolves to.
///
/// This model is Amazon Connect Wisdom: `smithy_query(query="service_info")` gives it
/// sdkId `Wisdom`, sigv4Name `wisdom`, arnNamespace `wisdom`, service shape
/// `WisdomService` version 2020-10-19, protocol `restJson1`, 41 operations. The repo
/// carries a SECOND model of that same endpoint,
/// `services/service-com-amazonaws-qconnect-wisdomservice` (sdkId `QConnect` — Amazon Q
/// in Connect, the endpoint's current API), whose `service_info` differs ONLY in sdkId:
/// the same service shape and version, the same protocol, the same signing name, the same
/// endpoint templates `https://wisdom.{Region}.{dnsSuffix}`, and 94 operations that
/// INCLUDE all 41 of this model's at the identical method+URI routes. restJson1 carries
/// no `X-Amz-Target`, so a request is dispatched by method+URI — and those routes are the
/// same here as there, which means no signal in a request can tell the two models apart.
/// The `wisdom` key therefore has to be served by exactly ONE of them, or the router
/// (which requires exactly one claimant) answers `500 AmbiguousService` for every request
/// to the endpoint, a code that exists in no AWS model.
///
/// The QConnect crate serves it, because it is the endpoint's current API and the
/// superset: a client signing `wisdom` reaches every operation EITHER model declares,
/// which is also what AWS does — one service answers both APIs on `wisdom.{region}`.
/// Claiming `wisdom` here as well would reinstate the ambiguity.
///
/// This service therefore answers under its own REGISTRATION identity: this model's shape
/// id `com.amazonaws.wisdom#WisdomService` in the key form the project names its service
/// crates by (`service-com-amazonaws-wisdom-wisdomservice`). It is unique per registered
/// service by construction and is no AWS signing name, endpoint prefix or sdkId — which
/// is exactly why it cannot collide with the shared endpoint identity. The legacy model
/// stays mounted, reachable and exercised by its own suite without ever contending for
/// `wisdom`.
///
/// Read this key as ADMINISTRATIVE, not as a wire identity: no AWS SDK or CLI ever signs
/// or addresses `com-amazonaws-wisdom-wisdomservice`, because AWS publishes exactly one
/// identity for this endpoint and the QConnect model above serves it. Only a caller that
/// names this registered model deliberately — this crate's own suite, or an operator
/// probing one mounted service — reaches these handlers, and nothing is lost by that: all
/// 41 routes of this model are bound in the QConnect crate, which answers the endpoint's
/// real traffic.
const OWN_REGISTRATION_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// Whether the request these `parts` came from resolves to [`OWN_REGISTRATION_KEY`] AND a
/// first URI path segment this model publishes (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of that rule, and it is CALLED here rather than re-implemented, with this service's
/// own key in place of the signing name. It takes a whole `http::Request` while
/// [`Service::can_handle`] is handed only the request's parts, so the two things it reads
/// — the URI and the headers — are re-presented to it as a bodiless request. It reads
/// neither the body nor the method, so nothing it decides on is missing.
/// (`services/common`'s `claims_request` states the same rule over `Parts`; it is not
/// used here only because depending on it edits `Cargo.lock`, which this repository's
/// serialized squash merge refuses whenever another change lands against it — migrating
/// these two crates onto it is a one-line follow-up.)
fn claims_own_registration_key(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        OWN_REGISTRATION_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-wisdom-wisdomservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The data plane that serves a request FOLLOWING one of the URLs this service hands
    /// out — an upload destination or a content asset — named by
    /// [`Service::override_operation`] so the emulator's front door holds that request to
    /// the credential the presigned URL carries.
    ///
    /// It holds the SAME shared engine the generated entry threads into every operation,
    /// because neither [`Service::can_handle`] nor the plane itself is an operation with a
    /// `self.engine` of its own, and both must read the one store `StartContentUpload`
    /// writes rather than a second copy.
    content_plane: ContentPlane<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            content_plane: ContentPlane { engine },
        }
    }
}

/// The two data planes behind the URLs this service hands out: the upload destination
/// `StartContentUpload` publishes, and the asset location every read of a content reports.
///
/// Neither is a modelled operation — the model binds no route to either path — so the
/// service NAMES this plane for such a request ([`Service::override_operation`]), which is
/// what routes it through the emulator's own front door: the presigned credential the URL
/// carries is verified there, before anything is written or served.
///
/// It holds the SAME shared engine every operation of this service does, so the upload a
/// `PUT` writes is the one `CreateContent` finalizes.
struct ContentPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ContentPlane<Eng> {

    /// Whether this request FOLLOWS an upload URL this service handed out: its path is
    /// one the upload plane publishes and it names an upload THIS store holds.
    ///
    /// The id in the path is the receipt: it was minted by this store, is unguessable,
    /// and expires. That is what makes the claim UNAMBIGUOUS — the Amazon Q in Connect
    /// view of this one endpoint mints its upload ids into its own store, so exactly the
    /// view that handed a URL out claims a request following it, whatever endpoint host
    /// or credential scope that request carries. Claiming every path of this shape
    /// instead would make both views claim it and the router refuse the request as
    /// `500 AmbiguousService`; the cost is that an upload URL this emulator never handed
    /// out is claimed by nobody and gets the router's own unknown-endpoint refusal.
    fn holds_content_upload(&self, target: &crate::state::ContentUploadTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.content_upload(&target.upload_id).is_some()
    }

    /// Serve one request to a published upload URL.
    ///
    /// A `PUT` writes the asset into the upload the URL names — which is the only thing
    /// the plane can touch — and is refused when the URL is no longer usable, when the
    /// request does not carry the headers the start call published, or when the body is
    /// larger than this emulator stores. The upload, the knowledge base it was started
    /// under and the id in the path must all agree: a path that crosses one knowledge
    /// base with another's upload names nothing.
    fn serve_content_upload(
        &self,
        req: &RtRequest,
        target: &crate::state::ContentUploadTarget,
    ) -> RtResponse {
        if req.method() != "PUT" {
            return upload_error(
                405,
                "MethodNotAllowed",
                "This upload URL is written by a single PUT",
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return upload_error(
                500,
                "InternalServerException",
                "the wisdom content-upload store is unavailable",
            );
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let resolved_knowledge_base = store.resolve_knowledge_base_id(&target.knowledge_base_label);
        let names_this_upload = store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| {
                resolved_knowledge_base.as_deref() == Some(upload.knowledge_base_id.as_str())
            });
        if !names_this_upload {
            return upload_error(404, "NoSuchUpload", UPLOAD_NOT_FOUND_MESSAGE);
        }
        // The URL stops being usable at its `urlExpiry`, so a late PUT is refused — and
        // the slot it named is reclaimed here, because the clock already owed it back.
        if store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| upload.has_expired(now))
        {
            store.reclaim_expired_content_uploads(now);
            return upload_error(403, "AccessDenied", UPLOAD_EXPIRED_MESSAGE);
        }
        // `headersToInclude` is what the start call told the caller to send, so a write
        // that does not carry them is refused rather than stored.
        let headers_satisfied = store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| {
                upload.headers_satisfied(|name| {
                    req.headers().get(name).map(::std::string::ToString::to_string)
                })
            });
        if !headers_satisfied {
            return upload_error(403, "AccessDenied", UPLOAD_HEADERS_MESSAGE);
        }
        let body = req.body().bytes().unwrap_or_default();
        if body.len() > crate::state::MAX_CONTENT_UPLOAD_BYTES {
            return upload_error(413, "EntityTooLarge", UPLOAD_TOO_LARGE_MESSAGE);
        }
        // The written bodies this store holds are bounded in AGGREGATE as well as one by
        // one (`crate::state::MAX_PENDING_UPLOAD_BYTES`), so a caller that opens many
        // uploads cannot hold the product of the two ceilings in this process's heap. A
        // write there is no room for stores nothing: the slot keeps the body it had, and
        // each outcome keeps its own answer.
        match store.write_content_upload_body(&target.upload_id, body.to_vec()) {
            crate::state::UploadWrite::Written => {}
            crate::state::UploadWrite::NoRoom => {
                return upload_error(413, "EntityTooLarge", UPLOAD_STORE_FULL_MESSAGE);
            }
            crate::state::UploadWrite::NoSuchUpload => {
                // Unreachable: the lookups above found it while this lock is held.
                return upload_error(404, "NoSuchUpload", UPLOAD_NOT_FOUND_MESSAGE);
            }
        }
        let Some(upload) = store.content_upload_mut(&target.upload_id) else {
            // Unreachable: the lookups above found it while this lock is held.
            return upload_error(404, "NoSuchUpload", UPLOAD_NOT_FOUND_MESSAGE);
        };
        upload.received_at = Some(now);

        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            SdkBody::empty(),
        );
        resp.headers_mut().insert("content-length", "0");
        resp
    }

    /// Whether this request FOLLOWS a content-asset URL this service handed out: its path
    /// is one the asset plane publishes and it names a content THIS store holds.
    ///
    /// The same rule as [`Self::holds_content_upload`], and for the same reason: the id in
    /// the path is the receipt, so exactly the view that handed the URL out claims a
    /// request following it.
    fn holds_content(&self, target: &crate::state::ContentAssetTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store
            .resolve_knowledge_base_id(&target.knowledge_base_label)
            .and_then(|knowledge_base_id| store.content(&knowledge_base_id, &target.content_id))
            .is_some()
    }

    /// Serve one request to a published content-asset URL.
    ///
    /// A `GET` answers the bytes `CreateContent` finalized into the content the URL names
    /// — which is the only thing the plane can touch, and it changes nothing — and is
    /// refused when the content's `urlExpiry` has passed. The content, the knowledge base
    /// it lives in and the id in the path must all agree: a path that crosses one
    /// knowledge base with another's content names nothing.
    fn serve_content_asset(
        &self,
        req: &RtRequest,
        target: &crate::state::ContentAssetTarget,
    ) -> RtResponse {
        if req.method() != "GET" {
            return upload_error(
                405,
                "MethodNotAllowed",
                "This content URL is read by a single GET",
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return upload_error(
                500,
                "InternalServerException",
                "the wisdom content store is unavailable",
            );
        };
        let now = ::std::time::SystemTime::now();
        let store = state.lock();
        let content = store
            .resolve_knowledge_base_id(&target.knowledge_base_label)
            .and_then(|knowledge_base_id| store.content(&knowledge_base_id, &target.content_id));
        let Some(content) = content else {
            return upload_error(404, "NoSuchKey", ASSET_NOT_FOUND_MESSAGE);
        };
        // The URL stops being usable at the `urlExpiry` every read of the content
        // reports, so a late GET is refused rather than served: what the plane honours and
        // what a read publishes are the one stored instant.
        if content.has_expired_url(now) {
            return upload_error(403, "AccessDenied", UPLOAD_EXPIRED_MESSAGE);
        }
        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            SdkBody::from(content.body.clone()),
        );
        // The media type is a STORED value that came from a request, so it is written
        // with the fallible insert: a stored string no header may carry (a byte outside
        // visible ASCII, a line break) falls back to the generic binary type rather than
        // unwinding this request. The length is this process's own number.
        if resp
            .headers_mut()
            .try_insert("content-type", content.content_type.clone())
            .is_err()
        {
            resp.headers_mut()
                .insert("content-type", GENERIC_ASSET_CONTENT_TYPE);
        }
        resp.headers_mut()
            .insert("content-length", content.body.len().to_string());
        resp
    }
}

/// The plane runs as the OPERATION the service named for the request, which is what the
/// front door authenticated before it got here: a `PUT` to an upload destination and a `GET`
/// of a content asset both carry the credential their presigned URL was minted with.
///
/// No `auth_schemes` answer of its own, so this plane declares nothing about its own
/// authentication and the emulator's front door holds the request to the credential the
/// announced URL carries — a presigned request carries it in the QUERY rather than in an
/// `Authorization` header, and the emulator verifies every request signature itself.
impl<Eng: Send + Sync + 'static> ErasedOperation for ContentPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> smithy_server_runtime::BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let path = request_path(req.uri());
            if let Some(target) = crate::state::parse_content_upload_path(path) {
                return self.serve_content_upload(req, &target);
            }
            if let Some(target) = crate::state::parse_content_asset_path(path) {
                return self.serve_content_asset(req, &target);
            }
            // Unreachable: the service names this plane only for a request on one of those
            // two paths, and the reading of the path is the same one.
            upload_error(404, "NoSuchKey", ASSET_NOT_FOUND_MESSAGE)
        })
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
        // Delegated to the framework's own predicate — see
        // [`claims_own_registration_key`]: this model is the Amazon Connect Wisdom view of
        // the endpoint Amazon Q in Connect now serves, publishing the very same wire
        // identity (signing name, endpoint, service shape, version, protocol) and a
        // strict subset of its routes, so the generated `wisdom` route key cannot tell
        // the two apart. It is claimed by the QConnect crate — the endpoint's current API
        // and the superset — and this service answers under its own registration key
        // instead, so exactly one service claims every request.
        // A request that FOLLOWS an upload URL this service handed out is claimed by
        // the store that minted it, whatever else the request carries: its credential
        // travels in the URL's own query rather than in a header, and its endpoint host
        // may name no service at all, so the route key cannot route it. The upload path
        // binds no modelled operation, so nothing modelled is diverted here. See
        // [`ContentPlane::holds_content_upload`].
        if let Some(target) = crate::state::parse_content_upload_path(parts.uri.path()) {
            return self.content_plane.holds_content_upload(&target);
        }
        // A request that follows a content-asset URL this service handed out is claimed
        // the same way, by the store that minted the content id in its path. That path
        // binds no modelled operation either — it is one segment deeper than the modelled
        // read of the content — so nothing modelled is diverted here.
        if let Some(target) = crate::state::parse_content_asset_path(parts.uri.path()) {
            return self.content_plane.holds_content(&target);
        }
        claims_own_registration_key(parts)
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
        // The two planes behind the URLs this service hands out are exactly that case by
        // construction: the model binds no operation to either path (see
        // `crate::state::content_upload_path` and `crate::state::content_asset_path`), so
        // the request would otherwise name no operation at all. Naming the plane that
        // serves it HERE is what puts the request through the emulator's own front door,
        // where the credential the presigned URL carries is verified before a byte is
        // written or served — a plane served from the routing-error seam instead is
        // reached BEFORE authentication, which would serve a caller presenting no
        // credential at all.
        //
        // The PATH alone decides, so a URL naming an upload or a content this store no
        // longer holds is answered by the plane as the thing it does not hold rather than
        // by the router as a request naming no operation.
        let path = request_path(req.uri());
        if crate::state::parse_content_upload_path(path).is_some()
            || crate::state::parse_content_asset_path(path).is_some()
        {
            return Some(&self.content_plane);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it; `None` keeps the framework's default (400 `ValidationException`,
        // which is also what this service's model declares for a constraint it
        // validates itself, so every other class is left alone).
        //
        // The ONE class this service states differently is a value outside a modelled
        // enumeration's value set: that is refused where the request body is read into
        // the operation's input shape, and this endpoint answers it 400
        // `BadRequestException` — a code neither this model nor the Q Connect view
        // declares as an operation error, because it is the deserialization boundary's,
        // not a modelled failure — with the one fixed sentence
        // [`INVALID_REQUEST_BODY_MESSAGE`] states. Nothing from the request travels in it:
        // the refusal is "this body cannot be read", not a report of which member failed
        // which constraint.
        //
        // `any`, not `all` or "the first": a request that ALSO breaks a constraint this
        // service validates itself could not be read into the input shape either, so the
        // boundary's refusal is the one the caller gets, whatever else the same request
        // gets wrong. That keeps the answer independent of the order the members happen to
        // be walked in.
        // Every other class is stated by the workspace's ONE renderer of an AWS constraint
        // refusal, `service_common::aws_validation_refusal`: it keeps the framework's
        // envelope, its member paths and every other requirement, and restates only the
        // two renderings that are AWS's rather than the framework's — a `@pattern` quoted
        // without the whole-value anchors a model wraps the service's expression in, and a
        // collection's `@length` clause stated without a value half. The code and status
        // stay the framework's own there, 400 `ValidationException`, which is also what
        // this service's model declares for a constraint it validates itself.
        match refusal {
            Refusal::Invalid(violations)
                if violations
                    .iter()
                    .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_))) =>
            {
                Some(Refused {
                    code: ENUM_VALUE_REFUSAL_CODE.to_string(),
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
        // The two planes behind the URLs this service hands out are NOT served here: this
        // seam is reached BEFORE authentication, so anything served from it is served to a
        // caller presenting no credential at all. They are named as this service's own
        // operation instead ([`Self::override_operation`]), which is what holds a request
        // following one of those URLs to the credential the URL carries.
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

