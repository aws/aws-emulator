// SPDX-License-Identifier: CC0-1.0

//! The message-template ATTACHMENT family of this service's state: the record one
//! attachment file is persisted as, the projection every operation that reports one
//! answers out of, and the locator the download plane serves it under.
//!
//! An attachment belongs to the WORKING COPY of the message template it was uploaded to —
//! `CreateMessageTemplateAttachment` documents the name as unique "for each message
//! template referenced by the $LATEST qualifier"
//! (`smithy_query(query="doc",shape="CreateMessageTemplateAttachment")`) — so the records
//! live on [`MessageTemplate`] itself rather than in a second map beside it, and a version
//! published from that template carries the attachments the snapshot held.
//!
//! The record holds the FILE as well as the metadata the API reports, because the `url` a
//! read reports is "a pre-signed Amazon S3 URL that can be used to download the attachment
//! file" (`smithy_query(query="shape",shape="MessageTemplateAttachment")`): a URL is a
//! promise it can be FOLLOWED, so the bytes the upload carried are what the download plane
//! in `crate::service` answers with.

use std::time::SystemTime;

use super::*;
use crate::ops::types::MessageTemplateAttachment;

/// The only `ContentDisposition` the model declares
/// (`smithy_query(query="shape",shape="ContentDisposition")`), which is therefore the
/// presentation every stored attachment carries.
pub const CONTENT_DISPOSITION_ATTACHMENT: &str = "ATTACHMENT";

/// The fixed text for a request that reports an attachment the message template does not
/// hold as the MISSING RESOURCE, worded as this endpoint words the other not-found
/// resources of this family ([`MESSAGE_MESSAGE_TEMPLATE_NOT_FOUND`]), with the resource
/// token spelled as the API itself names the resource.
///
/// It belongs to the not-found class (HTTP 404 `ResourceNotFoundException`) alone: a
/// refusal of the CONFLICT class states the conflict rather than a missing resource, and
/// carries [`MESSAGE_MESSAGE_TEMPLATE_ATTACHMENT_DELETE_CONFLICT`] instead.
pub const MESSAGE_MESSAGE_TEMPLATE_ATTACHMENT_NOT_FOUND: &str =
    "Message template attachment does not exist.";

/// The fixed text `DeleteMessageTemplateAttachment` refuses an `attachmentId` the
/// template's `$LATEST` working copy holds no file under with.
///
/// The refusal is the CONFLICT class rather than the not-found one: the message template
/// the route names exists, and it is the state that template is in — holding no file under
/// the id — that the request cannot be processed against, which is what
/// `ConflictException` states ("The request could not be processed because of conflict in
/// the current state of the resource",
/// `smithy_query(query="shape",shape="ConflictException")`). The sentence therefore names
/// the operation that could not be carried out and the conflict class itself, and
/// interpolates nothing from the request or from state: no attachment, template or
/// knowledge-base id, no name, no limit and no member name, so every such refusal of this
/// operation reads alike.
pub const MESSAGE_MESSAGE_TEMPLATE_ATTACHMENT_DELETE_CONFLICT: &str =
    "Unable to delete message template attachment: there is conflict in the current state \
     of resource";

/// The most attachments one message template's working copy may hold at a time; an upload
/// beyond it is refused with the modelled `ServiceQuotaExceededException` (HTTP 402),
/// which `CreateMessageTemplateAttachment` declares.
///
/// CHOSEN: AWS quotas attachments per message template but does not publish the figure.
/// The bound is also one half of what keeps this store finite — an attachment is released
/// only by a delete of it, of the template that holds it, or of the knowledge base that
/// holds the template.
pub const MAX_ATTACHMENTS_PER_MESSAGE_TEMPLATE: usize = 20;

/// The most bytes one attachment file may carry, measured on the DECODED file rather than
/// on its base64 text.
///
/// CHOSEN: this emulator holds the file in its own heap, so the ceiling is a resource
/// guard it owns rather than an AWS-published quota.
pub const MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

/// The most base64 TEXT a `body` that could decode within
/// [`MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES`] carries: base64 writes 4 characters per 3
/// bytes, and pads to a multiple of 4.
///
/// It is what an upload is measured against BEFORE the decode, so a body past the ceiling
/// is refused rather than decoded into this process's heap and refused afterwards.
pub const fn max_message_template_attachment_body_len() -> usize {
    MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES / 3 * 4 + 4
}

/// The most attachment bytes this store holds across EVERY message template, so the
/// attachments x bytes-per-attachment PRODUCT is finite and not just each factor. Refused
/// the same way as [`MAX_ATTACHMENTS_PER_MESSAGE_TEMPLATE`].
pub const MAX_STORED_MESSAGE_TEMPLATE_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;

/// The sentence an upload past either attachment ceiling is refused with. It states what
/// the caller can DO about it and names no figure of the store, which is this emulator's
/// own rather than an AWS-published quota.
pub fn message_message_template_attachment_quota() -> String {
    "This message template holds as many attachments as its store keeps. Delete an \
     attachment before uploading another."
        .to_string()
}

/// The sentence an upload whose `body` is not base64 is refused with.
///
/// The member's own documentation states the rule — "The body of the attachment file being
/// uploaded. It should be encoded using base64 encoding"
/// (`smithy_query(query="doc",shape="CreateMessageTemplateAttachmentRequest",member="body")`)
/// — so a body that is no base64 encoding carries no file to store. The one variable part
/// is the member the request carried it in; the value itself is `@sensitive` and is never
/// echoed.
pub fn message_attachment_body_not_base64(member: &str) -> String {
    format!("The {member} must be encoded using base64 encoding.")
}

/// The sentence an upload whose decoded file is larger than this endpoint stores is
/// refused with. Its variable part is the ceiling, in bytes, so the caller is told the
/// limit rather than only that it was passed.
pub fn message_attachment_too_large(member: &str) -> String {
    format!(
        "The {member} decodes to more than the {MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES} bytes \
         this endpoint accepts for one attachment file."
    )
}

/// One attachment file of a message template's working copy, as this emulator persists
/// it: the metadata every read of the template reports, and the file the published URL
/// serves.
#[derive(Debug, Clone)]
pub struct MessageTemplateAttachmentFile {
    /// `attachmentId`, of the modelled `Uuid` form — minted here, so it names exactly one
    /// file in the download URL's path.
    pub attachment_id: String,
    /// `AttachmentFileName` as the upload gave it, including its extension.
    pub name: String,
    /// `ContentDisposition` on the wire: how the file is presented to whoever downloads
    /// it ([`CONTENT_DISPOSITION_ATTACHMENT`]).
    pub content_disposition: String,
    /// When the file was uploaded, which is the `uploadedTime` every read reports.
    pub uploaded_at: SystemTime,
    /// The URL the file is downloaded from, kept so the record and the locator cannot
    /// drift.
    pub url: String,
    /// When that URL stops being usable — a download at or after it is refused, exactly
    /// as the `urlExpiry` every read reports says.
    pub url_expiry: SystemTime,
    /// The file, as the upload's base64 body decoded to.
    pub body: Vec<u8>,
}

impl MessageTemplateAttachmentFile {
    /// Whether this attachment's URL is no longer usable at `now`.
    pub fn has_expired_url(&self, now: SystemTime) -> bool {
        now >= self.url_expiry
    }

    /// This attachment as the modelled `MessageTemplateAttachment` shape the upload and
    /// every read of the template answer with: the stored metadata and the locator, never
    /// the file itself, which travels through the URL.
    pub fn to_attachment(&self) -> MessageTemplateAttachment {
        MessageTemplateAttachment {
            content_disposition: self.content_disposition.clone(),
            name: self.name.clone(),
            uploaded_time: ::aws_smithy_types::DateTime::from(self.uploaded_at),
            url: self.url.clone(),
            url_expiry: ::aws_smithy_types::DateTime::from(self.url_expiry),
            attachment_id: self.attachment_id.clone(),
        }
    }
}

/// What a request to a published attachment URL names: the knowledge-base and
/// message-template labels and the attachment id, as
/// [`message_template_attachment_path`] words them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageTemplateAttachmentTarget {
    pub knowledge_base_label: String,
    pub message_template_label: String,
    pub attachment_id: String,
}

/// The path the download plane serves one attachment file under.
///
/// It sits one segment DEEPER than the modelled route that addresses the attachment
/// itself (`DELETE /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}/attachments/{attachmentId}`),
/// so it names no modelled operation: the service's own data plane serves it, behind the
/// emulator's signature verification, the way [`content_asset_path`] is served. The URL a
/// caller is handed for it is signed by [`sign_data_plane_url`].
pub fn message_template_attachment_path(
    knowledge_base_id: &str,
    message_template_id: &str,
    attachment_id: &str,
) -> String {
    format!(
        "/knowledgeBases/{knowledge_base_id}/messageTemplates/{message_template_id}\
         /attachments/{attachment_id}/file"
    )
}

/// The attachment a request path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_message_template_attachment_path(
    path: &str,
) -> Option<MessageTemplateAttachmentTarget> {
    let rest = path.strip_prefix("/knowledgeBases/")?;
    let mut segments = rest.split('/');
    let (
        Some(knowledge_base_label),
        Some("messageTemplates"),
        Some(message_template_label),
        Some("attachments"),
        Some(attachment_id),
        Some("file"),
        None,
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    if knowledge_base_label.is_empty()
        || message_template_label.is_empty()
        || attachment_id.is_empty()
    {
        return None;
    }
    Some(MessageTemplateAttachmentTarget {
        knowledge_base_label: knowledge_base_label.to_string(),
        message_template_label: message_template_label.to_string(),
        attachment_id: attachment_id.to_string(),
    })
}

impl MessageTemplate {
    /// The attachments of this template as the modelled `MessageTemplateAttachmentList`,
    /// or `None` when it holds none: a template with nothing to report under the member
    /// reports no member rather than an empty list.
    ///
    /// THE ONE PLACE this record is projected onto that member, so every operation that
    /// reports a template's attachments — the extended read and a render — reports them
    /// alike.
    pub fn attachments_out(&self) -> Option<Vec<MessageTemplateAttachment>> {
        (!self.attachments.is_empty()).then(|| {
            self.attachments
                .iter()
                .map(MessageTemplateAttachmentFile::to_attachment)
                .collect()
        })
    }

    /// The attachment this template's working copy holds under `attachment_id`, or `None`
    /// when it holds none.
    pub fn attachment(&self, attachment_id: &str) -> Option<&MessageTemplateAttachmentFile> {
        self.attachments
            .iter()
            .find(|attachment| attachment.attachment_id == attachment_id)
    }

    /// Remove the attachment this template's working copy holds under `attachment_id`,
    /// answering whether it held one at all.
    ///
    /// `DeleteMessageTemplateAttachment` deletes the file "from the Amazon Q in Connect
    /// message template that is referenced by $LATEST qualifier"
    /// (`smithy_query(query="doc",shape="DeleteMessageTemplateAttachment")`), so this
    /// reaches the working copy ALONE: a version's snapshot is a separate record and keeps
    /// the attachments it was published with. The remaining attachments keep the order they
    /// were uploaded in, which is the order every read reports them in.
    pub fn remove_attachment(&mut self, attachment_id: &str) -> bool {
        let before = self.attachments.len();
        self.attachments
            .retain(|attachment| attachment.attachment_id != attachment_id);
        self.attachments.len() != before
    }

    /// Whether this template's working copy already holds an attachment under `name`.
    ///
    /// The operation's own documentation states the rule — "The name of the message
    /// template attachment has to be unique for each message template referenced by the
    /// $LATEST qualifier" — so a second upload claiming it conflicts with the first rather
    /// than replacing it.
    pub fn holds_attachment_named(&self, name: &str) -> bool {
        self.attachments
            .iter()
            .any(|attachment| attachment.name == name)
    }

    /// The bytes this template's attachments hold together.
    pub fn attachment_bytes(&self) -> usize {
        self.attachments
            .iter()
            .map(|attachment| attachment.body.len())
            .sum()
    }
}

impl ComAmazonawsQconnectWisdomserviceState {
    /// An `attachmentId` no attachment this store holds — on a working copy or in a
    /// published version's snapshot — already carries, or `None` when [`MINT_ATTEMPTS`]
    /// candidates of this sequence number's block were all taken.
    ///
    /// The form is the modelled `Uuid` the member declares, and uniqueness comes from the
    /// store's own sequence, which no two mints share, so the id names exactly one file in
    /// the download URL's path. What admits a read of that URL is the signature it carries
    /// (see [`sign_data_plane_url`]).
    pub fn mint_message_template_attachment_id(&mut self) -> Option<String> {
        let seq = self.next_seq();
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.holds_message_template_attachment_id(candidate))
    }

    /// Whether any message template this store holds carries an attachment under this id,
    /// on its working copy or in a version it published — a snapshot keeps the
    /// attachments it was published with, so an id a version still holds is taken.
    fn holds_message_template_attachment_id(&self, attachment_id: &str) -> bool {
        let working_copies = self
            .message_templates
            .values()
            .any(|template| template.attachment(attachment_id).is_some());
        working_copies
            || self
                .message_template_versions
                .values()
                .flat_map(|versions| versions.iter())
                .any(|version| version.snapshot.attachment(attachment_id).is_some())
    }

    /// The attachment a (resolved `knowledgeBaseId`, resolved `messageTemplateId`,
    /// `attachmentId`) triple names, or `None` when that knowledge base's template holds
    /// none under that id.
    pub fn message_template_attachment(
        &self,
        knowledge_base_id: &str,
        message_template_id: &str,
        attachment_id: &str,
    ) -> Option<&MessageTemplateAttachmentFile> {
        self.message_template(knowledge_base_id, message_template_id)?
            .attachment(attachment_id)
    }

    /// The attachment file one `attachmentId` names in ANY record this store holds of that
    /// message template: its working copy, or a version it published.
    ///
    /// What the DOWNLOAD plane resolves a published locator through.
    /// `DeleteMessageTemplateAttachment` removes a file from the working copy alone and
    /// states that "attachments on available message template versions will remain
    /// unchanged" (`smithy_query(query="doc",shape="DeleteMessageTemplateAttachment")`), so
    /// a file a published snapshot still carries is still a file this store holds and its
    /// locator still serves it. The working copy is preferred, so while both records carry
    /// the id the current file is what is served.
    pub fn stored_message_template_attachment(
        &self,
        knowledge_base_id: &str,
        message_template_id: &str,
        attachment_id: &str,
    ) -> Option<&MessageTemplateAttachmentFile> {
        // The template is reached through the knowledge base the locator names first: a
        // snapshot is reachable only through the template that published it.
        let message_template = self.message_template(knowledge_base_id, message_template_id)?;
        message_template.attachment(attachment_id).or_else(|| {
            self.message_template_versions_of(message_template_id)
                .iter()
                .find_map(|version| version.snapshot.attachment(attachment_id))
        })
    }

    /// The attachment bytes this store holds across every message template — what the
    /// aggregate ceiling [`MAX_STORED_MESSAGE_TEMPLATE_ATTACHMENT_BYTES`] is measured
    /// against.
    ///
    /// A published version carries the snapshot the publish took, so its attachments are
    /// counted too: they are bytes this process holds until the template goes.
    pub fn stored_message_template_attachment_bytes(&self) -> usize {
        let working_copies: usize = self
            .message_templates
            .values()
            .map(MessageTemplate::attachment_bytes)
            .sum();
        let versions: usize = self
            .message_template_versions
            .values()
            .flat_map(|versions| versions.iter())
            .map(|version| version.snapshot.attachment_bytes())
            .sum();
        working_copies + versions
    }
}
