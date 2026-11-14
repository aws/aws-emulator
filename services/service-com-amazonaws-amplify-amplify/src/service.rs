// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-amplify-amplify` service: how it claims requests and how it words the
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
//! ## The deployment-upload plane
//!
//! `CreateDeployment` hands out upload destinations, and a URL is a promise that it can
//! be FOLLOWED: the documented manual-deployment contract is that the caller uploads
//! the site to the URL that call returned and then calls `StartDeployment` with the
//! `jobId` it returned. The destinations it mints
//! (`crate::state::deployment_upload_url`) therefore address the endpoint the reserving
//! caller reached, under the reservation's own path
//! (`/apps/<appId>/deployments/<jobId>/uploads/<token>`) — a path the model binds no
//! operation to, so it reaches this file twice:
//!
//! - [`Service::can_handle`] also claims a request to an upload path naming a
//!   reservation THIS store holds, so following the URL routes back here even when
//!   nothing else in the request names a service (an upload URL is followed WITHOUT a
//!   signature, as a presigned URL is, and a loopback endpoint host names no service at
//!   all).
//! - [`Service::override_routing_error`] serves it: that seam is reached BEFORE
//!   authentication precisely for requests that name no modelled operation, which is
//!   what makes an unsigned `PUT` to a minted URL work.
//!
//! The plane writes ONLY the one upload the URL names — the bytes, bounded per
//! reservation by `crate::ops::start_deployment::MAX_ARCHIVE_BYTES` and across the whole
//! store by `crate::state::MAX_STORED_DEPLOYMENT_BYTES` — onto the reservation, which is
//! where `StartDeployment` reads what a reserved deployment deploys. It refuses a write
//! whose reservation's window has closed (the 8 hours between `CreateDeployment` and
//! `StartDeployment` that `crate::state::DEPLOYMENT_WINDOW_MILLIS` carries). Nothing else
//! about the service is reachable through it.
//!
//! ## The hosting edge
//!
//! A deployed branch is VISITED, at the default domain the control plane reports for it:
//! `Branch$displayName` "is used as the default domain prefix" and `App$defaultDomain`
//! is "the default domain for the Amplify app", so the site of a branch answers at
//! `<displayName>.<defaultDomain>`. An app announced under Amplify's own hosting zone is
//! therefore addressed by a HOST whose first label names a branch rather than a service, so
//! the framework resolves no service from it and nothing would claim the request; an app
//! announced on this emulator's OWN endpoint (`crate::state::announced_default_domain`) is
//! addressed by the PATH this plane serves its sites under, which names the app and the
//! branch and binds no modelled operation (`crate::state::hosted_site_path`). Either way [`Service::can_handle`] claims a request
//! that addresses a branch THIS store holds, and [`Service::override_routing_error`]
//! answers it with the files that branch's latest successful deployment published
//! (`crate::state::AmplifyBranch::site`).
//!
//! The edge READS: a request for a path the site carries is answered with those bytes
//! and that file's own media type, a request for the root (or for a directory) with the
//! site's root document, and anything else with a 404. Before any of that, the branch's
//! ACCESS CONTROL decides whether the request is served at all — a branch whose (or whose
//! app's) `enableBasicAuth` is set is served only to a request presenting the configured
//! `basicAuthCredentials`, and every other request is answered with a 401 and the `Basic`
//! challenge that asks for them (`crate::state::basic_auth`) — and the request path is
//! resolved against the owning app's rewrite and redirect rules (`App$customRules`),
//! which is what makes a rewritten path serve the object its target names
//! (`crate::state::resolve_hosted_request`). A served file carries the headers the app's
//! `App$customHeaders` rules state for its path (`crate::state::custom_headers`). The
//! only state the edge touches is the settling the clock already owes the branch
//! (`crate::state::AmplifyBranch::settle`), which publishes the site of a deployment
//! whose job the clock has finished.
//!
//! ## How an answer that carries no content is framed
//!
//! `UntagResource` answers `204 No Content` (see `SUCCESS_STATUS` in
//! `src/ops/untag_resource.rs`): its output declares a STRUCTURE with no members rather
//! than `smithy.api#Unit`
//! (`smithy_query(query="shape",shape="UntagResourceResponse")`), and the protocol's
//! serializer renders such a structure as the empty JSON object `{}` — a body on a
//! response that has none, which RFC 9110 §15.3.5 gives a 204 no room for.
//!
//! [`Service::override_protocol`] below therefore serves a protocol that is the model's
//! own in every respect except one: an answer this service gives that status is handed
//! through `service_common::no_content_responses::without_content`, so it carries no body,
//! whatever the serializer rendered for it, and states the length it does have. A response
//! that carries content — every reader's, every create's, every other delete's and every
//! modelled failure's — is left exactly as the model's protocol renders it. It is done
//! here, at the one place every response of this service passes through, rather than per
//! operation.
//!
//! An answer served over HTTP/1.1 reaches the client without the framing header: the HTTP
//! writer the vendored transport uses drops `content-length` from a `204`, which is below
//! anything this crate decides.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use aws_smithy_types::body::SdkBody;
use service_common::no_content_responses::without_content;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// `refusal` with every enumerated value set stated in the order THIS service states
/// one, and everything else — the member paths, the values echoed, every other
/// requirement — untouched.
///
/// A refusal of a value outside an enumerated type names the values that type admits,
/// and the order it names them in belongs to the SERVICE rather than to the model: the
/// order a value set reaches this seam in is the order the model happens to declare that
/// enum's members in, which is an artefact of how the model was written. The sets this
/// service is recorded stating in an order of its own are listed in
/// [`STATED_VALUE_SET_ORDERS`]; every other set of this model keeps the order it arrived
/// in, because nothing shows this service stating it differently.
///
/// The CLAUSE stays the framework's — only the order inside it changes — so the member
/// path, the echoed value and the requirement's wording keep coming from the one place
/// that writes them.
fn in_stated_value_order(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(violations.iter().map(in_this_services_order).collect())
}

/// The enumerated value sets this service states in an order the model does not declare,
/// each written in the order its own refusals state it.
///
/// Source: the refusals recorded for this service. The order belongs to the TYPE — one
/// set, one order, whatever member or operation carried the value — so a set is
/// recognised by the values it admits and by nothing about the request.
const STATED_VALUE_SET_ORDERS: &[&[&str]] = &[
    // `com.amazonaws.amplify#Stage`, which the model declares `PRODUCTION, BETA,
    // DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST`
    // (`smithy_query(query="shape",shape="Stage")`).
    &[
        "EXPERIMENTAL",
        "BETA",
        "PULL_REQUEST",
        "PRODUCTION",
        "DEVELOPMENT",
    ],
    // `com.amazonaws.amplify#Platform`, which the model declares `WEB, WEB_DYNAMIC,
    // WEB_COMPUTE` (`smithy_query(query="shape",shape="Platform")`).
    &["WEB_COMPUTE", "WEB_DYNAMIC", "WEB"],
];

/// One violation with its enumerated value set in the order this service states it, or
/// the violation unchanged when it carries no value set this service publishes an order
/// for.
fn in_this_services_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    match stated_order(declared) {
        Some(stated) => Violation {
            kind: ViolationKind::EnumValue(stated),
            ..violation.clone()
        },
        None => violation.clone(),
    }
}

/// The order [`STATED_VALUE_SET_ORDERS`] states the set `declared` in, or `None` for a
/// set it does not name.
///
/// Matched on MEMBERSHIP and size, never on position, so a model revision that reordered
/// its own declaration of the same enum is still stated in the published order.
fn stated_order(declared: &[String]) -> Option<Vec<String>> {
    STATED_VALUE_SET_ORDERS
        .iter()
        .find(|stated| {
            stated.len() == declared.len()
                && stated
                    .iter()
                    .all(|value| declared.iter().any(|admitted| admitted == value))
        })
        .map(|stated| stated.iter().map(|value| (*value).to_string()).collect())
}

/// The `com-amazonaws-amplify-amplify` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the deployment-upload plane and the hosting edge are served
    /// from [`Service::can_handle`] and [`Service::override_routing_error`], neither of
    /// which is an operation with a `self.engine` of its own; both must read the one
    /// store `CreateDeployment` and `StartDeployment` wrote to, not a second copy.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
        }
    }

    /// Whether this request FOLLOWS an upload URL this service handed out: its path is
    /// one the upload plane publishes and it names a reservation THIS store holds.
    ///
    /// The token in the path is the receipt: it was minted by this store, is
    /// unguessable, and stops being usable when the reservation's window closes. An
    /// upload URL this emulator never handed out is claimed by nobody and gets the
    /// router's own unknown-endpoint refusal, which is what a destination that does not
    /// exist should answer.
    fn holds_deployment_upload(&self, target: &crate::state::DeploymentUploadTarget) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.holds_deployment_upload(target)
    }

    /// Serve one request to a published deployment-upload URL.
    ///
    /// A `PUT` writes the bytes into the one upload the URL names — the only thing this
    /// plane can touch — and is refused when the reservation's window has already
    /// closed or when the body is larger than this emulator reads from one deployment.
    fn serve_deployment_upload(
        &self,
        req: &RtRequest,
        target: &crate::state::DeploymentUploadTarget,
    ) -> RtResponse {
        if req.method() != "PUT" {
            return plane_error(
                405,
                "MethodNotAllowed",
                "This upload destination is written by a single PUT",
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return plane_error(
                500,
                "InternalFailureException",
                "the Amplify deployment-upload store is unavailable",
            );
        };
        // The body is what this plane STORES, so a request whose body this process
        // cannot read is refused rather than stored as an empty upload: answering 200 to
        // a write that kept nothing would tell the caller its site had landed and leave
        // `StartDeployment` to fail the job for an upload that never arrived.
        let Some(body) = req.body().bytes() else {
            return plane_error(400, "IncompleteBody", UPLOAD_UNREADABLE_MESSAGE);
        };
        let now = crate::state::now_millis();
        let mut store = state.lock();
        // What the whole store already holds, apart from the slot this write replaces:
        // the per-reservation clamp below bounds ONE deployment, and this bounds the
        // TOTAL kept in this process, so no number of reservations, branches or apps
        // grows it without limit.
        let stored = store.retained_deployment_bytes_apart_from_upload(&target.token);
        let Some(reservation) = store.reservation_of_upload_mut(target) else {
            return plane_error(404, "NoSuchKey", UPLOAD_NOT_FOUND_MESSAGE);
        };
        // The reservation's window closes [`crate::state::DEPLOYMENT_WINDOW_MILLIS`]
        // after it was created whether or not anybody called, so a write after it is
        // refused: the deployment it would feed can no longer be started.
        if reservation.has_expired(now) {
            return plane_error(403, "AccessDenied", UPLOAD_EXPIRED_MESSAGE);
        }
        // What ONE reservation may hold is bounded as a whole — by the bytes
        // `StartDeployment` reads from one deployment — because the content is caller
        // content kept in this process and how many files a reservation names is the
        // caller's own choice. Both bounds are applied BEFORE the body is kept.
        let held = reservation.uploaded_bytes_apart_from(&target.token);
        if held.saturating_add(body.len()) > crate::ops::start_deployment::MAX_ARCHIVE_BYTES {
            return plane_error(413, "EntityTooLarge", UPLOAD_TOO_LARGE_MESSAGE);
        }
        if stored.saturating_add(body.len()) > crate::state::MAX_STORED_DEPLOYMENT_BYTES {
            return plane_error(
                413,
                "EntityTooLarge",
                crate::state::STORED_DEPLOYMENT_BYTES_EXCEEDED,
            );
        }
        let Some(upload) = reservation.upload_mut(&target.token) else {
            // Unreachable: the lookup above found the reservation by this same token
            // while this lock is held.
            return plane_error(404, "NoSuchKey", UPLOAD_NOT_FOUND_MESSAGE);
        };
        // A destination is written by ONE caller and may be re-written until the
        // deployment is started, exactly as an object store's key is: the last write is
        // what the deployment deploys.
        upload.content = Some(body.to_vec());

        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            SdkBody::empty(),
        );
        resp.headers_mut().insert("content-length", "0");
        resp
    }

    /// Whether this request is addressed to a BRANCH's site — the hosting edge of a branch
    /// this store holds — rather than to this service's endpoint.
    fn hosts_a_branch(&self, address: &crate::state::HostedSiteAddress<'_>) -> bool {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return false;
        };
        let store = state.lock();
        store.hosts_a_branch(address)
    }

    /// Serve one request to a branch's hosting edge from what that branch's latest
    /// successful deployment published, as the owning app's rewrite and redirect rules
    /// resolve it.
    ///
    /// Before anything is served, the branch's ACCESS CONTROL decides: a branch (or an
    /// app) with basic authorization enabled is served only to a request presenting the
    /// configured credentials, and answers anything else with a 401 carrying the `Basic`
    /// challenge that asks for them.
    ///
    /// A `GET` (or a `HEAD`, which is the same answer without the body) is then answered
    /// by the first of the app's `customRules` whose `source` matches the path — a rewrite
    /// with the object its `target` names, a redirect with that target as the `Location`
    /// — and otherwise with the file the path names, the site's root document for the
    /// root or a directory, and a 404 for a path neither a rule nor the deployment
    /// answers, including every path of a branch nothing has been deployed to yet. Any
    /// other method is refused: a hosting edge serves a site, it does not take writes.
    ///
    /// A served file additionally carries the headers the app's `customHeaders` rules
    /// state for its path, which is what that document configures them for.
    fn serve_hosted_site(
        &self,
        req: &RtRequest,
        address: &crate::state::HostedSiteAddress<'_>,
        path: &str,
    ) -> RtResponse {
        let method = req.method();
        if method != "GET" && method != "HEAD" {
            return plane_error(405, "MethodNotAllowed", SITE_METHOD_MESSAGE);
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return plane_error(500, "InternalFailureException", SITE_STORE_MESSAGE);
        };
        let path = path.to_string();
        // The credentials the visitor presents are read from the request here, so the
        // store is handed the pair to verify and never the header.
        let presented =
            crate::state::presented_credentials(req.headers().get("authorization"));
        // The store is read, and what the clock owes the branch materialised, under the
        // lock; the answer is BUILT after it is released, so no response construction
        // happens while other callers are waiting on the one store.
        let visit = {
            let mut store = state.lock();
            match store.hosted_visit(address, &path, presented.as_deref()) {
                Some(visit) => visit,
                None => return plane_error(404, "NotFound", SITE_NOT_FOUND_MESSAGE),
            }
        };
        match visit.answer {
            crate::state::HostedAnswer::File { key, content } => {
                let length = content.len();
                let mut resp = RtResponse::new(
                    StatusCode::try_from(200u16).expect("200 is a valid status"),
                    // A HEAD is the same answer as the GET without the body, so the
                    // length it reports is the file's.
                    if method == "HEAD" {
                        SdkBody::empty()
                    } else {
                        SdkBody::from(content)
                    },
                );
                resp.headers_mut()
                    .insert("content-type", crate::state::hosted_file_media_type(&key));
                resp.headers_mut()
                    .insert("content-length", length.to_string());
                // The app's configured headers are resolved HERE — after the store lock
                // is released — and added to the file's own: a header the document states
                // for this path REPLACES the one above where it names the same field,
                // which is what configuring it asks for. A key or value no response can
                // carry — the document's text is caller text — is left out rather than
                // failing the whole answer.
                for (key, value) in crate::state::matched_headers(&visit.header_rules, &path) {
                    let _ = resp.headers_mut().try_insert(key, value);
                }
                resp
            }
            crate::state::HostedAnswer::Redirect { redirect, location } => {
                // Both halves of a redirect are converted falliably rather than
                // asserted: the status is one of the two the redirect kind names, and
                // the `Location` is built from the rule's target, which is caller text —
                // a value no response can carry is answered as a path nothing resolves
                // rather than taken as an invariant of this module.
                let Ok(status) = StatusCode::try_from(redirect.status()) else {
                    return plane_error(404, "NotFound", SITE_NOT_FOUND_MESSAGE);
                };
                let mut resp = RtResponse::new(status, SdkBody::empty());
                if resp
                    .headers_mut()
                    .try_insert("location", location)
                    .is_err()
                {
                    return plane_error(404, "NotFound", SITE_NOT_FOUND_MESSAGE);
                }
                resp.headers_mut().insert("content-length", "0");
                resp
            }
            crate::state::HostedAnswer::NotFound => {
                plane_error(404, "NotFound", SITE_NOT_FOUND_MESSAGE)
            }
            crate::state::HostedAnswer::Unauthorized => site_unauthorized(),
        }
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one
/// protocol instance, the generated one, and no second routing table to drift from the
/// model.
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
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status and every other header are the protocol's: only an answer that
        // carries no content is emptied of the body rendered for it, and the framing of
        // such an answer is `service_common`'s, written once for every service whose
        // success answers it.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries a JSON error envelope at a 4xx
        // or 5xx, so nothing here is emptied; the same call is made on both paths rather
        // than leaving one of them to answer differently.
        Ok(without_content(response))
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

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // A request addressed to a BRANCH's site is this service's, because that is the
        // address the control plane told the caller its site is at: either the HOST whose
        // first label names a branch rather than a service — from which the framework
        // resolves no service, so nothing else would claim the request — or, for an app
        // announced on this emulator's own endpoint, the PATH under the reserved
        // data-plane marker, which binds no modelled operation. The host a request
        // addressed is read by the workspace's one reader of data-plane addressing. See
        // [`Self::hosts_a_branch`].
        if let Some((address, _within)) = crate::state::addressed_hosted_site(parts.uri.path()) {
            if self.hosts_a_branch(&address) {
                return true;
            }
        }
        if let Some(host) = ::service_common::data_plane_addressing::addressed_host(
            &parts.uri.to_string(),
            parts
                .headers
                .get(::http::header::HOST)
                .and_then(|value| value.to_str().ok()),
        ) {
            if self.hosts_a_branch(&crate::state::HostedSiteAddress::Host(&host)) {
                return true;
            }
        }
        // A request that FOLLOWS an upload destination this service handed out is
        // claimed by the store that minted it, whatever else the request carries: it
        // has no signature and its endpoint host may name no service at all, so the
        // route key cannot route it. The upload path binds no modelled operation, so
        // nothing modelled is diverted here. See [`Self::holds_deployment_upload`].
        if let Some(target) = crate::state::parse_deployment_upload_path(parts.uri.path()) {
            return self.holds_deployment_upload(&target);
        }
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) BEFORE any handler ran.
        // AWS answers that class as the `ValidationException` envelope the framework
        // already renders, with one difference in how a requirement is QUOTED: a
        // `@pattern` is stated WITHOUT the anchors a whole-value expression wraps it in,
        // so a client that re-applies the reported expression gets the one AWS reports.
        // That rendering is shared by every service on a JSON protocol, so it lives in
        // `service-common` rather than here; the CHECK still uses the model's pattern as
        // written, and the code, the status, the envelope, the member paths and every
        // other requirement stay the framework's.
        //
        // What this service chooses beyond that is the ORDER an enumerated value set is
        // stated in ([`in_stated_value_order`]), which is not the order the model
        // declares it in.
        ::service_common::aws_validation_refusal(&in_stated_value_order(refusal))
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by
        // the protocol returned here. See the module docs: it is the model's own
        // protocol, with one correction — an answer that carries no content carries no
        // body and states the length it has, instead of the empty JSON object the
        // serializer renders for a members-less output.
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A request to an upload destination this service handed out is exactly that
        // case by construction — the model binds no operation to the upload path — so
        // this is where `CreateDeployment`'s destinations are served, and a request to a
        // branch's site the same way: the site's paths are the DEPLOYMENT's, and the
        // model binds no operation to any of them. Anything else keeps the framework's
        // default.
        let uri = req.uri().to_string();
        let request_path = ::service_common::request_path(req.uri());
        if let Some((address, within)) = crate::state::addressed_hosted_site(request_path) {
            if self.hosts_a_branch(&address) {
                return Some(self.serve_hosted_site(req, &address, within));
            }
        }
        if let Some(host) = ::service_common::data_plane_addressing::addressed_host(
            &uri,
            req.headers().get("host"),
        ) {
            let address = crate::state::HostedSiteAddress::Host(&host);
            if self.hosts_a_branch(&address) {
                return Some(self.serve_hosted_site(req, &address, request_path));
            }
        }
        let target = crate::state::parse_deployment_upload_path(request_path)?;
        Some(self.serve_deployment_upload(req, &target))
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
/// nothing from the request, so a caller cannot probe for deployments it was not handed
/// a destination for.
const UPLOAD_NOT_FOUND_MESSAGE: &str = "The requested upload destination does not exist";

/// The body of an upload-plane refusal of a write whose reservation's window has closed.
const UPLOAD_EXPIRED_MESSAGE: &str = "Request has expired";

/// The body of an upload-plane refusal of a body larger than this emulator reads from
/// one deployment (`crate::ops::start_deployment::MAX_ARCHIVE_BYTES`).
const UPLOAD_TOO_LARGE_MESSAGE: &str = "The upload is larger than this endpoint accepts";

/// The body of an upload-plane refusal of a request whose body this process cannot read
/// as bytes, and so cannot store.
const UPLOAD_UNREADABLE_MESSAGE: &str = "The upload's body could not be read";

/// The body of a hosting-edge answer for a path the branch's deployment did not publish.
const SITE_NOT_FOUND_MESSAGE: &str = "The requested file was not found";

/// The body of a hosting-edge refusal of a method other than the read of a file.
const SITE_METHOD_MESSAGE: &str = "This site is read by GET or HEAD";

/// What a branch's site answers a request that does not satisfy its access control: the
/// `401` and the challenge that asks for the credentials.
///
/// RFC 9110 requires a `WWW-Authenticate` challenge on a `401`, and it is the challenge
/// that makes a browser ask the visitor for the `user:password` pair the branch's
/// `basicAuthCredentials` configured. The realm is a constant of this module and the body
/// carries nothing from the request or from the site: a visitor without credentials is
/// told to present some, and nothing about what the branch publishes.
fn site_unauthorized() -> RtResponse {
    let mut resp = plane_error(401, "Unauthorized", SITE_UNAUTHORIZED_MESSAGE);
    resp.headers_mut().insert(
        "www-authenticate",
        format!(
            "{} realm=\"{SITE_REALM}\"",
            crate::state::basic_auth::BASIC_SCHEME
        ),
    );
    resp
}

/// The body of that refusal.
const SITE_UNAUTHORIZED_MESSAGE: &str = "This site requires basic authorization credentials";

/// The protection space the challenge names: the access control of a branch's site, as a
/// constant carrying nothing from the request.
const SITE_REALM: &str = "Amplify Hosting";

/// The body of a hosting-edge answer when this service's store is unreachable.
const SITE_STORE_MESSAGE: &str = "the Amplify hosting store is unavailable";

/// One answer of a plane this service serves outside the model — the deployment-upload
/// destinations and the hosting edge: the status, the code in `x-amzn-errortype`, and a
/// JSON body carrying the reason.
///
/// Those planes are plain HTTPS rather than modelled operations, so they answer in this
/// one small envelope instead of the service's restJson1 one. The message is built from
/// constants, never from the request: a visitor of a site is told that the file is not
/// there, not what else the branch holds.
fn plane_error(status: u16, code: &str, message: &str) -> RtResponse {
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
