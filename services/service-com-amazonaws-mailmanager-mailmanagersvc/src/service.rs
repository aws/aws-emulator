// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mailmanager-mailmanagersvc` service: how it claims requests and how it words the
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// This service's endpoint prefix, as its model gives it
/// (`smithy_query(query="service_info")`: `awsService.endpointPrefix` `mail-manager`) —
/// the name a host-addressed request resolves to, beside the SigV4 signing name (`ses`)
/// the generated identity carries.
pub const ENDPOINT_PREFIX: &str = "mail-manager";

/// This service's Smithy shape id, whose LOCAL name (`MailManagerSvc`) is what an
/// `awsJson1_0` `X-Amz-Target` prefix and an `rpcv2Cbor` URI name it by
/// (`smithy_query(query="service_info")`: service `MailManagerSvc`, protocols
/// `awsJson1_0` and `rpcv2Cbor`).
const SERVICE_SHAPE_ID: &str = "com.amazonaws.mailmanager#MailManagerSvc";

/// The `com-amazonaws-mailmanager-mailmanagersvc` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the import-file upload plane is served from
    /// [`Service::override_routing_error`], which is not an operation and so has no
    /// `self.engine` of its own; it must write the one store the operations read, not a
    /// second copy.
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
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `ses` — a signing name it SHARES with three other
        // emulated services: SES v1 (`awsQuery`, endpoint `email`), SESv2 and Pinpoint
        // Email (`restJson1`, endpoint `email`). The router resolves a request's routing
        // key from the SigV4 credential scope first, so all four see the SAME key, and
        // claiming on the signing name alone makes all four claim every request — which
        // the router answers `500 AmbiguousService` for, a code no AWS model carries.
        //
        // This is the RPC member of that family, and its path space is its protocols'
        // own (`POST /` for `awsJson1_0`, `/service/…` for `rpcv2Cbor`) — which SES v1
        // posts to as well — so a path test alone cannot tell the two apart. The shared
        // claim rule is therefore applied with this service's shape id: a request
        // addressed in an RPC form is claimed only when that form NAMES this service,
        // and one addressed in no RPC form at all is left to the peer whose URI space it
        // addresses. The routing key may name either of the two names this one endpoint
        // answers on.
        if [self.entry.identity().signing_name(), ENDPOINT_PREFIX]
            .iter()
            .any(|route_key| {
                service_common::claims_request(
                    parts,
                    route_key,
                    crate::ROUTE_PATH_SEGMENTS,
                    Some(SERVICE_SHAPE_ID),
                )
            })
        {
            return true;
        }
        // The input file of an address-list import job
        // (`CreateAddressListImportJobResponse$PreSignedUrl`) is written by whatever client
        // holds the locator this service handed out, with no credential scope and no
        // `X-Amz-Target` to name this service by — exactly as a pre-signed S3 URL is
        // written — so the path this service publishes uploads at is the only thing left to
        // identify it, and without this the router answers `UnknownService` for a URL this
        // service announced. Every method is claimed, not only the one that is served, so a
        // GET of a path this service publishes is this service's to refuse as a method. The
        // path sits under the reserved data-plane namespace
        // (`service_common::data_plane_addressing`), whose segment no modelled route of any
        // service carries, so this claim takes nothing from the peers above.
        crate::state::parse_import_job_upload_path(parts.uri.path()).is_some()
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        //
        // A request to an import job's upload URL is exactly that case by construction —
        // its path is in neither of this service's protocol path spaces — so this is where
        // such a URL is served. Anything else keeps the framework's default.
        let job_id =
            crate::state::parse_import_job_upload_path(service_common::request_path(req.uri()))?
                .to_string();
        self.serve_import_upload(req, &job_id)
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

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Serve one request to the upload URL of the import job `job_id`.
    ///
    /// An upload is a WRITE — that is what the URL was handed out for — so it records the
    /// input file as the one that job imports, and a second upload to the same URL replaces
    /// the first: a `PUT` is idempotent. The upload is judged in this order: the method, so
    /// a path this service publishes states which methods it serves rather than appearing
    /// absent; the size this emulator retains; and then the store, because a URL naming a
    /// job that is gone — or one whose address list is gone — is no longer a target.
    /// `None` only if a status this file names is not a status at all, which it is: it is
    /// what keeps this plane free of a conversion that could panic on a request path, and
    /// leaves the framework's own refusal to answer instead.
    fn serve_import_upload(&self, req: &RtRequest, job_id: &str) -> Option<RtResponse> {
        if req.method() != UPLOAD_METHOD {
            return method_not_allowed();
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return upload_error(
                429,
                "ThrottlingException",
                crate::state::STORE_UNAVAILABLE.to_string(),
            );
        };
        let body = req.body().bytes().unwrap_or_default();
        if body.len() > crate::state::MAX_IMPORT_FILE_BYTES {
            return upload_error(
                413,
                "RequestEntityTooLarge",
                format!(
                    "The uploaded input file is larger than the {} bytes this emulator \
                     retains.",
                    crate::state::MAX_IMPORT_FILE_BYTES
                ),
            );
        }
        let uploaded_at_millis = crate::state::now_millis();
        {
            let mut store = state.lock();
            // Borrowed once as the state it is, so the job and the lists are reached as two
            // disjoint fields.
            let store = &mut *store;
            // A path naming no stored job is not a target this service published: it is
            // refused rather than accepting a file nothing will import.
            let Some(job) = store.import_jobs.get_mut(job_id) else {
                return upload_error(
                    404,
                    "ResourceNotFoundException",
                    format!("Import job {job_id} not found."),
                );
            };
            // Nor is the target one this service still serves once the address list the job
            // imports into is gone: `DeleteAddressList` leaves the job ORPHANED, and a file
            // uploaded against one could never be imported into anything.
            if !store.address_lists.contains_key(&job.address_list_id) {
                return upload_error(
                    404,
                    "ResourceNotFoundException",
                    crate::state::address_list_not_found_message(&job.address_list_id),
                );
            }
            job.uploaded_file = Some(crate::state::UploadedImportFile {
                content: body.to_vec(),
                uploaded_at_millis,
            });
        }
        // The answer a pre-signed S3 `PUT` gives once the object is written: 200 and no
        // body.
        let mut response = RtResponse::new(StatusCode::try_from(200).ok()?, SdkBody::empty());
        response.headers_mut().insert("content-length", "0");
        Some(response)
    }
}

/// The one method an import job's upload URL serves: a pre-signed upload target takes the
/// bytes by `PUT`, which is how a client writes a file to the URL
/// `CreateAddressListImportJob` hands it.
const UPLOAD_METHOD: &str = "PUT";

/// The refusal the upload plane answers a method it does not serve with. RFC 9110 requires
/// a `405` to state which methods the target supports, so the refusal names it in the
/// `Allow` header as well as in its message.
fn method_not_allowed() -> Option<RtResponse> {
    let mut response = upload_error(
        405,
        "MethodNotAllowed",
        format!("An import job's upload URL is served by {UPLOAD_METHOD}."),
    )?;
    response.headers_mut().insert("allow", UPLOAD_METHOD);
    Some(response)
}

/// One upload-plane refusal: the status, the code in `x-amzn-errortype`, and a JSON body
/// carrying the reason.
///
/// The upload plane is plain HTTPS rather than a modelled operation, so it answers in the
/// envelope the client of a pre-signed URL reads — the status and the error code — rather
/// than in this service's RPC envelope.
/// The message is rendered by a JSON serializer, so a value taken from the request cannot
/// break out of the envelope it is placed in.
fn upload_error(http_status: u16, code: &'static str, message: String) -> Option<RtResponse> {
    let body = ::serde_json::json!({ "__type": code, "message": message }).to_string();
    let mut response = RtResponse::new(StatusCode::try_from(http_status).ok()?, SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", "application/json");
    response.headers_mut().insert("x-amzn-errortype", code);
    Some(response)
}
