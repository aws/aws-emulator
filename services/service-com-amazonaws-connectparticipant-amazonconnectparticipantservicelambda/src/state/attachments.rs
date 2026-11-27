// SPDX-License-Identifier: CC0-1.0

//! One chat's attachments: what is held for an uploaded file, where it is uploaded to and
//! downloaded from, and the bounds on both.
//!
//! An attachment has a LIFECYCLE. `StartAttachmentUpload` "Provides a pre-signed Amazon S3
//! URL in response for uploading the file directly to S3", `CompleteAttachmentUpload`
//! "Allows you to confirm that the attachment has been uploaded using the pre-signed URL
//! provided in StartAttachmentUpload API", and `GetAttachment` "Provides a pre-signed URL
//! for download of a completed attachment". So the record minted by the first call is
//! [`ARTIFACT_STATUS_IN_PROGRESS`] and the second call is what moves it to
//! [`ARTIFACT_STATUS_APPROVED`]; a download is served for the approved attachment alone.
//!
//! Whether a chat may share attachments AT ALL is its instance's: a chat file is kept in
//! the storage the Amazon Connect instance is configured with, so a chat of an instance
//! that has none shares no files, and every attachment operation of such a chat is refused
//! ([`ChatSession::attachments_shared`]).
//!
//! Both URLs are addresses a CALLER follows, so each has to name a host this emulator
//! answers on — no production AWS hostname resolves here. Each is therefore PRESIGNED
//! against this emulator's own base URL ([`attachment_url`]) with the attachment's own
//! address in the path ([`attachment_object_path`]), and [`parse_attachment_path`] reads
//! that path back, so the URL this service hands out and the request `crate::service`'s
//! attachment plane serves cannot disagree about which attachment is named.
//!
//! Every store here is bounded. One attachment holds at most
//! [`MAX_ATTACHMENT_BYTES`] — the size `StartAttachmentUpload` declared, which the plane
//! holds the arriving file to — one contact holds at most
//! [`MAX_ATTACHMENTS_PER_CONTACT`] of them, and an attachment is reclaimed with the chat
//! session it belongs to (`State::prune_expired`), so nothing a caller can drive grows
//! without limit.

use super::*;

/// `ArtifactStatus$IN_PROGRESS`, from `ARTIFACT_STATUS_VALUES`: an attachment whose upload
/// has been started and whose upload nothing has confirmed yet.
pub const ARTIFACT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ArtifactStatus$APPROVED`: an attachment whose upload `CompleteAttachmentUpload` has
/// confirmed. The only status a download URL is reported for.
pub const ARTIFACT_STATUS_APPROVED: &str = "APPROVED";

/// `ChatItemType$ATTACHMENT`, the transcript item a confirmed upload appends: an
/// attachment is part of the conversation, which is why `Item` carries `Attachments`.
pub const CHAT_ITEM_TYPE_ATTACHMENT: &str = "ATTACHMENT";

/// The most attachments one contact may hold. `ServiceQuotaExceededException` is "The
/// number of attachments per contact exceeds the quota", and `StartAttachmentUpload`'s
/// documentation points at the supported file types and limits in the Amazon Connect
/// feature specifications; this is this emulator's own bound on that count, past which a
/// further upload is refused rather than state growing without limit.
pub const MAX_ATTACHMENTS_PER_CONTACT: usize = 20;

/// The largest attachment this emulator holds, in bytes. State is in memory, so the size
/// one caller can drive is capped; `StartAttachmentUpload` refuses a declared size past it
/// rather than publishing an upload target for a file that cannot be held.
pub const MAX_ATTACHMENT_BYTES: i64 = 20 * 1024 * 1024;

/// The shortest download window a caller may ask for:
/// `GetAttachmentRequest$UrlExpiryInSeconds` is modelled `range 5..300`.
pub const MIN_URL_EXPIRY_SECONDS: i64 = 5;

/// The longest download window a caller may ask for, from the same modelled range, and so
/// the window a URL this service signs of its own accord is given: `GetAttachment` is the
/// only input that carries the member at all, and the upload target published by
/// `StartAttachmentUpload` has no parameter of its own.
pub const MAX_URL_EXPIRY_SECONDS: i64 = 300;

/// [`MAX_URL_EXPIRY_SECONDS`] as a duration: the window an attachment URL is signed for
/// when the request names none.
pub const DEFAULT_URL_EXPIRY: Duration = Duration::from_secs(MAX_URL_EXPIRY_SECONDS as u64);

/// `ContentDisposition$ATTACHMENT`, from `CONTENT_DISPOSITION_VALUES`: the value
/// `GetAttachmentRequest$UrlContentDisposition` documents as downloading the file, and as
/// "the default behavior" when the request names none.
pub const CONTENT_DISPOSITION_ATTACHMENT: &str = "ATTACHMENT";

/// `ContentDisposition$INLINE`, from `CONTENT_DISPOSITION_VALUES`: the value that displays
/// the file in the browser instead of downloading it.
pub const CONTENT_DISPOSITION_INLINE: &str = "INLINE";

/// The field a published download URL carries the disposition its file is served under in,
/// as a pre-signed URL of the store this target stands in for names a response override.
///
/// ONE place states the field, so the URL `GetAttachment` hands out and the request the
/// attachment plane serves back cannot disagree about it (see
/// [`content_disposition_of_url`]).
pub const RESPONSE_CONTENT_DISPOSITION_FIELD: &str = "response-content-disposition";

/// The `Content-Disposition` header value that downloads the file.
pub const DISPOSITION_ATTACHMENT: &str = "attachment";

/// The `Content-Disposition` header value that displays the file in the browser.
pub const DISPOSITION_INLINE: &str = "inline";

/// The header value a `ContentDisposition` names, or `None` for a string that is not one of
/// the values the enum declares.
///
/// The member says how a browser handles the file at the URL, which a response states in its
/// `Content-Disposition`, so each modelled value maps to that header's own token.
#[must_use]
pub fn content_disposition_header(value: &str) -> Option<&'static str> {
    match value {
        CONTENT_DISPOSITION_ATTACHMENT => Some(DISPOSITION_ATTACHMENT),
        CONTENT_DISPOSITION_INLINE => Some(DISPOSITION_INLINE),
        _ => None,
    }
}

/// The disposition the file at a published download URL is served under: the one the URL's
/// own [`RESPONSE_CONTENT_DISPOSITION_FIELD`] names, and otherwise the documented default,
/// which downloads the file.
///
/// Only a field naming a disposition this service publishes is honoured, and the answer is
/// one of two fixed tokens: a URL's query is caller-controlled text, so nothing from it is
/// echoed into a response header.
#[must_use]
pub fn content_disposition_of_url(uri: &str) -> &'static str {
    let Some((_, query)) = uri.split_once('?') else {
        return DISPOSITION_ATTACHMENT;
    };
    query
        .split('&')
        .filter_map(|field| field.split_once('='))
        .find(|(name, _)| *name == RESPONSE_CONTENT_DISPOSITION_FIELD)
        .and_then(|(_, value)| match value {
            DISPOSITION_INLINE => Some(DISPOSITION_INLINE),
            DISPOSITION_ATTACHMENT => Some(DISPOSITION_ATTACHMENT),
            _ => None,
        })
        .unwrap_or(DISPOSITION_ATTACHMENT)
}

/// One attachment of one chat: what the upload declared, how far its lifecycle has got,
/// the upload target that was published for it and the bytes that arrived there.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// `ArtifactId` — the `AttachmentId` every later call names it by.
    pub id: String,
    /// The contact whose chat this attachment belongs to. Only a connection token that
    /// authenticates THAT contact's participant may reach it: an attachment of one chat is
    /// not an attachment of another's.
    pub contact_id: String,
    /// `AttachmentName`, "A case-sensitive name of the attachment being uploaded".
    pub name: String,
    /// `ContentType`, the MIME file type the upload declared.
    pub content_type: String,
    /// `AttachmentSizeInBytes`, the size the upload declared, which is what `GetAttachment`
    /// reports and what the upload plane holds the arriving body to.
    pub size_in_bytes: i64,
    /// `ArtifactStatus`: [`ARTIFACT_STATUS_IN_PROGRESS`] until the upload is confirmed,
    /// then [`ARTIFACT_STATUS_APPROVED`].
    pub status: String,
    /// The pre-signed upload target reported for this attachment, stored so a repeated
    /// `ClientToken` is answered with the very target the first call published.
    pub upload_url: String,
    /// When that target stops accepting the upload.
    pub upload_url_expiry: SystemTime,
    /// The headers the upload must carry, as `UploadMetadata$HeadersToInclude` reported
    /// them.
    pub upload_headers: Vec<(String, String)>,
    /// The bytes that arrived at the upload target, once they have. `None` for an
    /// attachment nothing has uploaded: no file is invented for it, so a download of one
    /// names an object that does not exist.
    pub uploaded: Option<Vec<u8>>,
    /// When the download target stops serving this attachment's file: the end of the window
    /// of the longest-lived pre-signed download URL `GetAttachment` has reported for it, and
    /// `None` until one has been. A URL already handed out keeps working for the window it
    /// was announced with, which is why this only ever moves later.
    pub download_url_expiry: Option<SystemTime>,
}

impl Attachment {
    /// Whether this attachment's upload has been confirmed.
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.status == ARTIFACT_STATUS_APPROVED
    }

    /// Record that a pre-signed download URL valid until `expiry` has been reported for this
    /// attachment, so the plane serves its file for exactly as long as the longest window a
    /// caller was handed.
    ///
    /// Part of MINTING that URL — the one write `GetAttachment` makes, because that operation
    /// is documented as providing the URL — and never a shortening: a window already
    /// announced cannot be taken back by a later, shorter one.
    pub fn downloadable_until(&mut self, expiry: SystemTime) {
        if self.download_url_expiry.is_none_or(|held| held < expiry) {
            self.download_url_expiry = Some(expiry);
        }
    }

    /// Whether a download of this attachment is inside the window of a URL that was reported
    /// for it: `at` is before the end of the longest one. That is what the signature of a
    /// pre-signed URL decides for the service this target stands in for, and an attachment no
    /// download URL was ever reported for is outside every window.
    #[must_use]
    pub fn is_downloadable_at(&self, at: SystemTime) -> bool {
        self.download_url_expiry.is_some_and(|expiry| at < expiry)
    }
}

impl ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState {
    /// The attachment `attachment_id` names, when it belongs to `contact_id`'s chat.
    ///
    /// An id this store holds no attachment for, and one of another contact's chat, both
    /// resolve to nothing: which attachments a caller can reach is decided by the contact
    /// its connection token authenticates.
    pub fn attachment_of_contact(
        &self,
        attachment_id: &str,
        contact_id: &str,
    ) -> Option<&Attachment> {
        self.attachments
            .get(attachment_id)
            .filter(|attachment| attachment.contact_id == contact_id)
    }

    /// [`Self::attachment_of_contact`], for a caller that moves the attachment's lifecycle
    /// on or stores the bytes that arrived for it.
    pub fn attachment_of_contact_mut(
        &mut self,
        attachment_id: &str,
        contact_id: &str,
    ) -> Option<&mut Attachment> {
        self.attachments
            .get_mut(attachment_id)
            .filter(|attachment| attachment.contact_id == contact_id)
    }

    /// How many attachments `contact_id` holds, which is what
    /// [`MAX_ATTACHMENTS_PER_CONTACT`] bounds.
    #[must_use]
    pub fn attachment_count(&self, contact_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| attachment.contact_id == contact_id)
            .count()
    }

    /// The attachment an idempotency `key` already answered for: the caller is repeating a
    /// request, so it must be answered with what the first call stored.
    pub fn replayed_attachment(&self, key: &str) -> Option<&Attachment> {
        self.attachments.get(self.attachment_idempotency.get(key)?)
    }

    /// Record that `key` answered for `attachment_id`, so a repeat of that request replays
    /// it. Only ever called alongside an attachment, so [`MAX_ATTACHMENTS_PER_CONTACT`]
    /// bounds this index too.
    pub fn record_attachment_idempotency(&mut self, key: String, attachment_id: String) {
        self.attachment_idempotency.insert(key, attachment_id);
    }
}

/// The key an attachment request's `ClientToken` is recorded under.
///
/// Scoped by the CONTACT, because a client token is a token of one chat's caller, and by
/// the OPERATION, because a `ClientToken` makes one operation idempotent: the same token on
/// `StartAttachmentUpload` and on `CompleteAttachmentUpload` is two distinct requests.
#[must_use]
pub fn attachment_idempotency_key(contact_id: &str, operation: &str, client_token: &str) -> String {
    format!("{contact_id}:{operation}:{client_token}")
}

/// The path prefix every attachment object this service publishes a URL into carries.
///
/// Inside this service's own modelled path space (`/participant/…`), so a request to a URL
/// it handed out is claimed by this service and by no peer sharing its signing name, while
/// no modelled operation binds this path — which is what makes such a request reach the
/// attachment plane.
pub const ATTACHMENT_PATH_PREFIX: &str = "/participant/attachments";

/// The path one attachment's object is addressed at: the contact's chat, then the
/// attachment under it.
///
/// ONE place states the form, so the URL this service hands out and the request it serves
/// back cannot disagree about it (see [`parse_attachment_path`]).
#[must_use]
pub fn attachment_object_path(contact_id: &str, attachment_id: &str) -> String {
    format!("{ATTACHMENT_PATH_PREFIX}/{contact_id}/{attachment_id}")
}

/// The contact and the attachment a request path names, or `None` when it addresses no
/// attachment object at all.
///
/// Read from the same two parts [`attachment_object_path`] builds the published URL out of.
/// Any signature the URL carries is not part of WHICH object it names, so it plays no part
/// here.
#[must_use]
pub fn parse_attachment_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix(ATTACHMENT_PATH_PREFIX)?.strip_prefix('/')?;
    let (contact_id, attachment_id) = rest.split_once('/')?;
    if contact_id.is_empty() || attachment_id.is_empty() || attachment_id.contains('/') {
        return None;
    }
    Some((contact_id.to_string(), attachment_id.to_string()))
}

/// The pre-signed URL one attachment is uploaded to (`method` `PUT`) or downloaded from
/// (`GET`), valid for `expires_in`.
///
/// Signed through the framework's URL signer, which is what makes the URL both a pre-signed
/// one and one this emulator ANSWERS: it signs for the emulator's current base URL, so the
/// announced host is the host the caller reached this service on rather than a host only AWS
/// serves. `bound_content_type` binds the media type the caller must declare into the
/// signature, which is how an upload target states the type of the file it accepts, and
/// `response_disposition` (a header value from [`content_disposition_header`]) is carried in
/// the URL's own [`RESPONSE_CONTENT_DISPOSITION_FIELD`], which is how a pre-signed URL states
/// how the response that serves the file must present it.
///
/// `None` only when the request cannot be signed at all, which the caller reports as a
/// server-side failure rather than handing out an address nothing answers.
pub fn attachment_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    method: http::Method,
    contact_id: &str,
    attachment_id: &str,
    bound_content_type: Option<&str>,
    response_disposition: Option<&str>,
    expires_in: Duration,
) -> Option<smithy_server_runtime::SignedUrl> {
    let path = attachment_object_path(contact_id, attachment_id);
    let mut request = smithy_server_runtime::SignedUrlRequest::new(
        method,
        ctx.identity().signing_name(),
        &path,
    );
    if let Some(content_type) = bound_content_type {
        request = request.header(http::header::CONTENT_TYPE.as_str(), content_type);
    }
    if let Some(disposition) = response_disposition {
        request = request.query(RESPONSE_CONTENT_DISPOSITION_FIELD, disposition);
    }
    ctx.url_signer().sign(request, expires_in).ok()
}

/// The headers an upload must carry, as `UploadMetadata$HeadersToInclude` ("The headers to
/// be provided while uploading the file to the URL") reports them: every header the
/// pre-signed URL bound into its signature, plus the length of the file the upload declared
/// — which is the length the upload plane holds the arriving body to.
#[must_use]
pub fn upload_headers(
    url: &smithy_server_runtime::SignedUrl,
    size_in_bytes: i64,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = url
        .required_headers()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    headers.push((
        http::header::CONTENT_LENGTH.as_str().to_string(),
        size_in_bytes.to_string(),
    ));
    headers
}
