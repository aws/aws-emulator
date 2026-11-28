// SPDX-License-Identifier: CC0-1.0

//! The data planes this service publishes URLs into, and the operation that serves them.
//!
//! Three paths bind no modelled operation — the content-upload slot a
//! `StartContentUpload` URL names, a content's stored asset, and a message-template
//! attachment's file — so the generated routing table resolves none of them. They are
//! served HERE, by one author-written [`ErasedOperation`] the service names from
//! [`Service::override_operation`](smithy_server_runtime::Service::override_operation).
//!
//! That seam is what puts these planes BEHIND the emulator's own authentication: the
//! router resolves the operation first and authenticates the request against the schemes
//! that operation declares before running it, so a request to one of these paths is
//! verified exactly as a request to a modelled operation is. [`Self::auth_schemes`] names
//! the service's own SigV4 scheme, because the routing key of such a request names no
//! modelled operation to take the schemes from. The URLs the handlers hand out are
//! query-signed through `ctx.url_signer()`, so a caller that follows one presents that
//! signature, its expiry and the path it was signed for.
//!
//! The id in the path, the `urlExpiry` of the record it names and the headers the start
//! call published are kept as the further constraints they always were: the signature says
//! the URL came from this emulator and has not expired, and the record says what the
//! request may touch.

use super::*;

/// The data planes of this service, as one operation the router can authenticate and run.
///
/// Holds the SAME shared engine the generated entry threads into every operation, so the
/// plane reads and writes the one store the modelled operations do.
pub struct ContentDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ContentDataPlane<Eng> {
    /// Construct from the shared engine, like every operation in this crate.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Whether `path` is one of the three data-plane paths, and so a request this
    /// operation serves rather than one the routing table resolves.
    pub fn serves(path: &str) -> bool {
        crate::state::parse_content_upload_path(path).is_some()
            || crate::state::parse_content_asset_path(path).is_some()
            || crate::state::parse_message_template_attachment_path(path).is_some()
    }
}

impl<Eng: Send + Sync + 'static> ::smithy_server_runtime::ErasedOperation
    for ContentDataPlane<Eng>
{
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // A request to one of these paths names no modelled operation, so the router has
        // none to read the schemes from and falls back to these: this service's own, as
        // the model declares them for every operation of it. Naming them is what makes
        // the router verify the request's signature — the one in the query of the URL
        // this service handed out — before this operation runs.
        Some(&["aws.auth#sigv4"])
    }

    fn run<'r>(
        &'r self,
        _svc: &'r dyn ::smithy_server_runtime::Service,
        req: &'r RtRequest,
        _ctx: &'r ::smithy_server_runtime::OperationContext<'r>,
    ) -> ::smithy_server_runtime::BoxFuture<'r, RtResponse> {
        ::std::boxed::Box::pin(async move {
            // Each plane answers in its own small envelope rather than the service's
            // restJson1 one, because none of them is a modelled operation; see
            // [`upload_error`].
            let path = request_path(req.uri());
            if let Some(target) = crate::state::parse_content_upload_path(path) {
                return self.serve_content_upload(req, &target);
            }
            if let Some(target) = crate::state::parse_content_asset_path(path) {
                return self.serve_content_asset(req, &target);
            }
            match crate::state::parse_message_template_attachment_path(path) {
                Some(target) => self.serve_message_template_attachment(req, &target),
                // Unreachable: the service names this operation only for a path
                // [`Self::serves`] admits.
                None => upload_error(404, "NoSuchUpload", UPLOAD_NOT_FOUND_MESSAGE),
            }
        })
    }
}

/// The planes themselves: one method per path [`ContentDataPlane::serves`] admits, each
/// reading and writing only the record the URL names.
impl<Eng: Send + Sync + 'static> ContentDataPlane<Eng> {
    /// Serve one request to a published upload URL.
    ///
    /// A `PUT` writes the asset into the upload the URL names — which is the only thing
    /// the plane can touch — and is refused when the URL is no longer usable, when the
    /// request does not carry the headers the start call published, or when the body is
    /// larger than this emulator stores. The upload, the knowledge base it was started
    /// under and the id in the path must all agree: a path that crosses one knowledge
    /// base with another's upload names nothing.
    pub(super) fn serve_content_upload(
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
                "the content-upload store is unavailable",
            );
        };
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        let resolved_knowledge_base = crate::state::resolve_id(&target.knowledge_base_label);
        let names_this_upload = store
            .content_upload(&target.upload_id)
            .is_some_and(|upload| resolved_knowledge_base == upload.knowledge_base_id);
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
                    req.headers()
                        .get(name)
                        .map(::std::string::ToString::to_string)
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
        // write there is no room for stores nothing: the slot keeps the body it had.
        if !store.write_content_upload_body(&target.upload_id, body.to_vec()) {
            return upload_error(413, "EntityTooLarge", UPLOAD_STORE_FULL_MESSAGE);
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

    /// Serve one request to a published attachment URL.
    ///
    /// A `GET` answers the file `CreateMessageTemplateAttachment` stored on the message
    /// template the URL names — which is the only thing the plane can touch, and it
    /// changes nothing — and is refused when the attachment's `urlExpiry` has passed. The
    /// attachment, the template that holds it and the knowledge base that holds the
    /// template must all agree: a path that crosses one template with another's attachment
    /// names nothing. A file only a published version still carries is served too, since
    /// `DeleteMessageTemplateAttachment` leaves every published version's attachments as
    /// they were.
    pub(super) fn serve_message_template_attachment(
        &self,
        req: &RtRequest,
        target: &crate::state::MessageTemplateAttachmentTarget,
    ) -> RtResponse {
        if req.method() != "GET" {
            return upload_error(
                405,
                "MethodNotAllowed",
                "This attachment URL is read by a single GET",
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return upload_error(
                500,
                "InternalServerException",
                "the message-template store is unavailable",
            );
        };
        let now = ::std::time::SystemTime::now();
        let store = state.lock();
        let knowledge_base_id = crate::state::resolve_id(&target.knowledge_base_label);
        let message_template_id = crate::state::resolve_id(&target.message_template_label);
        let Some(attachment) = store.stored_message_template_attachment(
            knowledge_base_id,
            message_template_id,
            &target.attachment_id,
        ) else {
            return upload_error(404, "NoSuchKey", ATTACHMENT_NOT_FOUND_MESSAGE);
        };
        // The URL stops being usable at the `urlExpiry` every read of the template
        // reports, so a late GET is refused rather than served: what the plane honours and
        // what a read publishes are the one stored instant.
        if attachment.has_expired_url(now) {
            return upload_error(403, "AccessDenied", UPLOAD_EXPIRED_MESSAGE);
        }
        let mut resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200 is a valid status"),
            SdkBody::from(attachment.body.clone()),
        );
        // The model carries no media type for an attachment file, so the plane claims
        // none: the bytes are served as the generic binary type, with the presentation the
        // upload gave. The presentation and the file NAME are both stored values that came
        // from a request, and a header's text is visible ASCII, so each is written only
        // when it is text a header may carry: a name of other scripts is served under the
        // presentation alone, and a presentation that is not header text under the fixed
        // [`ATTACHMENT_DISPOSITION`] — never as raw bytes in a header, and never through a
        // path that could unwind this request.
        resp.headers_mut()
            .insert("content-type", GENERIC_ASSET_CONTENT_TYPE);
        let presentation = attachment.content_disposition.to_ascii_lowercase();
        let presentation = if header_safe_text(&presentation) {
            presentation
        } else {
            ATTACHMENT_DISPOSITION.to_string()
        };
        let disposition = if header_safe_text(&attachment.name) {
            format!("{presentation}; filename=\"{}\"", attachment.name)
        } else {
            presentation
        };
        if resp
            .headers_mut()
            .try_insert("content-disposition", disposition)
            .is_err()
        {
            // Unreachable: every part of the value above is header text. The module's own
            // constant is served rather than unwinding this request.
            resp.headers_mut()
                .insert("content-disposition", ATTACHMENT_DISPOSITION);
        }
        resp.headers_mut()
            .insert("content-length", attachment.body.len().to_string());
        resp
    }

    /// Serve one request to a published content-asset URL.
    ///
    /// A `GET` answers the bytes `CreateContent` finalized into the content the URL names
    /// — which is the only thing the plane can touch, and it changes nothing — and is
    /// refused when the content's `urlExpiry` has passed. The content, the knowledge base
    /// it lives in and the id in the path must all agree: a path that crosses one
    /// knowledge base with another's content names nothing.
    pub(super) fn serve_content_asset(
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
                "the content store is unavailable",
            );
        };
        let now = ::std::time::SystemTime::now();
        let store = state.lock();
        let knowledge_base_id = crate::state::resolve_id(&target.knowledge_base_label);
        let Some(content) = store.content(knowledge_base_id, &target.content_id) else {
            return upload_error(404, "NoSuchKey", ASSET_NOT_FOUND_MESSAGE);
        };
        // The URL stops being usable at the `urlExpiry` every read of the content
        // reports, so a late GET is refused rather than served: what the plane honours
        // and what a read publishes are the one stored instant.
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
        // unwinding the request serving it. The length is this process's own number.
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
