// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-artifact-artifact service.
//!
//! AWS Artifact's report catalog is SERVICE-owned: the API has no `CreateReport`, so
//! every report a caller can see was published by AWS itself. This module therefore
//! holds the catalog as the state struct's [`Default`], which `engine::InMemoryEngine`
//! builds the first time any handler reaches the store. Seeding in `Default` — rather
//! than from the first read — is what keeps every report operation a pure reporting
//! operation: no `ListReports`, `GetReportMetadata` or `ListReportVersions` call writes
//! anything, so a read can never change what a later caller sees.
//!
//! The one thing a caller does create is a TERM GRANT: `GetTermForReport` mints the
//! acceptance receipt (`termToken`) that `GetReport` requires, and that token is
//! persisted here so `GetReport` can check it against the TERM it was issued for. A
//! grant is bound to the term document the caller accepted, not to the report it was
//! requested through: every version of a report is governed by one term document, so a
//! token minted for any report version governed by that document covers the others too,
//! and a token minted under a different term document covers none of them.
//!
//! Every report and term DOCUMENT lives here as bytes beside the record it belongs to,
//! so the URL `GetReport` and `GetTermForReport` announce resolves to a document this
//! service actually holds: the data plane behind that URL ([`crate::service`]) reads the
//! same [`Document`] this module seeded, and the URL names the endpoint the caller
//! reached this emulator on ([`document_url_host`]) rather than a real AWS host.
//!
//! The other caller-owned record is the account's SETTINGS: `PutAccountSettings` writes
//! the one `AccountSettings` record this store holds and `GetAccountSettings` reports it
//! back, so a setting a caller writes is what every later read of it answers with.
//!
//! COMPLIANCE INQUIRIES are the one thing a caller CREATES through this API:
//! `CreateComplianceInquiry` mints one, so the records live in the state struct rather
//! than in its [`Default`]. An inquiry carries its own clock-owed transition — AWS
//! answers the create with HTTP 202 and a non-terminal `PROCESSING` status — which
//! [`ComplianceInquiry::materialise`] applies from the stored due instant, so a reader
//! only ever materialises a change the clock had already made due. That same transition
//! RELEASES each query's answer — the `response` the retrieval over the published report
//! catalog produced for it and the `citations` naming where those sources are published,
//! which live in [`inquiry_answers`].
//!
//! An inquiry also carries the one TAG SET this service persists: `TagResource`,
//! `UntagResource` and `ListTagsForResource` act on the tags of the resource their
//! `resourceArn` names, and the only resource an account owns here is an inquiry (see
//! [`ComAmazonawsArtifactArtifactState::tagged_resource`]), so the tags live on the
//! inquiry record that `CreateComplianceInquiry` already accepts them with rather than in
//! a second map beside it. A tag write is therefore a modification of the INQUIRY, so it
//! moves that record's last-modified instant exactly as the clock-owed transition does
//! ([`ComplianceInquiry::record_modification`]).
//!
//! FEEDBACK is the other record a caller writes onto an inquiry:
//! `PutComplianceInquiryFeedback` records a rating on one of the inquiry's queries (or on
//! the inquiry's response as a whole when the request names no query), so an
//! [`InquiryFeedback`] lives on the inquiry record beside its tags, keyed by the
//! [`FeedbackTarget`] it was given on — the query AND the response revision of it. One
//! target carries at most ONE feedback ever: a second submission against a target that
//! already carries feedback is refused with [`feedback_already_submitted`] rather than
//! replacing it, which is the guard `PutComplianceInquiryFeedbackRequest$responseRevisionId`
//! advertises ("use this value to prevent submitting feedback on a stale response"), and
//! the record is reported back in the document [`ComplianceInquiry::export_document`]
//! renders. That
//! document's `feedback` key is this EMULATOR's rendering of what it holds, not modelled
//! content: AWS publishes an export as an opaque S3 object whose format the API does not
//! state (see [`INQUIRY_EXPORT_OBJECT`]), and the key is what makes the stored feedback
//! readable at all — the model declares no operation that reports it back. The
//! `clientToken` that call carries is remembered in
//! [`ComAmazonawsArtifactArtifactState::inquiry_feedback_tokens`], and the model has the
//! service IGNORE a later request carrying a remembered one, answering it with that call's
//! instant — so a token never produces a refusal here: the per-target rule above is the
//! only thing that refuses a submission, and it is reached by every request whose token this
//! service has not already recorded.
//!
//! An inquiry EXPORT is the second thing a caller creates: `ExportComplianceInquiry`
//! produces a document from one inquiry, so each call mints an [`InquiryExport`] whose id
//! the presigned URL it answers with carries. That document is served the same way a
//! report's is — from the bytes the export stored, under the endpoint the caller reached
//! ([`document_url_host`]) and a path this service publishes
//! ([`EXPORT_DOCUMENT_PATH_PREFIX`]) — so the URL the export announces resolves for
//! whoever was handed it, and it is authorised the same way too: the export's own
//! [`InquiryExport::download_token`] travels in that URL and is what the data plane
//! requires of a fetch ([`ComAmazonawsArtifactArtifactState::export_download_authorised`]),
//! exactly as a term grant's token authorises a report download.
//!
//! The CUSTOMER-AGREEMENT set is held the same way as the catalog, and for the same
//! reason: the API has no operation that creates, accepts or terminates one — an
//! account's agreements are established outside this API — so the set a caller can see is
//! persisted state built by this struct's [`Default`], and `ListCustomerAgreements` only
//! reads it. Unlike a report, a customer-agreement belongs to the calling identity's
//! ACCOUNT, which is what its ARN and its `awsAccountId` name.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use smithy_server_runtime::Violation;

pub use ::service_common::page_tokens::PageTokenKey;

use crate::ops::types::{
    AccountSettings, ConflictException, CustomerAgreementSummary, InquiryDetail, InquirySummary,
    QuerySummary, ReportDetail, ReportSummary, ResourceNotFoundException, ValidationException,
    ValidationExceptionField, NOTIFICATION_SUBSCRIPTION_STATUS_VALUES,
};

/// The ANSWER a compliance-inquiry query carries — the response this service retrieved for
/// it out of the published catalog, and the citations that support it.
mod inquiry_answers;
pub use inquiry_answers::*;

/// The ARN syntax the tagging surface reads a caller-supplied `resourceArn` against.
/// Re-exported by name, so a handler reaches it through `crate::state` like every other
/// rule this module decides.
mod resource_arn;
pub use resource_arn::is_resource_arn;

/// The accessibility-conformance class of the published report catalog: the rows AWS
/// publishes one of per evaluated product, which [`seed_catalog`] materialises beside
/// [`SEED`].
mod accessibility_reports;

/// The Region this service is served in, and the one every ARN it mints names.
///
/// This service's endpoints are PARTITIONAL: the model binds one endpoint per partition,
/// resolved at that partition's implicit global Region, so a call reaches the service in
/// that one Region whatever Region a caller configured a client for, and every resource
/// the service reports is homed there. For the partition this emulator writes its ARNs in
/// (`arn:aws:…`, the form [`::engine::emulator_arn`] builds) that Region is `us-east-1`.
pub const SERVICE_REGION: &str = "us-east-1";

/// The Region every ARN this service mints names — see [`SERVICE_REGION`].
///
/// A function rather than the constant at each use site, so an ARN builder reads the
/// service's own Region the one way and a caller-supplied value can never stand in for
/// it.
pub fn service_region() -> &'static str {
    SERVICE_REGION
}

/// The `resourceType` the modelled `ResourceNotFoundException` reports for a report
/// version that names nothing.
pub const REPORT_RESOURCE_TYPE: &str = "Report";

/// This service's ONE invalid-token text, independent of the envelope it is stated in: a
/// token (a `nextToken` or a `termToken`) this service did not issue, or one that does not
/// cover the requested term, is refused with this fixed text, which neither names the
/// check that rejected it nor echoes the token. The control plane states it as its
/// modelled `ValidationException` and the document data plane as its own refusal, so one
/// text covers both planes.
pub const INVALID_TOKEN_MESSAGE: &str = "The provided token is invalid.";

/// The largest page every paginated operation of this service serves, and the page size
/// used when a request names no `maxResults`: every one of them takes its `maxResults`
/// from `MaxResultsAttribute`, modelled `@range(min: 1, max: 300)`.
pub const REPORT_PAGE_LIMIT: i32 = 300;

/// `CustomerAgreementState`'s value for an agreement that is in force. The one state
/// `ListCustomerAgreements` reports: it "List[s] active customer-agreements applicable to
/// calling identity".
pub const AGREEMENT_STATE_ACTIVE: &str = "ACTIVE";

/// `CustomerAgreementState`'s value for an agreement the customer terminated.
pub const AGREEMENT_STATE_CUSTOMER_TERMINATED: &str = "CUSTOMER_TERMINATED";

/// `CustomerAgreementState`'s value for an agreement AWS terminated.
pub const AGREEMENT_STATE_AWS_TERMINATED: &str = "AWS_TERMINATED";

/// `AgreementType`'s value for an agreement accepted as AWS publishes it.
pub const AGREEMENT_TYPE_DEFAULT: &str = "DEFAULT";

/// `AgreementType`'s value for a published agreement accepted with modifications.
pub const AGREEMENT_TYPE_MODIFIED: &str = "MODIFIED";

/// `AgreementType`'s value for an agreement negotiated for one customer.
pub const AGREEMENT_TYPE_CUSTOM: &str = "CUSTOM";

/// How many term grants the store holds before `GetTermForReport` releases the oldest.
///
/// AWS publishes no bound on outstanding acceptances, so this is the emulator's OWN
/// bound on its state rather than a claim about the service: it is what stops a caller
/// accepting terms in a loop from growing `term_grants` without end. It is set far above
/// any workflow an emulator serves — a caller holds one grant per report version it
/// downloads — and the seeded catalog publishes far fewer versions than this.
pub const TERM_GRANT_CAPACITY: usize = 1_000;

/// `ValidationExceptionReason`'s value for a pagination or term token the service did
/// not issue.
pub const REASON_INVALID_TOKEN: &str = "invalidToken";

/// The path prefix the compliance-inquiry EXPORT documents are served under, within this
/// service's own endpoint (see [`export_document_path`]).
///
/// Like [`DOCUMENT_PATH_PREFIX`] it is not a modelled operation URI — every modelled one
/// is `/v1/compliance-inquiry/<verb>`, and the segment after this prefix is an inquiry id
/// rather than a verb — so an export download can never be mistaken for a control-plane
/// call, and `crate::service` can claim the paths this service publishes its exports at.
pub const EXPORT_DOCUMENT_PATH_PREFIX: &str = "/v1/compliance-inquiry/download";

/// The host a document URL names when the request carries neither a `Host` header nor an
/// authority to read the emulator's own endpoint from. A LOOPBACK literal: this emulator
/// serves the document the URL names, so a caller that follows it must come back here.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The path prefix the document data plane serves, under this service's own endpoint.
///
/// It is not a modelled operation URI — every modelled one is `/v1/report/<verb>` — so a
/// document fetch can never be mistaken for a control-plane call, and no control-plane
/// call can be mistaken for a document fetch.
pub const DOCUMENT_PATH_PREFIX: &str = "/v1/report/download";

/// The path segment naming a REPORT document, the thing `GetReport` hands a URL for.
pub const REPORT_DOCUMENT_KIND: &str = "report";

/// The path segment naming a TERM document, the thing `GetTermForReport` hands a URL for.
pub const TERM_DOCUMENT_KIND: &str = "term";

/// One stored document: the bytes this service serves for it, and what it is served as.
///
/// A document is PERSISTED, not rendered per request: the report and term URLs
/// `GetReport` and `GetTermForReport` announce resolve to the bytes held here, so two
/// fetches of one URL serve the same document and a fetch changes nothing.
#[derive(Debug, Clone)]
pub struct Document {
    /// The file name this document is published under — the last segment of its URL path.
    pub object: String,
    /// The `Content-Type` the data plane answers with, from the object's extension.
    pub content_type: &'static str,
    /// This document's IDENTITY: the SHA-256 of its bytes, hex-encoded.
    ///
    /// Two reports governed by the same term carry the same term document, and the term a
    /// download grant covers is identified by this digest rather than by the file name the
    /// document is published under — so the rule holds on content, not on a naming
    /// discipline the catalog happens to keep.
    pub digest: String,
    /// The document itself.
    pub bytes: Vec<u8>,
}

/// The `Content-Type` a published document's file name implies. The catalog publishes
/// report documents as PDF and term documents as plain text; anything else is served as
/// opaque bytes rather than mislabelled.
fn content_type_of(object: &str) -> &'static str {
    match object.rsplit_once('.').map(|(_, extension)| extension) {
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

impl Document {
    /// A document published as `object`, carrying `bytes`.
    pub fn new(object: &str, bytes: Vec<u8>) -> Self {
        let digest = {
            use ::sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            ::hex::encode(hasher.finalize())
        };
        Self {
            object: object.to_string(),
            content_type: content_type_of(object),
            digest,
            bytes,
        }
    }
}

/// The `NotificationSubscriptionStatus` an account carries before any
/// `PutAccountSettings` call.
///
/// AWS Artifact notifications are OPT-IN — `PutAccountSettings` is how a customer
/// subscribes — so an account nobody has subscribed reports the enum's
/// `NOT_SUBSCRIBED`, never an absent status.
pub const DEFAULT_NOTIFICATION_SUBSCRIPTION_STATUS: &str = "NOT_SUBSCRIBED";

/// The `resourceType` the modelled `ConflictException` reports for a compliance inquiry
/// id.
pub const INQUIRY_RESOURCE_TYPE: &str = "ComplianceInquiry";

/// How many compliance inquiries the store holds before `CreateComplianceInquiry`
/// refuses another.
///
/// AWS publishes no quota on compliance inquiries per account, so this is the emulator's
/// OWN bound on its state rather than a claim about the service: it is what stops a
/// caller submitting inquiries in a loop from growing `inquiries` without end. Nothing
/// is evicted to make room — an inquiry is a resource the caller created and a later
/// `GetComplianceInquiryMetadata` must still find it — so the create is refused instead.
pub const INQUIRY_CAPACITY: usize = 1_000;

/// How many `clientToken` records ONE operation remembers for replay before the oldest is
/// forgotten — `CreateComplianceInquiry`'s and `PutComplianceInquiryFeedback`'s tokens are
/// each held to this bound. The same kind of emulator-owned bound as
/// [`INQUIRY_CAPACITY`]: AWS documents no lifetime for this token, so the records are
/// bounded by count rather than expired on a span this emulator would be inventing.
pub const INQUIRY_IDEMPOTENCY_CAPACITY: usize = 1_000;

/// How many FEEDBACK records one inquiry holds before `PutComplianceInquiryFeedback`
/// refuses another.
///
/// One record per [`FeedbackTarget`], and a request may name any `responseRevisionId` an
/// `Integer` carries, so the set an inquiry can hold is otherwise unbounded. The same kind
/// of emulator-owned bound as [`INQUIRY_CAPACITY`], and refused the same way rather than
/// evicted: a record that was forgotten would let a response revision be rated a second
/// time, which is exactly what a feedback record exists to prevent.
pub const FEEDBACK_PER_INQUIRY_CAPACITY: usize = 1_000;

/// How many units of work ONE submission can name — the bound the API itself states:
/// `InquiryFileContent$fileSections` is `@length(min: 0, max: 30)`
/// (`smithy_query(query="shape",shape="InquiryFileContent")`), and a text submission is
/// one unit, so this is the most queries any inquiry owns.
///
/// Unlike [`INQUIRY_CAPACITY`] this is not an emulator-owned bound: the boundary validator
/// already refuses a request naming more sections, which is why no handler has to. It is
/// stated here, and applied where the records are built ([`InquiryContentRecord::work_units`]),
/// so the set one request can materialise is bounded at its source rather than by the
/// request shape alone.
pub const QUERIES_PER_INQUIRY_CEILING: usize = 30;

/// How many compliance-inquiry EXPORTS the store holds before the oldest is released.
///
/// `ExportComplianceInquiry` produces a new document on every call, so the records would
/// otherwise grow without end. The same kind of emulator-owned bound as
/// [`TERM_GRANT_CAPACITY`]: AWS publishes no quota on exports, and the oldest export is
/// the one whose URL a caller is least likely to still be following, so it is released
/// first rather than refusing the call.
pub const INQUIRY_EXPORT_CAPACITY: usize = 1_000;

/// The file name an exported compliance-inquiry report carries in its document URL.
///
/// A JSON document, and named one: AWS publishes the export as an opaque S3 object whose
/// format the API does not state, and this emulator renders the one it CAN render from
/// what it holds — the inquiry, its queries and their statuses. Nothing here claims a
/// typeset report it did not produce.
pub const INQUIRY_EXPORT_OBJECT: &str = "compliance-inquiry-report.json";

/// The media type [`INQUIRY_EXPORT_OBJECT`] is served with.
pub const INQUIRY_EXPORT_CONTENT_TYPE: &str = "application/json";

/// `ValidationExceptionReason`'s value for a member whose value the service itself
/// refused — what `fieldList` names the member in.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// `ValidationExceptionReason`'s value for a request refused on the STATE of the resource
/// it named rather than on a member of its own input — the enum's own catch-all, which is
/// what a precondition on a resource's lifecycle is stated under: no member of the request
/// failed, so there is no member for `fieldList` to name.
pub const REASON_OTHER: &str = "other";

/// How long after the create call an inquiry's processing is due to be over.
///
/// AWS answers `CreateComplianceInquiry` with HTTP 202 and a `PROCESSING` inquiry, so
/// the terminal status is owed by the CLOCK rather than by any later call. AWS publishes
/// no duration for it; this is the emulator's own span, chosen so a caller that polls
/// straight after the create still observes the non-terminal status the create returned.
pub const INQUIRY_PROCESSING_SPAN: Duration = Duration::from_millis(500);

/// `InquiryStatus`'s non-terminal value, which a created inquiry starts in.
pub const INQUIRY_STATUS_PROCESSING: &str = "PROCESSING";

/// `InquiryStatus`'s value for an inquiry whose processing is over.
pub const INQUIRY_STATUS_COMPLETED: &str = "COMPLETED";

/// `InquiryStatus`'s value for an inquiry whose processing could not be performed.
pub const INQUIRY_STATUS_FAILED: &str = "FAILED";

/// `InquiryStatusMessage`'s value for an inquiry that is still being processed.
pub const INQUIRY_MESSAGE_IN_PROGRESS: &str = "Compliance inquiry processing is in-progress.";

/// `InquiryStatusMessage`'s value for an inquiry whose processing is over.
pub const INQUIRY_MESSAGE_COMPLETE: &str = "Compliance inquiry processing is complete.";

/// `InquiryStatusMessage`'s value for an inquiry whose processing itself failed.
pub const INQUIRY_MESSAGE_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";

/// `QueryStatus`'s non-terminal value.
pub const QUERY_STATUS_PROCESSING: &str = "PROCESSING";

/// `QueryStatus`'s value for a query whose processing is over.
pub const QUERY_STATUS_COMPLETED: &str = "COMPLETED";

/// `QueryStatusMessage`'s value for a query that is still being processed.
pub const QUERY_MESSAGE_IN_PROGRESS: &str = "Query processing is in-progress.";

/// `QueryStatusMessage`'s value for a query whose processing is over.
pub const QUERY_MESSAGE_COMPLETE: &str = "Query processing is complete.";

/// `InputSource`'s value for an inquiry submitted as a single query.
pub const INPUT_SOURCE_TEXT: &str = "TEXT";

/// `InputSource`'s value for an inquiry submitted as an uploaded document.
pub const INPUT_SOURCE_FILE: &str = "FILE";

/// `InquirySupportMode`'s one value, which is also the default the member's own
/// documentation states: "Defaults to AI_ONLY if not specified."
/// (`CreateComplianceInquiryRequest$supportMode`). It is the whole of the modelled enum
/// (`smithy_query(query="shape",shape="InquirySupportMode")`), so it is the mode every
/// inquiry this service holds is processed under: a request carrying any other value is
/// refused against that enum before a handler runs.
pub const SUPPORT_MODE_AI_ONLY: &str = "AI_ONLY";

/// `ReviewType`'s value for a response reviewed by the service's own model.
///
/// The review every query of every inquiry this service holds carries: the one support
/// mode the model admits ([`SUPPORT_MODE_AI_ONLY`]) "provides AI-generated responses"
/// (`CreateComplianceInquiryRequest$supportMode`), and no request can ask for another, so
/// nothing here is submitted for a human expert review.
pub const REVIEW_TYPE_AI: &str = "AI";

/// The longest text a `LongStringAttribute` member of this service can report:
/// `QuerySummary$query`, `QuerySummary$response` and `Citation$sourceContent` all target
/// it, and it is `@length(min: 1, max: 2048)`.
pub const QUERY_TEXT_LIMIT: usize = 2048;

/// `label` as `QuerySummary$query` can report it, or `None` when nothing reportable
/// remains of it.
///
/// The member targets `LongStringAttribute` — `@length(min: 1, max: 2048)`, `@pattern
/// ^[^<>]*$` — which is the one shape rule [`reportable_long_text`] applies for every
/// member of this service targeting it. Submitted query text already satisfies the shape
/// (the request carried it through the same one), so this only ever bites on a handle
/// read off a member with looser constraints, such as the inquiry's title.
pub fn reportable_query_text(label: &str) -> Option<String> {
    reportable_long_text(label)
}

/// How many tags one resource carries at most.
///
/// `TagsMap` is modelled `@length(max: 50)` and `ListTagsForResource` reports a resource's
/// WHOLE tag set in that one map, so a resource holding more tags than the map admits
/// could not be reported at all: the bound is the resource's, not just one request's.
pub const TAGS_PER_RESOURCE_LIMIT: usize = 50;

/// The tag-key namespace Amazon Web Services keeps for the tags it applies itself.
///
/// Established AWS-wide tagging semantics: a key under this prefix belongs to AWS, so no
/// caller may create, change or remove one through a service's tagging API, and no such
/// key is ever part of a caller-created tag set. `TagKey`'s modelled pattern admits `:`,
/// so nothing at the boundary refuses such a key — the rule lives here, beside the
/// per-resource count every tag write is checked against.
///
/// Tag keys are case-sensitive strings, and what is reserved is this literal lowercase
/// prefix: a key that merely contains it, or starts with `aws` without the colon, is an
/// ordinary tag ([`is_reserved_tag_key`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// A PREFIX test on the key as text, so a key carrying the prefix anywhere but at its
/// start is an ordinary tag.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// The first key of `keys` that is in the reserved namespace, if any — the one test every
/// path that writes or removes tags states the rule through.
pub fn reserved_tag_key<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// The keys `keys` names as ONE field value: sorted, and joined with `,`.
///
/// What a record of a tag write names the keys it wrote or removed in, so the record is
/// the same whatever order the request listed them in (a `TagsMap` arrives as an unordered
/// map). `TagKey`'s modelled pattern `^[a-zA-Z0-9\s_.:/=+\-@]*$`
/// (`smithy_query(query="shape",shape="TagsMap")`) admits no comma, so the separator
/// cannot be mistaken for part of a key.
pub fn tag_key_list<'a, I>(keys: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.join(",")
}

/// The visibility of a report AWS publishes in the Artifact catalog: it is visible to
/// every account, as opposed to a report scoped to one customer.
pub const VISIBILITY_PUBLIC: &str = "PUBLIC";

/// `AcceptanceType`'s `EXPLICIT`: downloading the report requires the explicit
/// click-through acceptance of the term governing it.
pub const ACCEPTANCE_TYPE_EXPLICIT: &str = "EXPLICIT";

/// `AcceptanceType`'s `PASSTHROUGH`: downloading the report requires no explicit
/// click-through acceptance of the term governing it.
pub const ACCEPTANCE_TYPE_PASSTHROUGH: &str = "PASSTHROUGH";

/// One version of one AWS Artifact report, as this emulator persists it.
///
/// Every member of `ReportSummary` and `ReportDetail` that can differ between two
/// versions of the same report lives here; the members shared by all of a report's
/// versions live on [`Report`].
#[derive(Debug, Clone)]
pub struct ReportVersion {
    /// `VersionAttribute`, `@range(min: 1)`.
    pub version: i64,
    /// One of `PublishedState`'s values.
    pub state: String,
    /// One of `UploadState`'s values.
    pub upload_state: String,
    /// "The message associated with the current upload state."
    pub status_message: Option<String>,
    /// The PUBLISHED description of the document this version carries — the publisher's
    /// own prose about it, which is what `ReportSummary$description` and
    /// `ReportDetail$description` report (both target `LongStringAttribute`,
    /// `@length(min: 1, max: 2048)`, `@pattern ^[^<>]*$`). Held per version because the
    /// member is projected per version, so a report that ever published two descriptions
    /// can hold both.
    pub description: String,
    /// "Timestamp indicating the report resource effective start."
    pub period_start: SystemTime,
    /// "Timestamp indicating the report resource effective end."
    pub period_end: SystemTime,
    /// When AWS published this version — `createdAt` and, since a published version is
    /// never edited in place, `lastModifiedAt`.
    pub published_at: SystemTime,
    /// "Sequence number to enforce optimistic locking", `@range(min: 1)`.
    pub sequence_number: i64,
    /// The report document THIS version publishes. Each version has its own: a version
    /// is published because its document differs from the previous one's.
    pub document: Document,
}

/// One AWS Artifact report: the members every version of it shares, plus its versions.
#[derive(Debug, Clone)]
pub struct Report {
    /// `ReportId`, `^report-[a-zA-Z0-9]{16}$`.
    pub id: String,
    pub name: String,
    pub series: String,
    pub category: String,
    pub company_name: String,
    pub product_name: String,
    /// One of `AcceptanceType`'s values: [`ACCEPTANCE_TYPE_EXPLICIT`] requires the
    /// click-through acceptance of the report's term and
    /// [`ACCEPTANCE_TYPE_PASSTHROUGH`] does not.
    ///
    /// A REPORTED attribute: it tells a client whether it has to put the term document in
    /// front of a person before asking for the receipt, and it decides nothing about the
    /// receipt itself — `GetReport` requires a `termToken` of every download whatever this
    /// value is ([`ComAmazonawsArtifactArtifactState::download_authorised`]).
    pub acceptance_type: String,
    /// The TERM document governing this report: the thing a caller accepts to download
    /// any version of it. One per report, not per version — which is why a term grant
    /// minted through one version covers the others.
    ///
    /// The document carries its own published file name ([`Document::object`]), which is
    /// what the term URL is built from, so the name is held once beside the bytes it
    /// names rather than a second time on this record.
    pub term: Document,
    /// Who can see this report — [`VISIBILITY_PUBLIC`] for everything AWS publishes in
    /// the catalog. Stored rather than assumed, so the projections report the record.
    pub visibility: String,
    /// WHERE this compliance document is published — the locator of the source itself,
    /// which is what a citation of it carries (`Citation$sourceLink`, "Link to the
    /// compliance source").
    ///
    /// A property of the SOURCE, so it is held on the record beside the report's name and
    /// its published text: a citation reports this stored value
    /// ([`QueryCitation::source_link`]), which is therefore the same whichever endpoint a
    /// reader reached this emulator on and the same in every projection of that citation.
    pub source_link: String,
    /// Every published version, keyed by version number. A `BTreeMap` is what gives
    /// `ListReportVersions` the ascending total order its pagination token indexes into,
    /// and what makes "the latest version" the last entry.
    ///
    /// INVARIANT: never empty — a report exists because AWS published a version of it.
    pub versions: BTreeMap<i64, ReportVersion>,
}

impl Report {
    /// The version `requested` names, or the newest published one when it names none.
    ///
    /// TOTAL: `None` means "this report has no such version" — including the case where
    /// the request named none and the report holds none at all — so a caller answers it
    /// with a modelled error and never has to re-derive what was asked for.
    pub fn resolve(&self, requested: Option<i64>) -> Option<&ReportVersion> {
        match requested {
            Some(version) => self.versions.get(&version),
            None => self.versions.values().next_back(),
        }
    }

    /// The `ReportSummary` projection `ListReports` and `ListReportVersions` return for
    /// one version of this report.
    ///
    /// A catalog listing projects the PUBLISHED report: who published it, what period it
    /// covers, which version this entry is and who may see it. The document-upload
    /// progress members (`uploadState`, `statusMessage`) are not part of that projection —
    /// the recorded AWS response for a published, PUBLIC catalog entry carries neither,
    /// while the single-report metadata read ([`Report::to_detail`]) carries both — so
    /// they are left unset here and reported there, from the one stored record.
    pub fn to_summary(&self, version: &ReportVersion) -> ReportSummary {
        ReportSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            state: Some(version.state.clone()),
            arn: Some(report_arn(&self.id, version.version)),
            version: Some(version.version),
            upload_state: None,
            description: Some(version.description.clone()),
            period_start: Some(::aws_smithy_types::DateTime::from(version.period_start)),
            period_end: Some(::aws_smithy_types::DateTime::from(version.period_end)),
            series: Some(self.series.clone()),
            category: Some(self.category.clone()),
            company_name: Some(self.company_name.clone()),
            product_name: Some(self.product_name.clone()),
            status_message: None,
            acceptance_type: Some(self.acceptance_type.clone()),
            visibility: Some(self.visibility.clone()),
        }
    }

    /// The `ReportDetail` projection `GetReportMetadata` returns for one version of this
    /// report. Every member it shares with [`Report::to_summary`] takes the same value:
    /// both read the one stored record.
    pub fn to_detail(&self, version: &ReportVersion) -> ReportDetail {
        ReportDetail {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: Some(version.description.clone()),
            period_start: Some(::aws_smithy_types::DateTime::from(version.period_start)),
            period_end: Some(::aws_smithy_types::DateTime::from(version.period_end)),
            created_at: Some(::aws_smithy_types::DateTime::from(version.published_at)),
            // A published version is never edited in place: AWS publishes a new version
            // instead, so the last modification of THIS version is its publication.
            last_modified_at: Some(::aws_smithy_types::DateTime::from(version.published_at)),
            // "Timestamp indicating when the report resource was deleted" — only a
            // deleted report carries one, and the catalog holds none.
            deleted_at: None,
            state: Some(version.state.clone()),
            arn: Some(report_arn(&self.id, version.version)),
            series: Some(self.series.clone()),
            category: Some(self.category.clone()),
            company_name: Some(self.company_name.clone()),
            product_name: Some(self.product_name.clone()),
            // The term is the REPORT's, one for every version of it, so the ARN that
            // identifies it is read off that one stored document and never off the version
            // this projection was asked for.
            term_arn: Some(term_arn(&self.term)),
            version: Some(version.version),
            acceptance_type: Some(self.acceptance_type.clone()),
            sequence_number: Some(version.sequence_number),
            upload_state: Some(version.upload_state.clone()),
            status_message: version.status_message.clone(),
            visibility: Some(self.visibility.clone()),
        }
    }
}

/// One AWS Artifact customer-agreement resource, as this emulator persists it.
///
/// A customer-agreement is the calling identity's acceptance of an AWS-published
/// agreement; this record holds every member of `CustomerAgreementSummary` that can
/// differ between two of them.
#[derive(Debug, Clone)]
pub struct CustomerAgreement {
    /// `CustomerAgreementIdAttribute`, `^customer-agreement-[a-zA-Z0-9]{16}$`.
    pub id: String,
    /// "Name of the customer-agreement resource."
    pub name: String,
    /// "Description of the resource."
    pub description: String,
    /// The AWS-published agreement resource this customer-agreement represents, which
    /// its `agreementArn` names.
    pub agreement_id: String,
    /// One of `AgreementType`'s values ([`AGREEMENT_TYPE_DEFAULT`],
    /// [`AGREEMENT_TYPE_MODIFIED`], [`AGREEMENT_TYPE_CUSTOM`]).
    pub agreement_type: String,
    /// One of `CustomerAgreementState`'s values ([`AGREEMENT_STATE_ACTIVE`],
    /// [`AGREEMENT_STATE_CUSTOMER_TERMINATED`], [`AGREEMENT_STATE_AWS_TERMINATED`]).
    pub state: String,
    /// "Timestamp indicating when the agreement became effective."
    pub effective_start: SystemTime,
    /// "Timestamp indicating when the agreement was terminated" — so only a terminated
    /// agreement carries one.
    pub effective_end: Option<SystemTime>,
    /// "Terms required to accept the agreement resource." `AgreementTerms`, at most 10.
    pub acceptance_terms: Vec<String>,
    /// "Terms required to terminate the customer-agreement resource." `AgreementTerms`,
    /// at most 10.
    pub terminate_terms: Vec<String>,
}

impl CustomerAgreement {
    /// True when this agreement is in `CustomerAgreementState`'s `ACTIVE` state — the
    /// agreements `ListCustomerAgreements` reports.
    pub fn is_active(&self) -> bool {
        self.state == AGREEMENT_STATE_ACTIVE
    }

    /// The `CustomerAgreementSummary` projection `ListCustomerAgreements` returns.
    pub fn to_summary(&self) -> CustomerAgreementSummary {
        CustomerAgreementSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(customer_agreement_arn(&self.id)),
            agreement_arn: Some(agreement_arn(&self.agreement_id)),
            aws_account_id: Some(::engine::emulator_account_id().to_string()),
            // "ARN of the organization that owns the resource": this emulator presents a
            // single standalone account, so no organization owns the agreement and
            // naming one would claim an organization that does not exist.
            organization_arn: None,
            effective_start: Some(::aws_smithy_types::DateTime::from(self.effective_start)),
            effective_end: self.effective_end.map(::aws_smithy_types::DateTime::from),
            state: Some(self.state.clone()),
            description: Some(self.description.clone()),
            acceptance_terms: Some(self.acceptance_terms.clone()),
            terminate_terms: Some(self.terminate_terms.clone()),
            r#type: Some(self.agreement_type.clone()),
        }
    }
}

/// A term grant: the acceptance receipt `GetTermForReport` minted, and the TERM it
/// covers.
///
/// The grant records the term document the caller accepted, because that is what the
/// receipt is for: `GetReport` serves a document for a token whose term is the term
/// governing the requested report, whichever report version the acceptance was
/// requested through.
#[derive(Debug, Clone)]
pub struct TermGrant {
    /// The `termToken` handed to the caller: this acceptance's RECEIPT, and the one thing
    /// that authorises a download of a report whose term it covers. Minted by the one
    /// receipt rule ([`mint_download_receipt`]) rather than from the counter the grant's
    /// store id comes from, so a caller that accepted nothing cannot derive it; its 32 hex
    /// digits match `ShortStringAttribute`'s `^[a-zA-Z0-9_\-\s]*$` within 1..256 bytes,
    /// which is what `GetReport`'s `termToken` member admits.
    pub token: String,
    /// The term document this acceptance covers, by its CONTENT digest — the grant's
    /// subject. Identity is the document's own bytes, so two reports governed by the same
    /// term share one grant and two different terms never do, whatever file names the
    /// catalog publishes them under.
    pub term_digest: String,
    /// The report version the acceptance was requested through, kept so an acceptance can
    /// be told apart from another acceptance of the same term in the store. It is NOT
    /// what the grant authorises — the term above is — and no check reads it.
    pub report_id: String,
    pub report_version: i64,
    /// When the acceptance was recorded. Grants are released by [`TERM_GRANT_CAPACITY`]
    /// rather than by age, so nothing expires on this: it is the record of when the
    /// caller accepted.
    pub issued_at: SystemTime,
}

/// The account-wide Artifact settings, as this emulator persists them.
///
/// AWS Artifact's settings are a SINGLE per-account record: neither
/// `GetAccountSettings` nor `PutAccountSettings` carries a member naming which settings
/// to read or write, so the store holds exactly one and every caller of this emulator
/// observes the same one.
#[derive(Debug, Clone)]
pub struct AccountSettingsRecord {
    /// "Notification subscription status of the customer" — one of the values
    /// [`NOTIFICATION_SUBSCRIPTION_STATUS_VALUES`](crate::ops::types::NOTIFICATION_SUBSCRIPTION_STATUS_VALUES)
    /// lists for the model's `NotificationSubscriptionStatus` enum. Written only through
    /// [`AccountSettingsRecord::set_notification_subscription_status`], which ties the
    /// stored string to that set.
    notification_subscription_status: String,
}

impl Default for AccountSettingsRecord {
    fn default() -> Self {
        Self {
            notification_subscription_status: DEFAULT_NOTIFICATION_SUBSCRIPTION_STATUS.to_string(),
        }
    }
}

impl AccountSettingsRecord {
    /// The stored status — always a value of the modelled enum.
    pub fn notification_subscription_status(&self) -> &str {
        &self.notification_subscription_status
    }

    /// Record `status` as the account's notification subscription status.
    ///
    /// The debug assertion states the invariant the store holds rather than correcting a
    /// caller: a value outside the modelled enum reaching here would be a boundary-binding
    /// bug (the gateway and the framework's validator both refuse one), and a release
    /// build still stores what the caller asked for rather than silently repairing it.
    pub fn set_notification_subscription_status(&mut self, status: &str) {
        debug_assert!(
            NOTIFICATION_SUBSCRIPTION_STATUS_VALUES.contains(&status),
            "a notification subscription status must be one of \
             {NOTIFICATION_SUBSCRIPTION_STATUS_VALUES:?}, got {status:?}"
        );
        self.notification_subscription_status = status.to_string();
    }

    /// The `AccountSettings` projection both account-settings operations return, so the
    /// answer to a `Get` and the answer to a `Put` are the same projection of the one
    /// stored record.
    pub fn to_settings(&self) -> AccountSettings {
        AccountSettings {
            notification_subscription_status: Some(self.notification_subscription_status.clone()),
        }
    }
}

/// What a caller submitted as an inquiry's content, as `InquiryContent`'s two union
/// members carry it.
///
/// The content is kept because it IS the created resource: the inquiry's queries are
/// enumerated from it, and `ExportComplianceInquiry` reports on it. It is `@sensitive`
/// in the model, so it is never written to a log or an error message.
#[derive(Debug, Clone)]
pub enum InquiryContentRecord {
    /// `InquiryContent$query` — one query, submitted as text.
    Text(String),
    /// `InquiryContent$fileContent` — an uploaded document, plus "the list of file
    /// sections/sheets to process" (`InquiryFileContent$fileSections`).
    File {
        sections: Vec<String>,
        content: Vec<u8>,
    },
}

impl InquiryContentRecord {
    /// The `InputSource` this content was submitted through.
    pub fn input_source(&self) -> &'static str {
        match self {
            Self::Text(_) => INPUT_SOURCE_TEXT,
            Self::File { .. } => INPUT_SOURCE_FILE,
        }
    }

    /// The display name this content DERIVES, when it carries one.
    ///
    /// A submission made of a single text query names itself: the query text is the
    /// content, so the service's display name for the inquiry is that text and a caller
    /// finds its inquiry in a listing under exactly what it submitted. An uploaded
    /// document carries no single query to name the inquiry after — its queries are
    /// written inside the file — so it derives nothing and the submitted title stands.
    ///
    /// TOTAL: `None` means "this content derives no name", which is what makes
    /// [`ComplianceInquiry::display_name`] fall back to the title rather than guessing.
    pub fn derived_name(&self) -> Option<&str> {
        match self {
            Self::Text(query) => Some(query.as_str()),
            Self::File { .. } => None,
        }
    }

    /// The units of work this content submits, in the order they are processed — one
    /// query each, reported under the handle the submission gave it.
    ///
    /// A text submission submits exactly ONE: the query it carries, which IS the unit of
    /// work. An uploaded document is processed SECTION BY SECTION —
    /// `InquiryFileContent$fileSections` is the "[l]ist of file sections/sheets to
    /// process" — so it submits one unit per section it names, and the document as a
    /// whole, under the `title` the submission carries, when it names none.
    ///
    /// A unit is reported under its own handle and nothing more: the questions a section
    /// holds are written INSIDE the document, no document understanding runs in this
    /// emulator, and no question text is invented for them. What each unit is ANSWERED
    /// with is the retrieval over the published catalog for the handle it carries (see
    /// [`InquiryQuery::answer`]).
    ///
    /// Content with [`nothing_to_process`](Self::nothing_to_process) submits NO unit: there
    /// is nothing for a unit of work to be about, which is the one thing that makes an
    /// inquiry own no work at all.
    pub fn work_units(&self, title: &str) -> Vec<String> {
        if self.nothing_to_process() {
            return Vec::new();
        }
        match self {
            Self::Text(query) => vec![query.clone()],
            Self::File { sections, .. } => {
                if sections.is_empty() {
                    vec![title.to_string()]
                } else {
                    // Held to the bound the API states ([`QUERIES_PER_INQUIRY_CEILING`]),
                    // so what one request body can materialise here is bounded whatever
                    // reaches this record: a request naming more sections than the member
                    // admits is refused at the boundary and never gets here, so this
                    // takes nothing away from any request the service serves.
                    sections
                        .iter()
                        .take(QUERIES_PER_INQUIRY_CEILING)
                        .cloned()
                        .collect()
                }
            }
        }
    }

    /// Whether this content carries NOTHING to process — the one thing that is empty about
    /// a submission, and the only reason an inquiry gets as far as no unit of work at all
    /// ([`work_units`](Self::work_units) is empty for exactly this content and no other).
    ///
    /// `InquiryFileContent$content` is `@required` but the model bounds its length in no
    /// other way, so an upload carrying zero bytes is a well-formed request with no
    /// document behind it. A submission that DID carry something is processed whatever
    /// handle its units of work turn out to be reportable under
    /// ([`reportable_query_text`]): what a unit can be REPORTED as is a property of the
    /// member that reports it, never of whether there was work to do — which is what
    /// [`ComplianceInquiry::materialise`] reads this, rather than the reported query list,
    /// to decide.
    pub fn nothing_to_process(&self) -> bool {
        match self {
            Self::Text(query) => query.is_empty(),
            Self::File { content, .. } => content.is_empty(),
        }
    }
}

/// One query within one compliance inquiry — the `QuerySummary` projection
/// `ListComplianceInquiryQueries` reports, and what `PutComplianceInquiryFeedback`'s
/// `queryIdentifier` names.
#[derive(Debug, Clone)]
pub struct InquiryQuery {
    /// "Sequential identifier of the query within the inquiry", 1-based.
    pub identifier: i32,
    /// "The actual query text" — the handle the submitted content gave this unit of
    /// work (see [`InquiryContentRecord::work_units`]), rendered inside the modelled
    /// bounds of the member that reports it ([`reportable_query_text`]).
    pub query: String,
    /// "Generated response to the query", and "[s]upporting citations for the response",
    /// as the retrieval over the published report catalog produced them for this query's
    /// text ([`answer_from_catalog`]).
    ///
    /// `None` when the catalog holds nothing bearing on the query: there is no passage to
    /// quote, so the members stay absent rather than carrying text no source supports.
    /// Produced when the query was SUBMITTED and reported once the clock has made its
    /// processing due ([`Self::reported_answer`]) — the catalog is service-owned and never
    /// written, so the answer does not depend on when the retrieval ran.
    pub answer: Option<QueryAnswer>,
    /// One of `ReviewType`'s values — the review the inquiry's support mode asks for,
    /// which for the one mode the model admits is [`REVIEW_TYPE_AI`].
    pub review_type: Option<String>,
    /// One of `QueryStatus`'s values.
    pub status: String,
    /// One of `QueryStatusMessage`'s values.
    pub status_message: String,
    pub created_at: SystemTime,
}

impl InquiryQuery {
    /// The answer this query REPORTS, which is the one it holds once its processing is no
    /// longer in progress.
    ///
    /// A query still being processed reports no response and no citation: the answer is
    /// released when the processing that produced it is over, so a client polling for it
    /// sees it appear exactly when the status says it may.
    pub fn reported_answer(&self) -> Option<&QueryAnswer> {
        self.answer
            .as_ref()
            .filter(|_| self.status != QUERY_STATUS_PROCESSING)
    }

    /// The `QuerySummary` projection `ListComplianceInquiryQueries` reports.
    ///
    /// `response` and `citations` are the answer this query reports, both read off the one
    /// stored [`QueryAnswer`] so the response and the sources supporting it cannot
    /// disagree; a citation's link is the stored link of the source it names
    /// ([`QueryCitation::source_link`]), so what a reader sees does not depend on the
    /// endpoint it reached this emulator on.
    ///
    /// `updatedResponseVersions` is absent rather than empty: a response version records an
    /// EDIT to a response, and nothing in this emulator edits one, so there is no version
    /// history to report.
    pub fn to_summary(&self) -> QuerySummary {
        let answer = self.reported_answer();
        QuerySummary {
            query_identifier: self.identifier,
            query: self.query.clone(),
            response: answer.map(|answer| answer.response.clone()),
            review_type: self.review_type.clone(),
            citations: answer.map(|answer| answer.reported_citations()),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_response_versions: None,
        }
    }
}

/// WHAT one piece of feedback is about: the query it was given on — `None` for the
/// inquiry's response as a whole, which the request names no `queryIdentifier` for — and
/// the response revision of that query the request named, `None` when it named none.
///
/// This pair is the uniqueness key of a feedback record: a response revision accepts
/// exactly one submitted feedback, so the pair is what a second submission is refused
/// against. A revision the inquiry has no feedback for yet is a target of its own, which
/// is what makes feedback on a LATER revision of the same query a new record rather than a
/// conflict.
pub type FeedbackTarget = (Option<i32>, Option<i32>);

/// One piece of feedback a caller submitted on a compliance inquiry, as
/// `PutComplianceInquiryFeedback` persists it.
///
/// The record is keyed on the inquiry by its [`FeedbackTarget`], and a target that already
/// carries a record is REFUSED a second one: feedback is submitted once per response
/// revision, never replaced, which is also what bounds the set an inquiry can hold to one
/// record per (query, revision) it has been rated on.
#[derive(Debug, Clone)]
pub struct InquiryFeedback {
    /// "The sequential identifier of the query to provide feedback on", or `None` when the
    /// request named none and the feedback is on the inquiry's response as a whole.
    pub query_identifier: Option<i32>,
    /// One of `FeedbackRating`'s values.
    pub rating: String,
    /// "The reason codes that describe why you rated the response" — `FeedbackReasonCode`
    /// values, ascending and without repeats: which reasons were given is a SET, so
    /// neither the order a request listed them in nor a repeat is part of it.
    pub reason_codes: Vec<String>,
    /// "An optional comment for the feedback." `@sensitive` in the model, so it is never
    /// written to a log or an error message.
    pub comment: Option<String>,
    /// "The response revision ID", as the request named it, and half of the
    /// [`FeedbackTarget`] this record is keyed by.
    ///
    /// Part of the record's [`FeedbackTarget`]: the revision is what feedback is submitted
    /// once per, so two submissions naming different revisions of one query are two
    /// records and a second submission naming the same one is refused.
    ///
    /// It is not compared against a GENERATED revision, though: nothing in this emulator
    /// edits a response (see [`QueryAnswer`]), so no stored revision exists for a
    /// request's value to be stale against, and no staleness refusal is synthesised for a
    /// comparison nothing could make.
    pub response_revision_id: Option<i32>,
    /// "The timestamp when the feedback was submitted" — what the call answered with.
    pub submitted_at: SystemTime,
}

impl InquiryFeedback {
    /// The [`FeedbackTarget`] this record was submitted against, so the key a stored record
    /// is held under is read off the record itself rather than rebuilt beside it.
    pub fn target(&self) -> FeedbackTarget {
        (self.query_identifier, self.response_revision_id)
    }
}

/// One compliance inquiry a caller created, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ComplianceInquiry {
    /// `InquiryId`, `^compliance-inquiry-[a-zA-Z0-9]{16}$`.
    pub id: String,
    pub arn: String,
    /// "Title of the inquiry" (`CreateComplianceInquiryRequest$name`), as the caller
    /// submitted it. What a READER sees is
    /// [`display_name`](Self::display_name): a submission that derives its own name
    /// (a single text query) is reported under that name, so this member is the title
    /// of record rather than the projected one.
    pub name: String,
    /// One of `InquiryStatus`'s values.
    pub status: String,
    /// One of `InquiryStatusMessage`'s values.
    pub status_message: String,
    /// One of `InquirySupportMode`'s values, defaulted to [`SUPPORT_MODE_AI_ONLY`] when
    /// the request named none — and always the mode this submission IS processed under:
    /// the modelled enum admits that one mode, so a read can never advertise a support
    /// mode the inquiry's queries were not reviewed in ([`REVIEW_TYPE_AI`]).
    pub support_mode: String,
    pub content: InquiryContentRecord,
    /// The queries this inquiry owns, by ascending `queryIdentifier`.
    pub queries: Vec<InquiryQuery>,
    pub created_at: SystemTime,
    /// "Timestamp indicating when the resource was last modified."
    pub updated_at: SystemTime,
    /// When this inquiry's processing is due to be over, while it still is.
    ///
    /// `None` once the inquiry is in a terminal status. Stored rather than slept on:
    /// [`materialise`](Self::materialise) applies the transition the clock already owed.
    pub processing_due_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The feedback `PutComplianceInquiryFeedback` has recorded on this inquiry, keyed by
    /// the [`FeedbackTarget`] it was given on — the query and the response revision — so
    /// one revision holds at most one record. A `BTreeMap` orders the inquiry-wide entries
    /// first, then by ascending query identifier and revision.
    pub feedback: BTreeMap<FeedbackTarget, InquiryFeedback>,
}

impl ComplianceInquiry {
    /// Apply the transition the CLOCK owes this inquiry, if `now` has reached its due
    /// instant. A no-op otherwise, and a no-op for an inquiry already terminal, so
    /// calling it on every read is idempotent.
    ///
    /// The terminal status says what became of the SUBMISSION: the inquiry is over when
    /// the work its content submitted is, and the inquiry-level message says what became
    /// of it.
    ///
    /// * a query the service's own model reviews ([`REVIEW_TYPE_AI`], the review every
    ///   query of every inquiry this service holds carries) is processed by the due
    ///   instant, so it reaches `COMPLETED`, and an inquiry whose queries all did
    ///   reaches the modelled clean-completion message. From that instant the query
    ///   REPORTS the answer it holds ([`InquiryQuery::reported_answer`]) — the passages the
    ///   retrieval over the published catalog found for it and the citations naming where
    ///   they are published — and a query the catalog holds nothing bearing on reports
    ///   none, which is the honest signal rather than a query-level error that never
    ///   happened. Every reader of an inquiry (`GetComplianceInquiryMetadata`,
    ///   `ListComplianceInquiries`, `ListComplianceInquiryQueries`) and every writer
    ///   against one (`PutComplianceInquiryFeedback`) reports or records in whatever
    ///   status it is in; only `ExportComplianceInquiry` has a precondition on it, because
    ///   a document can be rendered only from an inquiry whose processing is over
    ///   ([`processing_in_progress`](Self::processing_in_progress));
    /// * the modelled FAILED state is for a submission with NOTHING to process
    ///   ([`InquiryContentRecord::nothing_to_process`]) — it never got as far as a unit of
    ///   work. A submission that DID carry something to process is processed, so it reaches
    ///   a non-failure terminal status whatever handle its units of work could be reported
    ///   under: how many queries the inquiry can REPORT ([`reportable_query_text`]) is a
    ///   property of the member that reports them, and reporting fewer than were submitted
    ///   is not a processing failure.
    ///
    /// The modelled `HUMAN_REVIEW` status and `Query is pending human review.` message are
    /// not reported by this emulator: `InquirySupportMode` admits one mode
    /// ([`SUPPORT_MODE_AI_ONLY`]), so nothing a caller can submit asks for a human expert
    /// review, and no status is reported for a review nothing performed.
    ///
    /// No query of an inquiry this emulator holds ever fails, so no inquiry reports the
    /// modelled completed-with-query-errors message: a query is processed by reading the
    /// submitted content, nothing here does that, and no failure is synthesised for work
    /// nothing ran.
    pub fn materialise(&mut self, now: SystemTime) {
        let Some(due) = self.processing_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.processing_due_at = None;
        // The transition was owed AT `due`, so that is when the resource changed —
        // not the instant some caller happened to look.
        self.updated_at = due;
        for query in &mut self.queries {
            if query.status != QUERY_STATUS_PROCESSING {
                continue;
            }
            query.status = QUERY_STATUS_COMPLETED.to_string();
            query.status_message = QUERY_MESSAGE_COMPLETE.to_string();
        }
        if self.content.nothing_to_process() {
            self.status = INQUIRY_STATUS_FAILED.to_string();
            self.status_message = INQUIRY_MESSAGE_INTERNAL_ERROR.to_string();
        } else {
            self.status = INQUIRY_STATUS_COMPLETED.to_string();
            self.status_message = INQUIRY_MESSAGE_COMPLETE.to_string();
        }
    }

    /// Record that a caller's write CHANGED this inquiry at `now`, which is what
    /// [`updated_at`](Self::updated_at) — "Timestamp indicating when the resource was last
    /// modified" — reports.
    ///
    /// The resource is everything its own projection reports
    /// ([`to_detail`](Self::to_detail) and [`reported_tags`](Self::reported_tags)): its
    /// TAG SET as much as its status, because this service persists the tags on this very
    /// record and reports them beside the timestamp. So a write that changes any of them
    /// is a modification of the resource and moves the timestamp to the instant it was
    /// made, and the field never reports an instant before a change a later read of the
    /// resource can observe.
    ///
    /// The transition the CLOCK owed is applied FIRST
    /// ([`materialise`](Self::materialise)): it changed the record at its own due instant,
    /// which is before this write, so applying it afterwards would drag the timestamp back
    /// behind this change.
    ///
    /// Called only where the resource really changed — a call that leaves it as it was
    /// modified nothing, which is what makes a repeat of an `@idempotent` tag write
    /// unobservable.
    pub fn record_modification(&mut self, now: SystemTime) {
        self.materialise(now);
        self.updated_at = now;
    }

    /// Whether this inquiry's processing is STILL UNDERWAY — its status is `InquiryStatus`'s
    /// non-terminal [`INQUIRY_STATUS_PROCESSING`], the status the create answered its 202
    /// with and the one the clock moves it off ([`materialise`](Self::materialise)).
    ///
    /// The precondition `ExportComplianceInquiry` is refused against
    /// ([`unexportable_while_processing`]): the document an export publishes is rendered
    /// from the inquiry as it stands, so there is nothing finished to render while the
    /// inquiry is still being processed. An inquiry whose processing IS over — whatever it
    /// came to — is exported: the status the document reports is read off this same record.
    ///
    /// Asked of the inquiry rather than spelled out at the call site, so the one condition
    /// the refusal is keyed on and the status every reader reports cannot disagree.
    pub fn processing_in_progress(&self) -> bool {
        self.status == INQUIRY_STATUS_PROCESSING
    }

    /// Every query identifier this inquiry OWNS, in the ascending order its queries are
    /// held in.
    ///
    /// What `ExportComplianceInquiry` covers when the request named no `queryIdentifiers`:
    /// the whole set [`owns_query`](Self::owns_query) is the membership test for.
    pub fn query_identifiers(&self) -> Vec<i32> {
        self.queries.iter().map(|query| query.identifier).collect()
    }

    /// Whether `identifier` names a query of THIS inquiry.
    ///
    /// The ownership rule both operations taking a query identifier are refused against —
    /// `ExportComplianceInquiry`'s `queryIdentifiers` and
    /// `PutComplianceInquiryFeedback`'s `queryIdentifier`. One rule, applied in one place,
    /// so the two cannot disagree about which queries an inquiry owns.
    pub fn owns_query(&self, identifier: i32) -> bool {
        self.queries
            .iter()
            .any(|query| query.identifier == identifier)
    }

    /// The feedback this inquiry already holds for `target`, if any.
    ///
    /// What makes a second submission against ONE response revision a conflict rather than
    /// a replacement: the record's presence is the state condition the refusal is keyed on.
    pub fn feedback_for(&self, target: FeedbackTarget) -> Option<&InquiryFeedback> {
        self.feedback.get(&target)
    }

    /// Whether this inquiry can hold another feedback record — [`FEEDBACK_PER_INQUIRY_CAPACITY`].
    pub fn accepts_another_feedback(&self) -> bool {
        self.feedback.len() < FEEDBACK_PER_INQUIRY_CAPACITY
    }

    /// Record `feedback` against its own [`FeedbackTarget`], so the key a record is stored
    /// under is always the one the record itself names.
    ///
    /// Only ever called for a target this inquiry holds no record for
    /// ([`feedback_for`](Self::feedback_for)): one revision is rated once.
    pub fn record_feedback(&mut self, feedback: InquiryFeedback) {
        self.feedback.insert(feedback.target(), feedback);
    }

    /// The name every read of this inquiry reports — `InquirySummary$name`,
    /// `InquiryDetail$name` and the exported document's `name`.
    ///
    /// The content names the inquiry when it derives a name
    /// ([`InquiryContentRecord::derived_name`]), and the caller-supplied title stands
    /// only for content that derives none. One rule, applied in one place, so the
    /// create, the listing, the metadata read and the export cannot disagree about what
    /// an inquiry is called.
    pub fn display_name(&self) -> &str {
        self.content.derived_name().unwrap_or(self.name.as_str())
    }

    /// The `InquirySummary` projection `CreateComplianceInquiry` and
    /// `ListComplianceInquiries` return.
    pub fn to_summary(&self) -> InquirySummary {
        InquirySummary {
            arn: self.arn.clone(),
            name: self.display_name().to_string(),
            id: self.id.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            input_source: self.content.input_source().to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
        }
    }

    /// The `InquiryDetail` projection `GetComplianceInquiryMetadata` returns. Every
    /// member it shares with [`to_summary`](Self::to_summary) takes the same value: both
    /// read the one stored record.
    pub fn to_detail(&self) -> InquiryDetail {
        InquiryDetail {
            arn: self.arn.clone(),
            name: self.display_name().to_string(),
            id: self.id.clone(),
            status: self.status.clone(),
            status_message: self.status_message.clone(),
            input_source: self.content.input_source().to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            support_mode: Some(self.support_mode.clone()),
        }
    }

    /// The inquiry's tags, in the `TagsMap` shape every operation returns them in.
    pub fn tags_map(&self) -> ::std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The `tags` member every operation that reports this resource's tag set answers
    /// with — the create, the metadata read, the export and the tag listing.
    ///
    /// The member is ALWAYS present: a resource holding no tags reports the EMPTY map,
    /// never an absent member, because the member reports the resource's tag set and a
    /// resource with no tags has an empty one — not an unknown one a caller would have to
    /// re-read. One rule, applied in one place, so no two operations can disagree about
    /// what an untagged inquiry reports.
    pub fn reported_tags(&self) -> Option<::std::collections::HashMap<String, String>> {
        Some(self.tags_map())
    }

    /// The DOCUMENT `ExportComplianceInquiry` exports: this inquiry, and the queries
    /// `covered` names, as the bytes a caller downloads from the URL that export
    /// published.
    ///
    /// Every value in it is read off this stored record — the inquiry's metadata and its
    /// queries with their statuses and answers — so the document reports what the emulator
    /// holds and nothing else. In particular a query carries the `response` it reports
    /// ([`InquiryQuery::reported_answer`]), which is null while its processing is not over,
    /// and a citation carries the stored link of the source it names
    /// ([`QueryCitation::source_link`]), so the exported triple is the listed one.
    pub fn export_document(&self, covered: &[i32], include_citations: bool) -> Vec<u8> {
        // Which queries the export covers is asked once per query and once per feedback
        // record, so the set is INDEXED once here rather than scanned for each question.
        let covers: BTreeSet<i32> = covered.iter().copied().collect();
        let queries: Vec<::serde_json::Value> = self
            .queries
            .iter()
            .filter(|query| covers.contains(&query.identifier))
            .map(|query| {
                let answer = query.reported_answer();
                let mut entry = ::serde_json::json!({
                    "queryIdentifier": query.identifier,
                    "query": query.query,
                    "response": answer.map(|answer| answer.response.clone()),
                    "reviewType": query.review_type,
                    "status": query.status,
                    "statusMessage": query.status_message,
                    "createdAt": rendered_timestamp(query.created_at),
                });
                if include_citations {
                    // "When true, include citations in the exported document." A citation
                    // cites the source the query's response was drawn from, so the sources
                    // reported here are the ones the stored answer holds — each rendered
                    // through the ONE citation projection
                    // ([`QueryCitation::to_document_entry`]), so the triple the document
                    // reports is the triple the query listing reports — and a query
                    // reporting no answer yet contributes an EMPTY list rather than
                    // references nothing read.
                    entry["citations"] = ::serde_json::json!(answer
                        .map(|answer| answer
                            .citations
                            .iter()
                            .map(|citation| citation.to_document_entry())
                            .collect::<Vec<::serde_json::Value>>())
                        .unwrap_or_default());
                }
                entry
            })
            .collect();
        let document = ::serde_json::json!({
            "complianceInquiryId": self.id,
            "arn": self.arn,
            "name": self.display_name(),
            "status": self.status,
            "statusMessage": self.status_message,
            "supportMode": self.support_mode,
            "inputSource": self.content.input_source(),
            "createdAt": rendered_timestamp(self.created_at),
            "updatedAt": rendered_timestamp(self.updated_at),
            "includeCitations": include_citations,
            "queryIdentifiers": covered,
            "queries": queries,
            "feedback": self.feedback_entries(&covers),
            "tags": self.tags,
        });
        // Pretty-printed: the document is what a caller downloads and reads, not a wire
        // payload. A value that will not render leaves the document empty rather than
        // half-written, which the download then reports as the server error it is.
        ::serde_json::to_vec_pretty(&document).unwrap_or_default()
    }

    /// The feedback this inquiry holds for the queries `covers` names, as the exported
    /// document reports it — read off the stored [`InquiryFeedback`] records, in the key
    /// order the store holds them in (the inquiry-wide entries first, then by ascending
    /// query identifier and response revision).
    ///
    /// Filtered to `covers` for the same reason the `queries` list is: the document is a
    /// report on the queries the export covers, so feedback on a query it does not cover
    /// is not part of it.
    ///
    /// Like every other key of this document, `feedback` is this emulator's own rendering
    /// rather than a modelled shape — AWS states no format for the exported object — and
    /// it is the only place a caller can read the stored feedback back, because the model
    /// declares no operation that reports it.
    fn feedback_entries(&self, covers: &BTreeSet<i32>) -> Vec<::serde_json::Value> {
        self.feedback
            .values()
            .filter(|entry| {
                entry
                    .query_identifier
                    .is_none_or(|identifier| covers.contains(&identifier))
            })
            .map(|entry| {
                ::serde_json::json!({
                    "queryIdentifier": entry.query_identifier,
                    "rating": entry.rating,
                    "reasonCodes": entry.reason_codes,
                    "comment": entry.comment,
                    "responseRevisionId": entry.response_revision_id,
                    "submittedAt": rendered_timestamp(entry.submitted_at),
                })
            })
            .collect()
    }
}

/// One timestamp as the service renders every `TimestampAttribute` on the wire — RFC 3339
/// date-time, which is the `@timestampFormat` the model declares for them, carrying a
/// fractional-seconds component only when the instant itself has one.
///
/// The ONE renderer, shared with the codec this service's responses are written by
/// ([`service_common::date_time_json`]), so a document this service renders an instant into
/// and the response member reporting the same instant cannot disagree about its form.
pub fn rendered_timestamp(value: SystemTime) -> String {
    ::service_common::date_time_json::date_time_text(&::aws_smithy_types::DateTime::from(value))
        .unwrap_or_default()
}

/// What one remembered `CreateComplianceInquiry` `clientToken` replays: the inquiry the
/// first call with that token created, and the fingerprint of the request that created
/// it, so the same token on a DIFFERENT request is a conflict rather than a replay.
#[derive(Debug, Clone)]
pub struct InquiryCreateRecord {
    pub inquiry_id: String,
    pub fingerprint: String,
}

/// What one remembered `PutComplianceInquiryFeedback` `clientToken` answers: the inquiry
/// the call with that token recorded feedback on, the fingerprint of the request it was
/// recorded for — so a RETRY of that request can be told from the token sent on another one
/// — and the `submittedAt` that call answered, which is what every later request carrying
/// the token is answered with.
///
/// "If this token matches a previous request, the service ignores the request, but does not
/// return an error" (`PutComplianceInquiryFeedbackRequest$clientToken`): ignoring the
/// request means the stored feedback and the answered timestamp both stay the first call's,
/// whichever of the two a later request is — the fingerprint decides only WHERE in the
/// handler it is ignored, never whether it is refused.
#[derive(Debug, Clone)]
pub struct InquiryFeedbackRecord {
    pub inquiry_id: String,
    pub fingerprint: String,
    pub submitted_at: SystemTime,
}

/// One export `ExportComplianceInquiry` produced: the document it minted for an inquiry,
/// and the options it was exported under.
///
/// The record is what makes the presigned URL a locator for something this emulator
/// persists rather than a string assembled per request: the URL carries the export's id,
/// and this is the export that id names.
#[derive(Debug, Clone)]
pub struct InquiryExport {
    /// The minted export id, which the document URL carries.
    pub id: String,
    /// The inquiry this document was exported from.
    pub inquiry_id: String,
    /// The `queryIdentifiers` the export covers, ascending and without repeats: the ones
    /// the request named, or every query the inquiry owns when it named none.
    pub query_identifiers: Vec<i32>,
    /// `ExportComplianceInquiryRequest$includeCitations`, defaulted to false when the
    /// request named no value.
    pub include_citations: bool,
    pub exported_at: SystemTime,
    /// The exported document itself, as [`ComplianceInquiry::export_document`] rendered
    /// it when this export was taken.
    ///
    /// Held rather than re-rendered on download, because an export is a SNAPSHOT: the
    /// document a caller follows the URL to is the one the export produced, whatever has
    /// become of the inquiry since.
    pub document: Vec<u8>,
    /// The token the announced document URL carries, and the one thing that authorises a
    /// fetch of this export's document
    /// ([`ComAmazonawsArtifactArtifactState::export_download_authorised`]).
    ///
    /// It is what the caller this export was handed to holds and nobody else does: the
    /// inquiry and export ids the URL's path names are minted from counters, so they are
    /// derivable by a caller that never called the control plane, while this is not
    /// ([`mint_download_receipt`]). Carried only inside the announced URL — whose
    /// `PresignedUrl` shape is `@sensitive` — and never logged or answered on its own.
    pub download_token: String,
}

/// One export [`ComAmazonawsArtifactArtifactState::record_inquiry_export`] just minted:
/// the id its document URL names, and the token that URL carries to authorise a fetch of
/// the document.
#[derive(Debug, Clone)]
pub struct MintedInquiryExport {
    /// The export's id, the path segment [`export_document_path`] names it by.
    pub id: String,
    /// The export's download token, which [`inquiry_export_url`] carries in the URL.
    pub download_token: String,
}

/// What a request to a published export-document URL names: the inquiry the document was
/// exported from, the export that produced it, and the object being fetched.
///
/// The path is the one [`export_document_path`] builds:
/// `<EXPORT_DOCUMENT_PATH_PREFIX>/<inquiryId>/export/<exportId>/<object>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDocumentTarget {
    pub inquiry_id: String,
    pub export_id: String,
    pub object: String,
}

/// The segments a request path carries under `prefix`, each percent-DECODED, or `None`
/// when the path is not one under that prefix.
///
/// The ONE reader both of this service's document planes resolve a request path through
/// ([`parse_export_document_path`] below and `crate::service`'s report/term parser), so
/// the two cannot publish their documents under two spelling conventions.
///
/// A segment is DECODED before anything is matched against it, because an escaped octet
/// and the character it escapes are the same character: a URL whose segments carry a
/// percent-encoded spelling names the same document as the URL this service announced, so
/// a client, SDK or proxy that re-spells a segment on the way is served the document the
/// path named. The path is split into segments FIRST, so a `%2F` inside one segment stays
/// a character of that segment rather than becoming a separator — an escaped delimiter
/// does not delimit. A decoded segment is only ever compared against a stored record and
/// never joined into a path this emulator opens, so a decoded `..` names no document
/// rather than naming a parent.
///
/// At most one segment MORE than a published path carries is read
/// ([`DOCUMENT_PATH_SEGMENTS`]), which is what tells a caller's path with extra segments
/// from a published one: the extra segment is enough for either parser to refuse the path,
/// and a longer path is refused on its length rather than decoded segment by segment.
pub fn decoded_path_segments(path: &str, prefix: &str) -> Option<Vec<String>> {
    let mut segments = path.split('/').map(smithy_server_runtime::percent_decode);
    for expected in prefix.split('/') {
        if segments.next()? != expected {
            return None;
        }
    }
    let rest: Vec<String> = segments.take(DOCUMENT_PATH_SEGMENTS + 1).collect();
    (!rest.is_empty()).then_some(rest)
}

/// How many segments a document path of this service carries under its prefix: both planes
/// publish four — the two the document's owner is named by, the one that qualifies it (the
/// version, or the fixed [`EXPORT_PATH_SEGMENT`]) and the object.
const DOCUMENT_PATH_SEGMENTS: usize = 4;

/// The fixed segment [`export_document_path`] writes between the inquiry's id and the
/// export's, so the path names the export as a thing OF the inquiry.
const EXPORT_PATH_SEGMENT: &str = "export";

/// The export document a request path names, or `None` when the path is not one this
/// service publishes.
///
/// Read through [`decoded_path_segments`], so every spelling of the announced path names
/// the same export.
pub fn parse_export_document_path(path: &str) -> Option<ExportDocumentTarget> {
    let segments = decoded_path_segments(path, EXPORT_DOCUMENT_PATH_PREFIX)?;
    let [inquiry_id, export_segment, export_id, object] = segments.as_slice() else {
        return None;
    };
    if export_segment != EXPORT_PATH_SEGMENT {
        return None;
    }
    if inquiry_id.is_empty() || export_id.is_empty() || object.is_empty() {
        return None;
    }
    Some(ExportDocumentTarget {
        inquiry_id: inquiry_id.clone(),
        export_id: export_id.clone(),
        object: object.clone(),
    })
}

/// Per-service state for com-amazonaws-artifact-artifact. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsArtifactArtifactState {
    /// The service-owned report catalog, keyed by report id. A `BTreeMap` is what gives
    /// `ListReports` the stable total order its pagination token indexes into.
    pub reports: BTreeMap<String, Report>,
    /// Every term grant `GetTermForReport` has minted, keyed by the grant's own id.
    ///
    /// Keyed by that id and NOT by the receipt the caller was handed, exactly as
    /// [`inquiry_exports`](Self::inquiry_exports) is: the id comes from
    /// [`mint_counter_id`], and a `BTreeMap` rather than a hash map because that rule's
    /// fixed-width hex ids make their lexicographic order their issue order — which is
    /// what lets it hold this map to [`TERM_GRANT_CAPACITY`] without scanning it. A
    /// counter-drawn value is derivable by a caller that was handed nothing, so it keys a
    /// store and authorises nobody; what authorises a download is the receipt on the
    /// record ([`TermGrant::token`]).
    pub term_grants: BTreeMap<String, TermGrant>,
    /// The counter grant ids are minted from, so two acceptances never share an id.
    pub term_grant_seq: u64,
    /// The account's settings: what `PutAccountSettings` writes and
    /// `GetAccountSettings` reports.
    pub account_settings: AccountSettingsRecord,
    /// Every compliance inquiry `CreateComplianceInquiry` has created, keyed by its id.
    ///
    /// A `BTreeMap` because the ids come from [`mint_counter_id`], whose fixed-width hex
    /// ids make their lexicographic order their creation order: that is the stable total
    /// order `ListComplianceInquiries` can index a pagination token into.
    pub inquiries: BTreeMap<String, ComplianceInquiry>,
    /// The counter inquiry ids are minted from, so two creates never share an id.
    pub inquiry_seq: u64,
    /// `CreateComplianceInquiry` `clientToken` -> what it replays. Bounded by
    /// [`INQUIRY_IDEMPOTENCY_CAPACITY`].
    pub inquiry_create_tokens: BTreeMap<String, InquiryCreateRecord>,
    /// The order [`inquiry_create_tokens`](Self::inquiry_create_tokens) were recorded in,
    /// so the oldest is the one forgotten once the map is full. The tokens are
    /// caller-chosen, so their own order says nothing about when they arrived.
    pub inquiry_create_token_order: ::std::collections::VecDeque<String>,
    /// `PutComplianceInquiryFeedback` `clientToken` -> what it replays. Bounded by
    /// [`INQUIRY_IDEMPOTENCY_CAPACITY`], and separate from
    /// [`inquiry_create_tokens`](Self::inquiry_create_tokens) because an idempotency token
    /// identifies one call of ONE operation: the same token on two different operations is
    /// two calls, not a replay.
    pub inquiry_feedback_tokens: BTreeMap<String, InquiryFeedbackRecord>,
    /// The order [`inquiry_feedback_tokens`](Self::inquiry_feedback_tokens) were recorded
    /// in, so the oldest is the one forgotten once the map is full.
    pub inquiry_feedback_token_order: ::std::collections::VecDeque<String>,
    /// The calling account's customer-agreements, keyed by id. A `BTreeMap` is what
    /// gives `ListCustomerAgreements` the stable total order its pagination token
    /// indexes into.
    pub customer_agreements: BTreeMap<String, CustomerAgreement>,
    /// Every document `ExportComplianceInquiry` has exported, keyed by its export id.
    ///
    /// A `BTreeMap` for the same reason as [`term_grants`](Self::term_grants): the ids
    /// come from [`mint_counter_id`], so the first entry is always the oldest export,
    /// which is what lets that one rule hold this map to [`INQUIRY_EXPORT_CAPACITY`]
    /// without scanning it.
    pub inquiry_exports: BTreeMap<String, InquiryExport>,
    /// The counter export ids are minted from, so two exports never share an id.
    pub inquiry_export_seq: u64,
    /// The secret this store's pagination cursors are authenticated under, drawn from the
    /// system's randomness when the store is created ([`PageTokenKey`]) and never
    /// reported: it is what lets a paginated read tell a cursor this service CUT from a
    /// value a caller assembled. Private, and reached through
    /// [`page_token_key`](Self::page_token_key), so nothing can overwrite it with a value
    /// a request carried.
    page_token_key: PageTokenKey,
}

impl Default for ComAmazonawsArtifactArtifactState {
    fn default() -> Self {
        Self {
            reports: seed_catalog(),
            term_grants: BTreeMap::new(),
            term_grant_seq: 0,
            account_settings: AccountSettingsRecord::default(),
            inquiries: BTreeMap::new(),
            inquiry_seq: 0,
            inquiry_create_tokens: BTreeMap::new(),
            inquiry_create_token_order: ::std::collections::VecDeque::new(),
            inquiry_feedback_tokens: BTreeMap::new(),
            inquiry_feedback_token_order: ::std::collections::VecDeque::new(),
            customer_agreements: seed_customer_agreements(),
            inquiry_exports: BTreeMap::new(),
            inquiry_export_seq: 0,
            // One key per store, from the system's randomness: see `PageTokenKey`.
            page_token_key: PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsArtifactArtifactState {
    /// The secret this store's pagination cursors are sealed under, for the paginated
    /// operation about to mint or spend one.
    ///
    /// Handed out by value — [`PageTokenKey`] is `Copy` — so a read can take the key off
    /// the store and go on to borrow the records it is about to report in the same call.
    pub fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }

    /// Whether the inquiry store is below [`INQUIRY_CAPACITY`].
    pub fn accepts_another_inquiry(&self) -> bool {
        self.inquiries.len() < INQUIRY_CAPACITY
    }

    /// The TAGGABLE resource one `resourceArn` names, or `None` when this account holds
    /// no such resource.
    ///
    /// A compliance inquiry is the only resource of this service that can carry tags: it
    /// is the only one a caller creates, and the only one whose modelled shapes have a
    /// `tags` member at all (`CreateComplianceInquiryRequest$tags` — "Tags to associate
    /// with the compliance inquiry resource" — and the `tags` of every inquiry
    /// projection; no report, term or customer-agreement shape declares one). So a
    /// `resourceArn` naming anything else names nothing this store holds.
    ///
    /// The ARN is compared WHOLE against the one [`inquiry_arn`] built for the stored
    /// record rather than parsed for an id, which is what makes the partition, the
    /// service, the region and the account segments all have to be the ones this
    /// emulator issued the resource under: an ARN of another account, another region or
    /// another service names no resource here, whatever id it carries.
    pub fn tagged_resource(&self, resource_arn: &str) -> Option<&ComplianceInquiry> {
        self.inquiries
            .values()
            .find(|inquiry| inquiry.arn == resource_arn)
    }

    /// Mint an unused `InquiryId`.
    ///
    /// The one minting rule ([`mint_counter_id`]) under the `compliance-inquiry` prefix,
    /// whose 16 lower-case hex digits complete into `InquiryId`'s
    /// `^compliance-inquiry-[a-zA-Z0-9]{16}$`.
    ///
    /// Nothing is RELEASED to make room, so the rule is asked for no capacity here: an
    /// inquiry is a resource the caller created and a later read must still find it, which
    /// is why this store is bounded by refusing the create
    /// ([`accepts_another_inquiry`](Self::accepts_another_inquiry)) instead.
    pub fn mint_inquiry_id(&mut self) -> String {
        mint_counter_id(
            &mut self.inquiries,
            &mut self.inquiry_seq,
            INQUIRY_ID_PREFIX,
            None,
        )
    }

    /// Remember that `token` created `inquiry_id` from the request `fingerprint`,
    /// forgetting the oldest remembered token once [`INQUIRY_IDEMPOTENCY_CAPACITY`] are
    /// held — so a caller sending a fresh `clientToken` on every request cannot grow this
    /// map without end.
    pub fn record_inquiry_create(&mut self, token: String, inquiry_id: &str, fingerprint: &str) {
        remember_token(
            &mut self.inquiry_create_tokens,
            &mut self.inquiry_create_token_order,
            token,
            InquiryCreateRecord {
                inquiry_id: inquiry_id.to_string(),
                fingerprint: fingerprint.to_string(),
            },
        );
    }

    /// Remember that `token` recorded the feedback the call carrying it submitted on
    /// `inquiry_id` at `submitted_at`, from the request `fingerprint` — bounded exactly as
    /// [`record_inquiry_create`](Self::record_inquiry_create) is, so a caller sending a
    /// fresh `clientToken` on every request cannot grow this map without end.
    pub fn record_inquiry_feedback_token(
        &mut self,
        token: String,
        inquiry_id: &str,
        fingerprint: &str,
        submitted_at: SystemTime,
    ) {
        remember_token(
            &mut self.inquiry_feedback_tokens,
            &mut self.inquiry_feedback_token_order,
            token,
            InquiryFeedbackRecord {
                inquiry_id: inquiry_id.to_string(),
                fingerprint: fingerprint.to_string(),
                submitted_at,
            },
        );
    }

    /// Mint and persist a term grant for one report version, returning its token.
    ///
    /// The grant covers the TERM document governing `report`, so the token it returns
    /// authorises every version of every report that document governs.
    ///
    /// The grant's ID is minted, and the store held to [`TERM_GRANT_CAPACITY`], by the one
    /// rule ([`mint_counter_id`]): no OUTSTANDING grant ever shares an id with another, and
    /// a caller accepting terms in a loop cannot grow this map without end, because the
    /// oldest grants are released first. The TOKEN it answers with is minted separately, by
    /// the one receipt rule ([`mint_download_receipt`]), because it is what authorises a
    /// download: per acceptance, so a grant is the receipt for one acceptance, and drawn
    /// from no counter, so a caller that accepted nothing cannot derive it.
    pub fn mint_term_grant(
        &mut self,
        report: &Report,
        report_version: i64,
        now: SystemTime,
    ) -> String {
        let id = mint_counter_id(
            &mut self.term_grants,
            &mut self.term_grant_seq,
            TERM_GRANT_ID_PREFIX,
            Some(TERM_GRANT_CAPACITY),
        );
        let token = mint_download_receipt();
        self.term_grants.insert(
            id,
            TermGrant {
                token: token.clone(),
                term_digest: report.term.digest.clone(),
                report_id: report.id.clone(),
                report_version,
                issued_at: now,
            },
        );
        token
    }

    /// Whether `token` is a grant this service issued that covers `report`'s term.
    ///
    /// The one place the term-token rule is decided, so every path that decides a
    /// download ([`Self::download_authorised`]) reads the same answer. A term is
    /// identified by its document's CONTENT digest: the same term text, published for two
    /// reports, is one term, and two different texts are two terms whatever they are
    /// named.
    ///
    /// The grants are searched by the RECEIPT each holds rather than looked up by it,
    /// because the store is keyed by the grant id its capacity rule needs
    /// ([`term_grants`](Self::term_grants)) and a receipt is derivable from no key.
    pub fn grant_covers(&self, token: &str, report: &Report) -> bool {
        self.term_grants
            .values()
            .any(|grant| grant.token == token && grant.term_digest == report.term.digest)
    }

    /// Whether a download of `report` presenting `token` is authorised.
    ///
    /// The one place the download rule is decided, so `GetReport` and the document data
    /// plane cannot disagree about which report downloads under which token: a download is
    /// served only under a grant covering the term governing that report
    /// ([`Self::grant_covers`]), and nothing else is a receipt.
    ///
    /// The report's `AcceptanceType` does not enter into it. `GetReport`'s `termToken` is
    /// `@required` and documented as the "Unique download token provided by
    /// GetTermForReport API", so a value no acceptance of this service ever minted is not a
    /// token on any report; what the acceptance type says is whether the caller has to be
    /// shown the term document before asking for one, which is a statement about the
    /// client's flow and not about which receipts this service issued.
    pub fn download_authorised(&self, token: &str, report: &Report) -> bool {
        self.grant_covers(token, report)
    }

    /// The stored document a data-plane path names, when this service holds it: the
    /// report document of one version, or the term document governing that report.
    ///
    /// A pure read — the data plane serves what [`Default`] seeded and never writes.
    pub fn document(&self, kind: &str, report_id: &str, version: i64) -> Option<&Document> {
        let report = self.reports.get(report_id)?;
        match kind {
            REPORT_DOCUMENT_KIND => report.versions.get(&version).map(|held| &held.document),
            // A term document is the report's, so it is served under any version the
            // report published — and under none it did not.
            TERM_DOCUMENT_KIND => report.versions.get(&version).map(|_| &report.term),
            _ => None,
        }
    }

    /// The report `report_id` names, when the account's catalog publishes it.
    ///
    /// The FIRST stage of the one rule every report-reading operation of this service
    /// resolves its target by: the catalog IS the account's entitlement, so an id it does
    /// not hold names no report this account may read and the answer is the front door's
    /// authorization refusal rather than a modelled not-found — this service never
    /// confirms whether a report the caller cannot reach exists.
    ///
    /// Decided here so the operations that need only this stage and the ones that go on to
    /// resolve a VERSION ([`Self::report_version`]) cannot disagree about the same
    /// `reportId`. The refusal is handed back for the caller to lift into its own error
    /// channel, exactly as [`resume_position`] hands back its own.
    pub fn catalog_report(
        &self,
        report_id: &str,
    ) -> Result<&Report, crate::scaffolded::FrontDoorRefusal> {
        self.reports
            .get(report_id)
            .ok_or_else(crate::scaffolded::FrontDoorRefusal::unauthorized)
    }

    /// The report and the one version of it a request targets: the version `requested`
    /// names, or the newest published one when it names none.
    ///
    /// BOTH stages of the rule, in the order a caller can tell them apart by: an id the
    /// catalog does not hold is the front door's refusal ([`Self::catalog_report`]), while
    /// a report that IS reachable and never published the version asked for — or, when the
    /// request named no version, never published any at all — is the modelled not-found
    /// ([`report_version_not_found`]). Which refusal applies is decided once, here; the
    /// variant the modelled one travels in is the only part that differs per operation,
    /// so it is left to [`ReportRefusal::lift`].
    pub fn report_version(
        &self,
        report_id: &str,
        requested: Option<i64>,
    ) -> Result<(&Report, &ReportVersion), ReportRefusal> {
        let report = self
            .catalog_report(report_id)
            .map_err(ReportRefusal::FrontDoor)?;
        // TOTAL in both shapes, so nothing a request carries may abort the handler.
        let version = report.resolve(requested).ok_or_else(|| {
            ReportRefusal::VersionNotFound(report_version_not_found(report_id, requested))
        })?;
        Ok((report, version))
    }

    /// The compliance inquiry a request TARGETS, brought up to date with the clock, when
    /// the account holds it.
    ///
    /// The one rule every inquiry-targeting operation of this service resolves its target
    /// by, in the two parts a caller can observe:
    ///
    /// 1. the stored inquiry set IS the account's entitlement, so an `InquiryId` it does
    ///    not hold names no inquiry this account may reach and the answer is the front
    ///    door's authorization refusal rather than a modelled not-found — exactly as a
    ///    `reportId` the catalog does not publish is ([`Self::catalog_report`]), because
    ///    this service never confirms whether a resource the caller cannot reach exists;
    /// 2. the inquiry it DOES hold is materialised ([`ComplianceInquiry::materialise`])
    ///    before it is handed back: the transition the clock already owed it was due at
    ///    the stored instant whether or not anybody called, so every operation that
    ///    resolves this target sees the same record.
    ///
    /// Decided here so the operations that only report the inquiry and the ones that go on
    /// to record against it cannot disagree about the same `complianceInquiryId`. The
    /// refusal is handed back for the caller to lift into its own error channel, exactly as
    /// [`Self::catalog_report`] hands back its own — which is what lets an operation that
    /// attributes its calls record the refusal before stating it.
    pub fn targeted_inquiry_mut(
        &mut self,
        inquiry_id: &str,
        now: SystemTime,
    ) -> Result<&mut ComplianceInquiry, crate::scaffolded::FrontDoorRefusal> {
        let inquiry = self
            .inquiries
            .get_mut(inquiry_id)
            .ok_or_else(crate::scaffolded::FrontDoorRefusal::unauthorized)?;
        inquiry.materialise(now);
        Ok(inquiry)
    }

    /// The taggable resource `arn` names, for a WRITE to its tag set, or `None` when this
    /// account holds no such resource. The read-only counterpart, and which ARNs name a
    /// resource at all, is [`tagged_resource`](Self::tagged_resource).
    pub fn tagged_resource_mut(&mut self, resource_arn: &str) -> Option<&mut ComplianceInquiry> {
        self.inquiries
            .values_mut()
            .find(|inquiry| inquiry.arn == resource_arn)
    }

    /// Mint and persist one compliance-inquiry export, returning the id its URL names and
    /// the token that URL carries to authorise a fetch of the document.
    ///
    /// Every call produces a NEW document — that is what exporting is — so the id is
    /// fresh on every call and two exports of one inquiry are two records with two URLs.
    /// Minted, and the map held to [`INQUIRY_EXPORT_CAPACITY`] by releasing the oldest
    /// export first, by the one rule ([`mint_counter_id`]), so a caller exporting in a
    /// loop cannot grow it without end.
    pub fn record_inquiry_export(
        &mut self,
        inquiry_id: &str,
        query_identifiers: Vec<i32>,
        include_citations: bool,
        document: Vec<u8>,
        now: SystemTime,
    ) -> MintedInquiryExport {
        let id = mint_counter_id(
            &mut self.inquiry_exports,
            &mut self.inquiry_export_seq,
            EXPORT_ID_PREFIX,
            Some(INQUIRY_EXPORT_CAPACITY),
        );
        // The receipt this export is handed out under, minted per export: two exports of
        // one inquiry are two documents, so they are authorised by two tokens.
        let download_token = mint_download_receipt();
        self.inquiry_exports.insert(
            id.clone(),
            InquiryExport {
                id: id.clone(),
                inquiry_id: inquiry_id.to_string(),
                query_identifiers,
                include_citations,
                exported_at: now,
                document,
                download_token: download_token.clone(),
            },
        );
        MintedInquiryExport { id, download_token }
    }

    /// Whether a fetch of the export document `target` names, presenting `token`, is
    /// authorised.
    ///
    /// The ONE place the export-download rule is decided, as [`Self::download_authorised`]
    /// is for a report document, so the control plane and the data plane cannot disagree
    /// about which document downloads under which token. A document is authorised only by
    /// the token the URL that announced it carries, and only for the export that URL
    /// names: the export the path names must be one this store holds, taken from the
    /// inquiry the path names, under the token it was announced with. A fetch carrying no
    /// token carries no receipt, so the empty token is what this rule is asked about.
    pub fn export_download_authorised(&self, token: &str, target: &ExportDocumentTarget) -> bool {
        self.inquiry_exports
            .get(&target.export_id)
            .is_some_and(|export| {
                export.inquiry_id == target.inquiry_id && export.download_token == token
            })
    }
}

/// Mint the token one announced document URL carries.
///
/// The ONE place a download receipt of this service is minted, whichever plane announces
/// the URL — the term grant a report download is authorised by and the download token an
/// inquiry export's document is — so the two cannot disagree about what a receipt is.
///
/// A receipt rather than a key into a store, so unlike [`mint_counter_id`] it is NOT drawn
/// from a counter: a counter-minted token is derivable by a caller that was handed no URL,
/// and a URL carrying only derivable values authorises nobody. It is a version-4 UUID's 32
/// hex digits, which satisfy the one query-value encoding [`QUERY_VALUE`] admits unescaped
/// and the `^[a-zA-Z0-9_\-\s]*$` within 1..256 bytes that `GetReport`'s `termToken` member
/// admits.
fn mint_download_receipt() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// Remember what one `clientToken` replays, forgetting the oldest remembered token once
/// [`INQUIRY_IDEMPOTENCY_CAPACITY`] are held.
///
/// `order` is what says which token is the oldest: the tokens are caller-chosen, so their
/// own order says nothing about when they arrived.
fn remember_token<R>(
    tokens: &mut BTreeMap<String, R>,
    order: &mut ::std::collections::VecDeque<String>,
    token: String,
    record: R,
) {
    if tokens.insert(token.clone(), record).is_none() {
        order.push_back(token);
    }
    while order.len() > INQUIRY_IDEMPOTENCY_CAPACITY {
        match order.pop_front() {
            Some(oldest) => {
                tokens.remove(&oldest);
            }
            None => break,
        }
    }
}

/// The prefix a term grant's ID carries — the id that KEYS the grant store, never the
/// receipt the acceptance answers with ([`mint_download_receipt`]). The model constrains it
/// nowhere, since nothing announces it, so it is spelled like the modelled id attributes.
const TERM_GRANT_ID_PREFIX: &str = "term";

/// The prefix a compliance inquiry's id carries — the literal part of `InquiryId`'s
/// `^compliance-inquiry-[a-zA-Z0-9]{16}$`, which [`is_inquiry_id`] judges a value against.
const INQUIRY_ID_PREFIX: &str = "compliance-inquiry";

/// The prefix an inquiry export's id carries. The model constrains it nowhere — an export
/// id is this emulator's own, named only inside the document URL the export announces
/// ([`export_document_path`]) — so it is spelled like the modelled id attributes.
const EXPORT_ID_PREFIX: &str = "export";

/// Mint an id no entry of `store` holds, after holding `store` to `capacity` by releasing
/// its oldest entries.
///
/// The ONE place this rule lives, for every store this service keys by an id it mints —
/// the term grants, the compliance inquiries and the inquiry exports — as [`walk_page`] is
/// the one place the page walk lives: a correction to the eviction order, to the collision
/// re-roll or to the id width is then made once rather than once per store.
///
/// `seq` advances on every call and the loop skips an id the store already holds, so no
/// two entries a store holds at one time ever share an id. An id is `prefix` followed by
/// 16 lower-case hex digits, which satisfies the `<prefix>-[a-zA-Z0-9]{16}` shape this
/// model's id attributes declare; short of the counter's wrap, which 2^64 calls away is
/// unreachable, the ids rise with the counter, so their lexicographic order IS their issue
/// order — that is what makes the `BTreeMap`'s FIRST entry its oldest and lets `capacity`
/// be held with `pop_first`, without scanning the store.
///
/// `capacity` is `Some(bound)` for a store whose oldest entry may be released to make
/// room, and `None` for a store bounded another way: one whose records a later call must
/// still find refuses the write when it is full instead (see
/// [`ComAmazonawsArtifactArtifactState::mint_inquiry_id`]), so nothing may be released
/// from it here.
pub fn mint_counter_id<Record>(
    store: &mut BTreeMap<String, Record>,
    seq: &mut u64,
    prefix: &str,
    capacity: Option<usize>,
) -> String {
    if let Some(bound) = capacity {
        while store.len() >= bound {
            if store.pop_first().is_none() {
                // A store holding nothing can release nothing: unreachable for a positive
                // bound, and what keeps a zero one from looping for ever.
                break;
            }
        }
    }
    loop {
        *seq = seq.wrapping_add(1);
        let candidate = format!("{prefix}-{:016x}", *seq);
        if !store.contains_key(&candidate) {
            return candidate;
        }
    }
}

/// The ARN of one VERSION of a report resource.
///
/// AWS Artifact reports are service-owned, so the ARN carries this service's own Region
/// ([`service_region`]) and an EMPTY account id. The version is part of the ARN
/// (`arn:aws:artifact:<region>::report/<reportId>:<version>`), because the resource an
/// Artifact report ARN names is one published version of a report: `ListReportVersions`
/// reports one entry per version, and two of them must not name the same resource.
///
/// The version-bearing form is what the recorded `ListReports` and `ListReportVersions`
/// bodies quoted in
/// `issues/things-that-now-work/20261022T003730Z-artifact-report-catalog-and-report-record-projection-diverge.md`
/// show (`arn:aws:artifact:us-east-1::report/report-7g5j2DTVeNXWx7St:1`). It supersedes
/// the version-less form the model's `ListReports` example sketches, which cannot name
/// one version of a report at all.
pub fn report_arn(report_id: &str, version: i64) -> String {
    format!(
        "arn:aws:artifact:{region}::report/{report_id}:{version}",
        region = service_region()
    )
}

/// The ARN of a TERM resource, in the same service-owned form as [`report_arn`]
/// (`arn:aws:artifact:<region>::term/<termId>`).
///
/// `ReportDetail$termArn` is "Unique resource ARN for term resource": an IDENTIFIER of the
/// term, so it is a function of the term alone. A term in this service IS its document
/// ([`Document::digest`] is that document's identity), and that is the identity a download
/// grant covers ([`ComAmazonawsArtifactArtifactState::grant_covers`]): one term document is
/// one term resource — under every version of every report it governs — and two different
/// term texts are two term resources, whatever file names they are published under. So this
/// ARN never carries the report version a projection was read through: a report holds one
/// term ([`Report::term`]) for all of its versions, and naming one term per version would
/// tell a client to accept as many terms as the report has versions while a single
/// acceptance authorises every one of them.
pub fn term_arn(term: &Document) -> String {
    format!(
        "arn:aws:artifact:{region}::term/{}",
        term_id(term),
        region = service_region()
    )
}

/// How many hex digits of a term document's digest the term's resource id carries — enough
/// for the id to have the 16-character shape this service's other resource ids have
/// (`ReportId` is `^report-[a-zA-Z0-9]{16}$`).
const TERM_ID_DIGITS: usize = 16;

/// The resource id of the term `term` is: `term-` followed by [`TERM_ID_DIGITS`] of the
/// document's own identity digest.
///
/// Derived from the document's CONTENT, so the id is stable across reads, equal for the
/// same term text and different for different term text — which is what makes
/// [`term_arn`] identify the term rather than locate it.
fn term_id(term: &Document) -> String {
    let digits = term.digest.get(..TERM_ID_DIGITS).unwrap_or(&term.digest);
    format!("term-{digits}")
}

/// The ARN of a customer-agreement resource.
///
/// A customer-agreement is ACCOUNT-owned — it is one account's acceptance of an
/// agreement, which is what its `awsAccountId` reports — so unlike [`report_arn`] its ARN
/// carries the account id beside this service's own Region.
pub fn customer_agreement_arn(id: &str) -> String {
    format!(
        "arn:aws:artifact:{region}:{account}:customer-agreement/{id}",
        region = service_region(),
        account = ::engine::emulator_account_id()
    )
}

/// The ARN of the AWS-published agreement resource a customer-agreement represents.
///
/// The agreement itself is published by AWS rather than owned by an account, and it is
/// not regional, so both fields are empty — the same service-owned ARN form [`report_arn`]
/// uses for a report, which carries no account id.
pub fn agreement_arn(agreement_id: &str) -> String {
    format!("arn:aws:artifact:::agreement/{agreement_id}")
}

/// The path a stored document is published at: its kind, the report version it belongs
/// to, and the document's own file name. Built from the STORED record, never from the
/// request, so the URL names where the document is rather than what was asked for.
pub fn document_path(kind: &str, report_id: &str, version: i64, object: &str) -> String {
    format!("{DOCUMENT_PATH_PREFIX}/{kind}/{report_id}/v{version}/{object}")
}

/// The URL a caller downloads a stored document from.
///
/// `host` is the endpoint the CALLER reached this emulator on (see [`document_url_host`])
/// and the path is the document's own, so the announced URL resolves: a GET of it comes
/// back to this service's document data plane, which serves the stored bytes. It names
/// the caller's own endpoint rather than a host of this service's choosing — this
/// emulator holds the document, so a client or test that follows the URL must come back
/// here.
///
/// The token the grant was issued under is carried as a query parameter
/// ([`tokened_document_url`]): it is the receipt that authorises the download, and it is
/// what makes two acceptances of the same report yield two distinct URLs.
///
/// NO SigV4 signature is attached — this emulator does not sign documents, and a
/// fabricated signature would claim a computation nothing performed.
fn document_url(
    host: &str,
    kind: &str,
    report_id: &str,
    version: i64,
    object: &str,
    term_token: &str,
) -> String {
    tokened_document_url(
        host,
        &document_path(kind, report_id, version, object),
        TERM_TOKEN_PARAM,
        term_token,
    )
}

/// The query parameter a report or term document URL carries its term grant's token in,
/// which `crate::service`'s document plane reads the fetch's authorisation off.
pub const TERM_TOKEN_PARAM: &str = "termToken";

/// The query parameter an export document URL carries its download token in, which
/// `crate::service`'s export plane reads the fetch's authorisation off.
pub const INQUIRY_EXPORT_TOKEN_PARAM: &str = "exportToken";

/// One document URL this service announces: the endpoint the caller reached
/// ([`document_url_host`]), the path the document is published at, and the token that
/// authorises a fetch of it.
///
/// The ONE place a document URL of this service is assembled, so both planes announce
/// their documents under one convention. The token is PERCENT-ENCODED on the way in, so
/// the announced URL is well formed for every token either plane mints rather than only
/// for the ones whose characters happen to need no escaping; the data plane reads it back
/// through the framework's query decoder, which undoes exactly this.
fn tokened_document_url(host: &str, path: &str, parameter: &str, token: &str) -> String {
    let token = ::percent_encoding::utf8_percent_encode(token, QUERY_VALUE);
    format!("https://{host}{path}?{parameter}={token}")
}

/// What a query-string VALUE must escape: everything but RFC 3986's unreserved set
/// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`), so a value carrying a space, an `&` or a
/// `=` cannot be read as the end of the parameter.
const QUERY_VALUE: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The endpoint the request reached this emulator on, as every document URL this service
/// announces names it: the request's own `Host`, else the authority its URI carries, else
/// [`LOOPBACK_HOST`].
///
/// Read from the request rather than built from the model's endpoint template, because
/// the URL has to resolve for whoever was handed it — a client addressing the emulator by
/// socket is handed that socket.
///
/// The value is a value the CALLER chose, and it is interpolated into a URL this service
/// announces, so only a BARE AUTHORITY is honoured: anything else is not an endpoint, and
/// the loopback literal is announced instead of reflecting it.
///
/// Each source is tested on its own, in that order, and the FIRST one carrying a bare
/// authority is what the URL names: a `Host` this service cannot honour leaves the
/// authority the URI carries still to be read, rather than standing in for it.
pub fn document_url_host(parts: &::http::request::Parts) -> String {
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        // The URI's AUTHORITY, not its host: a request that carries its endpoint on the
        // URI (HTTP/2, or an SDK-built absolute form) carries the PORT there too, and a
        // URL announced without the port would not resolve back to the socket the caller
        // reached.
        parts.uri.authority().map(::http::uri::Authority::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| is_bare_authority(candidate))
    .unwrap_or(LOOPBACK_HOST)
    .to_string()
}

/// True when `value` is a bare authority: a host with an optional port and nothing else.
///
/// A path, query, fragment, userinfo or space makes it something other than an endpoint —
/// a URL built around it would not resolve back to this emulator — so the parse is
/// `::http::uri::Authority`'s, with those separators refused first because an authority
/// accepts userinfo.
fn is_bare_authority(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '?', '#', '@', '\\', ' ', '\t'])
        && value.parse::<::http::uri::Authority>().is_ok()
}

/// The URL of the report document for one version, under the grant `term_token`.
pub fn report_document_url(
    host: &str,
    report: &Report,
    version: &ReportVersion,
    term_token: &str,
) -> String {
    document_url(
        host,
        REPORT_DOCUMENT_KIND,
        &report.id,
        version.version,
        &version.document.object,
        term_token,
    )
}

/// The path the document one compliance-inquiry export produced is published at: the
/// inquiry it was exported from, the export that produced it, and the document's own file
/// name. Built from the STORED export, never from the request, so the path names where the
/// document is rather than what was asked for.
pub fn export_document_path(inquiry_id: &str, export_id: &str) -> String {
    format!(
        "{EXPORT_DOCUMENT_PATH_PREFIX}/{inquiry_id}/{EXPORT_PATH_SEGMENT}/{export_id}/\
         {INQUIRY_EXPORT_OBJECT}"
    )
}

/// The URL of the document one compliance-inquiry export produced.
///
/// `host` is the endpoint the CALLER reached this emulator on (see [`document_url_host`]),
/// exactly as [`report_document_url`] and [`term_document_url`] name it: this emulator
/// holds the export's document, so the URL it announces has to resolve back here for
/// whoever was handed it rather than naming a host this emulator does not serve.
///
/// The path is built from the stored export — the inquiry it was exported from and the
/// export's own id — so each export of an inquiry has its own URL and the URL names the
/// record it can be traced back to. As with [`report_document_url`], NO SigV4 signature is
/// attached: this emulator does not sign documents, and a fabricated signature would claim
/// a computation nothing performed.
///
/// What authorises the fetch instead is `download_token`, the receipt this export was
/// minted with ([`InquiryExport::download_token`]), carried in the query string exactly as
/// [`report_document_url`] carries its term grant's: the URL is handed to ONE caller, and
/// the token is what that caller holds and a caller that was handed no URL does not.
pub fn inquiry_export_url(
    host: &str,
    inquiry_id: &str,
    export_id: &str,
    download_token: &str,
) -> String {
    tokened_document_url(
        host,
        &export_document_path(inquiry_id, export_id),
        INQUIRY_EXPORT_TOKEN_PARAM,
        download_token,
    )
}

/// The URL of the term document governing `report`, under the grant `term_token`.
pub fn term_document_url(host: &str, report: &Report, version: i64, term_token: &str) -> String {
    document_url(
        host,
        TERM_DOCUMENT_KIND,
        &report.id,
        version,
        &report.term.object,
        term_token,
    )
}

/// The Region this request was signed for — what an audit record attributes the call to.
///
/// This is the Region the CALLER presented in its credential scope, and it is reported as
/// such and nothing else: no ARN and no URL is built from it, because this service is
/// served in one Region whatever a caller signed for ([`service_region`]).
///
/// The scope's region is only honoured when it has an AWS region's SHAPE
/// (`<partition>-<area>[-<area>]-<number>`, e.g. `us-east-1`, `ap-southeast-3`), so a
/// record never carries arbitrary header text as a Region. Anything else falls back to
/// [`SERVICE_REGION`], the Region the call was in fact served in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| SERVICE_REGION.to_string())
}

/// True when `value` has an AWS region's shape: lowercase alphabetic segments joined by
/// `-`, ending in a digit group, at least three segments.
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// True when `value` satisfies `ReportId`'s `^report-[a-zA-Z0-9]{16}$`.
///
/// Two callers need it where the boundary validator cannot be the judge: a pagination
/// cursor, which carries a report id and which the model does not constrain, and the
/// front door in [`crate::service`], which decides whether a request names a resource
/// at all BEFORE that validator runs.
pub fn is_report_id(value: &str) -> bool {
    is_id_shaped(value, "report-")
}

/// True when `value` satisfies `CustomerAgreementIdAttribute`'s
/// `^customer-agreement-[a-zA-Z0-9]{16}$`.
///
/// As with [`is_report_id`], this is for a value the model does not constrain: the id a
/// decoded `ListCustomerAgreements` pagination cursor resumes at.
pub fn is_customer_agreement_id(value: &str) -> bool {
    is_id_shaped(value, "customer-agreement-")
}

/// True when `value` satisfies `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$` —
/// the id of a compliance inquiry, as every `complianceInquiryId` member targets one.
///
/// Spelled the same way as [`is_report_id`], and for the same caller: the front door
/// judges the target of a request before the boundary validator sees the member.
pub fn is_inquiry_id(value: &str) -> bool {
    is_id_shaped(value, "compliance-inquiry-")
}

/// True when `value` is `prefix` followed by 16 alphanumerics — the shape every
/// `<resource>-[a-zA-Z0-9]{16}` id attribute in this model declares.
fn is_id_shaped(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

// ---------------------------------------------------------------------------
// Pagination tokens.
//
// A `nextToken` is an OPAQUE continuation value: the only thing a client may do with one
// is hand it back. So a token this service mints carries NOTHING a caller could read a
// record out of — not the id of the record the next page resumes at, and not the id of the
// resource whose sub-collection was paged:
//
//   * WHERE the page resumes is carried as the POSITION in the operation's own reported
//     order (an index), never as the key of a record the same response published;
//   * WHICH resource a scoped page belongs to is carried as the DIGEST of that resource's
//     id, so the cursor names exactly one resource without putting the id on the wire;
//   * the whole payload is SEALED with an AUTHENTICATOR over it, computed under the secret
//     this store drew from the system's randomness and never reveals
//     ([`PageTokenKey`]), so a value a client assembled — including one whose fields are
//     spelled exactly as this codec spells them — decodes to no cursor at all and is
//     refused with the modelled `ValidationException`, rather than being honoured as a
//     resume point this service never cut. The seal covers the scope field too, which is
//     what makes the binding above hold: a cursor cut for one report cannot be re-pointed
//     at another, because a caller cannot seal the payload it would have to write to do
//     so. A seal taken over nothing but public values would authenticate nothing, since
//     every input to it is one the caller holds.
//   * and a cursor is honoured only in the SPELLING this codec writes: the position it
//     carries is decimal with no sign and no leading zero, the one rendering
//     [`encode_cursor`]'s callers give it, so no other spelling of a number is a cursor of
//     this service even though `str::parse` would read it.
//
// This holds for EVERY paginated operation of the service, not only the report listings:
// `ListReports`, `ListReportVersions`, `ListCustomerAgreements`, `ListComplianceInquiries`
// and `ListComplianceInquiryQueries` all cut their page with a cursor from this codec, so
// none of them can publish — or hand back a token anyone can decode into — one of its own
// records' ids.
//
// The encoding is lowercase HEX: every character it emits is unreserved in a URI, so the
// token survives `@httpQuery` round-tripping unescaped, it carries no punctuation a client
// could mangle, and its output stays far inside `NextTokenAttribute`'s `@length(1..2048)`.
// ---------------------------------------------------------------------------

/// The service every cursor this codec mints belongs to. Part of the encoded payload, so
/// a value that is not a full cursor of THIS service never decodes to one.
const CURSOR_SERVICE: &str = "artifact";

/// The cursor kind a `ListReports` token carries.
const REPORTS_CURSOR: &str = "reports";

/// The cursor kind a `ListReportVersions` token carries.
const REPORT_VERSIONS_CURSOR: &str = "reportVersions";

/// The cursor kind a `ListCustomerAgreements` token carries.
const CUSTOMER_AGREEMENTS_CURSOR: &str = "customerAgreements";

/// The cursor kind a `ListComplianceInquiries` token carries.
const INQUIRIES_CURSOR: &str = "complianceInquiries";

/// The cursor kind a `ListComplianceInquiryQueries` token carries.
const INQUIRY_QUERIES_CURSOR: &str = "inquiryQueries";

/// The separator between a cursor's fields. Any byte works — the payload is encoded, so
/// the separator never reaches the wire — and `|` appears in no cursor field: not in a
/// kind, not in a hex digest and not in a decimal position.
const CURSOR_SEPARATOR: char = '|';

/// The scope field of a cursor cut for a WHOLE collection rather than for the
/// sub-collection of one resource. Not a digest, so it can never be mistaken for one.
const CURSOR_UNSCOPED: &str = "-";

/// Encode a cursor: the service and operation that cut the page, the resource the page was
/// cut for (as a digest, or [`CURSOR_UNSCOPED`]) and the position it resumes at, sealed
/// under `key` — the secret the issuing store drew from the system's randomness — which is
/// what lets [`decode_cursor`] tell a cursor this service minted from a value a client
/// assembled.
fn encode_cursor(
    key: &PageTokenKey,
    kind: &str,
    scope: Option<&str>,
    resume_at: &str,
) -> String {
    let scope = scope.map_or_else(|| CURSOR_UNSCOPED.to_string(), cursor_digest);
    let payload = format!(
        "{CURSOR_SERVICE}{CURSOR_SEPARATOR}{kind}{CURSOR_SEPARATOR}{scope}\
         {CURSOR_SEPARATOR}{resume_at}"
    );
    ::hex::encode(key.mint(&payload))
}

/// The digest the scope field of a SCOPED cursor names a resource by: it is the resource
/// the cursor binds to, spelled so the id itself never reaches the wire. What makes that
/// binding unforgeable is the seal the whole payload travels under, not this digest.
fn cursor_digest(resource_id: &str) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(CURSOR_DIGEST_DOMAIN.as_bytes());
    hasher.update(resource_id.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// The domain string every cursor digest is taken over, so a digest from this codec can
/// never collide with one this service computes for anything else.
const CURSOR_DIGEST_DOMAIN: &str = "com.amazonaws.artifact#pagination-cursor\n";

/// The position a token resumes at, when it decodes to a cursor of `kind` cut for `scope`
/// under `key`. `None` for anything else — a value this service never minted, one a client
/// assembled, one another store minted, a cursor another of this service's paginated
/// operations cut, or one cut for another resource.
fn decode_cursor(
    key: &PageTokenKey,
    kind: &str,
    scope: Option<&str>,
    token: &str,
) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    // The seal first: a payload this store did not authenticate names no cursor, so no
    // field of it is read at all.
    let payload = key.cursor_of(&decoded)?;
    let mut fields = payload.split(CURSOR_SEPARATOR);
    let service = fields.next()?;
    let found_kind = fields.next()?;
    let found_scope = fields.next()?;
    let resume_at = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let expected_scope = scope.map_or_else(|| CURSOR_UNSCOPED.to_string(), cursor_digest);
    (service == CURSOR_SERVICE && found_kind == kind && found_scope == expected_scope)
        .then(|| resume_at.to_string())
}

/// The position a cursor of `kind` cut for `scope` resumes its collection at, when
/// `token` is one this store minted for it.
///
/// The position has to be spelled the way this codec writes it — `usize::to_string`, so
/// decimal with no sign and no leading zero — because a cursor is honoured only as the
/// value it was cut as. `str::parse` reads `+0005` as five, so the round-trip check is
/// what rejects every spelling the minting side never writes.
fn decode_position(
    key: &PageTokenKey,
    kind: &str,
    scope: Option<&str>,
    token: &str,
) -> Option<usize> {
    let resume_at = decode_cursor(key, kind, scope, token)?;
    let position: usize = resume_at.parse().ok()?;
    (position.to_string() == resume_at).then_some(position)
}

/// The `ListReports` pagination token that resumes the catalog at `position` — the number
/// of catalog entries this operation has already reported — sealed under the issuing
/// store's `key` ([`ComAmazonawsArtifactArtifactState::page_token_key`]).
pub fn reports_page_token(key: &PageTokenKey, position: usize) -> String {
    encode_cursor(key, REPORTS_CURSOR, None, &position.to_string())
}

/// The catalog position a `ListReports` token resumes at, when it is a token THIS operation
/// issued from THIS store's key. `None` for anything else.
pub fn parse_reports_page_token(key: &PageTokenKey, token: &str) -> Option<usize> {
    decode_position(key, REPORTS_CURSOR, None, token)
}

/// The `ListReportVersions` pagination token that resumes `report_id`'s versions at
/// `position` — the number of that report's versions this operation has already reported.
///
/// SCOPED to the report it was cut for: the payload carries the digest of the report id, so
/// a token cut for one report is refused on another, while the id itself never reaches the
/// wire.
pub fn report_versions_token(key: &PageTokenKey, report_id: &str, position: usize) -> String {
    encode_cursor(
        key,
        REPORT_VERSIONS_CURSOR,
        Some(report_id),
        &position.to_string(),
    )
}

/// The position in `report_id`'s versions a `ListReportVersions` token resumes at, when it
/// is a token THIS operation issued for that report from THIS store's key. `None` for
/// anything else.
pub fn parse_report_versions_token(
    key: &PageTokenKey,
    token: &str,
    report_id: &str,
) -> Option<usize> {
    decode_position(key, REPORT_VERSIONS_CURSOR, Some(report_id), token)
}

/// The `ListComplianceInquiryQueries` pagination token that resumes `inquiry_id`'s queries
/// at `position`.
///
/// Scoped to the inquiry it was cut for, for the same reason as
/// [`report_versions_token`]: a token issued for one inquiry must not be spendable as a
/// resume point on another.
pub fn inquiry_queries_token(key: &PageTokenKey, inquiry_id: &str, position: usize) -> String {
    encode_cursor(
        key,
        INQUIRY_QUERIES_CURSOR,
        Some(inquiry_id),
        &position.to_string(),
    )
}

/// The position in `inquiry_id`'s queries a `ListComplianceInquiryQueries` token resumes
/// at, when it is a token THIS operation issued for that inquiry from THIS store's key.
/// `None` for anything else.
pub fn parse_inquiry_queries_token(
    key: &PageTokenKey,
    token: &str,
    inquiry_id: &str,
) -> Option<usize> {
    decode_position(key, INQUIRY_QUERIES_CURSOR, Some(inquiry_id), token)
}

/// The `ListCustomerAgreements` pagination token that resumes the account's agreements at
/// `position` — the number of agreements this operation has already reported.
///
/// A position rather than an agreement id, for the same reason as [`reports_page_token`]:
/// a `nextToken` is a continuation value, not a resource identifier, so no id this
/// operation reports reaches the wire inside its token.
pub fn customer_agreements_page_token(key: &PageTokenKey, position: usize) -> String {
    encode_cursor(key, CUSTOMER_AGREEMENTS_CURSOR, None, &position.to_string())
}

/// The position a `ListCustomerAgreements` token resumes at, when it is a token THIS
/// operation issued from THIS store's key. `None` for anything else.
pub fn parse_customer_agreements_page_token(key: &PageTokenKey, token: &str) -> Option<usize> {
    decode_position(key, CUSTOMER_AGREEMENTS_CURSOR, None, token)
}

/// The `ListComplianceInquiries` pagination token that resumes the account's inquiries at
/// `position`. A position rather than an inquiry id, for the same reason as
/// [`reports_page_token`].
pub fn inquiries_page_token(key: &PageTokenKey, position: usize) -> String {
    encode_cursor(key, INQUIRIES_CURSOR, None, &position.to_string())
}

/// The position a `ListComplianceInquiries` token resumes at, when it is a token THIS
/// operation issued from THIS store's key. `None` for anything else.
pub fn parse_inquiries_page_token(key: &PageTokenKey, token: &str) -> Option<usize> {
    decode_position(key, INQUIRIES_CURSOR, None, token)
}

/// The `ResourceNotFoundException` payload for a version the named report never had.
///
/// A report the catalog does not hold is NOT this error: the front door refuses that
/// call before the catalog is consulted (see [`crate::scaffolded::Scaffolded`]). This
/// error is for a report that IS visible and simply never published the version asked
/// for — or, when the request named no version, never published any at all.
pub fn report_version_not_found(report_id: &str, version: Option<i64>) -> ResourceNotFoundException {
    let message = match version {
        Some(version) => format!("Report {report_id} has no version {version}"),
        None => format!("Report {report_id} has no published version"),
    };
    ResourceNotFoundException {
        message,
        resource_id: report_id.to_string(),
        resource_type: REPORT_RESOURCE_TYPE.to_string(),
    }
}

/// Which of the two refusals a request naming a report version is answered with — the
/// outcome [`ComAmazonawsArtifactArtifactState::report_version`] hands back when it
/// resolves nothing.
///
/// The refusal travels as data so each operation can lift it into its OWN error channel,
/// the idiom [`resume_position`] already uses: which modelled variant the not-found is
/// reported in is the only part of this decision that differs between the report-reading
/// operations.
#[derive(Debug, Clone)]
pub enum ReportRefusal {
    /// The account's catalog holds no such report, so the front door's authorization
    /// refusal is what the caller sees.
    FrontDoor(crate::scaffolded::FrontDoorRefusal),
    /// The report is reachable and never published the version asked for.
    VersionNotFound(ResourceNotFoundException),
}

impl ReportRefusal {
    /// This refusal in one operation's error channel: `version_not_found` names that
    /// operation's own modelled `ResourceNotFoundException` variant, and the front door's
    /// refusal travels in the channel's own variant because its shape is not modelled.
    pub fn lift<Modeled>(
        self,
        version_not_found: impl FnOnce(ResourceNotFoundException) -> Modeled,
    ) -> crate::scaffolded::Scaffolded<Modeled> {
        match self {
            Self::FrontDoor(refusal) => crate::scaffolded::Scaffolded::FrontDoor(refusal),
            Self::VersionNotFound(missing) => {
                crate::scaffolded::Scaffolded::Modeled(version_not_found(missing))
            }
        }
    }
}

/// The ARN of a compliance inquiry.
///
/// Unlike a report, an inquiry is CALLER-owned, so its ARN carries the account id this
/// emulator presents, through `engine::emulator_arn`. Its Region is this service's own
/// ([`service_region`]), like every other ARN minted here: a caller cannot reach this
/// service in another one.
pub fn inquiry_arn(inquiry_id: &str) -> String {
    ::engine::emulator_arn(
        "artifact",
        service_region(),
        &format!("compliance-inquiry/{inquiry_id}"),
    )
}

// There is deliberately no not-found payload for a compliance inquiry id: an id the
// account holds no inquiry for is answered with the front door's 401
// `UnauthorizedException` (`crate::scaffolded::Scaffolded::unauthorized`) rather than the
// modelled 404, exactly as a `reportId` the catalog does not publish is — the gateway
// authorizes the target before the state is consulted, so the absence of a resource the
// caller may not read is never confirmed. That judgement is made in ONE place
// ([`ComAmazonawsArtifactArtifactState::targeted_inquiry_mut`]), so no operation targeting
// an inquiry can answer it differently.

/// The modelled `ConflictException` for a `clientToken` replayed on a request that is
/// not the one it created an inquiry for.
///
/// An `@idempotencyToken` identifies ONE call: honouring it on different content would
/// either hide the second submission or silently replace the first, so the request is
/// refused instead.
pub fn inquiry_token_mismatch(inquiry_id: &str) -> ConflictException {
    ConflictException {
        message: "the clientToken was already used to create a compliance inquiry from \
                  different content"
            .to_string(),
        resource_id: inquiry_id.to_string(),
        resource_type: INQUIRY_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled `ConflictException` for a feedback submission against a response revision
/// that already carries feedback.
///
/// The STATE condition is the rule: a response revision of a compliance inquiry query
/// accepts exactly one submitted feedback, so a second submission against one is refused
/// whatever `clientToken` it carries — that is the guard
/// `PutComplianceInquiryFeedbackRequest$responseRevisionId` advertises ("use this value to
/// prevent submitting feedback on a stale response"), and it is what lets a client tell
/// "this revision has been rated" (terminal) from a token collision (retry with a new
/// token).
///
/// The message is the service's own fixed text for this condition, as AWS states it; the
/// requirement is the condition, not the wording. `resourceId` and `resourceType` are the
/// `@required` members of the modelled error, naming the inquiry the conflicting record
/// lives on, exactly as every other conflict of this service names its resource.
pub fn feedback_already_submitted(inquiry_id: &str) -> ConflictException {
    ConflictException {
        message: FEEDBACK_ALREADY_SUBMITTED_MESSAGE.to_string(),
        resource_id: inquiry_id.to_string(),
        resource_type: INQUIRY_RESOURCE_TYPE.to_string(),
    }
}

/// The message [`feedback_already_submitted`] states, as AWS states it for this condition.
pub const FEEDBACK_ALREADY_SUBMITTED_MESSAGE: &str =
    "Feedback already submitted for this response version.";

/// The modelled `ValidationException` for a QUESTION-LEVEL feedback submission that names
/// no response revision — feedback on one query of an inquiry, with nothing saying which
/// version of that query's response it is feedback on.
///
/// The requirement is the condition: `responseRevisionId` is what identifies the response
/// version a rating is about ("use this value to prevent submitting feedback on a stale
/// response"), so a submission scoped to a single query has to carry it, while one on the
/// inquiry's response as a whole names no query and no revision of one.
///
/// The message is the service's own fixed sentence for this condition
/// ([`QUESTION_LEVEL_REVISION_REQUIRED_MESSAGE`]) rather than the field-clause envelope
/// [`field_validation_failed`] renders, so there is no clause for `fieldList` to name a
/// member with; `reason` is `ValidationExceptionReason`'s `fieldValidationFailed`, because
/// what the request failed on is a member of its own input.
pub fn question_level_revision_required() -> ValidationException {
    ValidationException {
        message: QUESTION_LEVEL_REVISION_REQUIRED_MESSAGE.to_string(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: None,
    }
}

/// The message [`question_level_revision_required`] states, as AWS states it for this
/// condition: one fixed sentence that interpolates nothing from the request.
pub const QUESTION_LEVEL_REVISION_REQUIRED_MESSAGE: &str =
    "expectedResponseRevisionId is required for question-level feedback submission.";

/// The modelled `ValidationException` for a token this service never issued, carrying
/// `ValidationExceptionReason`'s `invalidToken`.
///
/// The message is the service's own fixed refusal text ([`INVALID_TOKEN_MESSAGE`]), not a
/// sentence describing which check failed: which token member was refused, and why the
/// emulator could not place it, is information about the SERVER, and a client branches on
/// the error code and the reason rather than on prose. The token itself is not echoed
/// either — the request already carried it.
pub fn invalid_token() -> ValidationException {
    ValidationException {
        message: INVALID_TOKEN_MESSAGE.to_string(),
        reason: REASON_INVALID_TOKEN.to_string(),
        field_list: None,
    }
}

/// The requirement a tag write that would leave a resource holding more tags than
/// [`TAGS_PER_RESOURCE_LIMIT`] failed.
///
/// The bound is `TagsMap`'s own modelled `@length(max: 50)` measured over the resource's
/// whole tag set ([`TAGS_PER_RESOURCE_LIMIT`]), so the requirement is that constraint
/// stated in the words a length ceiling is stated in — the same clause the boundary
/// validator renders for the same constraint, so one refusal of this bound cannot be
/// worded unlike the other.
pub fn tags_per_resource_requirement() -> String {
    format!("Member must have length less than or equal to {TAGS_PER_RESOURCE_LIMIT}")
}

/// The modelled `ValidationException` for a tag write that would leave a resource holding
/// more tags than [`TAGS_PER_RESOURCE_LIMIT`], stated against `member` — the member of
/// THAT request which carried the tags.
///
/// The same validation envelope every other service-decided refusal here is stated in
/// ([`field_validation_failed`]): the clause names the member and the requirement its
/// value failed, and neither the tags nor the count the write would have left behind is
/// echoed back — the request already carried the tags, and the resource's own count is
/// what `ListTagsForResource` reports.
pub fn too_many_tags(member: &str) -> ValidationException {
    field_validation_failed(member, &tags_per_resource_requirement())
}

/// The requirement a tag key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace failed
/// — the AWS-wide tagging rule stated as the constraint the member's value did not
/// satisfy.
pub fn reserved_tag_key_requirement() -> String {
    format!("Member must not contain a tag key beginning with {RESERVED_TAG_KEY_PREFIX}")
}

/// The modelled `ValidationException` for a request naming a tag key in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace, stated against `member` — the member of THAT
/// request which carried the key (`tags` on a tag write or a create, `tagKeys` on a
/// removal).
///
/// The same validation envelope every other service-decided refusal here is stated in
/// ([`field_validation_failed`]): the clause names the member and the requirement its
/// value failed, and the refused key is not echoed back — the request already carried it.
pub fn reserved_tag_keys(member: &str) -> ValidationException {
    field_validation_failed(member, &reserved_tag_key_requirement())
}

/// The modelled `ValidationException` for a request whose query identifiers name queries
/// the targeted inquiry does not own, stated against `member` — the member of THAT request
/// which carried them (`queryIdentifiers` on an export, `queryIdentifier` on a feedback
/// submission).
///
/// The clause is built from the REQUEST — the member that carried the value and the
/// requirement it failed — in the same validation envelope
/// [`constraint_violation_message`] renders for a modelled constraint, and without
/// echoing the refused identifiers back: the request already carried them.
pub fn unknown_query_identifiers(member: &str) -> ValidationException {
    field_validation_failed(member, UNKNOWN_QUERY_IDENTIFIER_REQUIREMENT)
}

/// The modelled `ValidationException` for an export asked of a resource whose processing
/// is still underway, naming the `state` it was asked in — the resource's own stored status
/// ([`ComplianceInquiry::processing_in_progress`]).
///
/// A state PRECONDITION rather than a field failure: nothing the request carried is
/// invalid, so `reason` is [`REASON_OTHER`] and `fieldList` names no member — unlike
/// [`field_validation_failed`], whose clause is about a value the request sent. The
/// sentence is the fixed text AWS states for this class, and the state it names is read off
/// the resource rather than off the request, which is the only part of it that varies.
pub fn unexportable_while_processing(state: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Unable to export resource in a `{state}` state. Please wait for processing to \
             complete."
        ),
        reason: REASON_OTHER.to_string(),
        field_list: None,
    }
}

/// The modelled `ValidationException` for a value the SERVICE itself refused on `member`,
/// against `requirement`.
///
/// The one envelope every such refusal is stated in: `ValidationExceptionReason`'s
/// `fieldValidationFailed`, the member named in `fieldList`, and the same clause in both
/// the message and the field entry ([`field_constraint_clause`]) — the AWS validation
/// envelope [`constraint_violation_message`] renders for a modelled constraint, without
/// echoing the refused value, which the request already carried.
pub fn field_validation_failed(member: &str, requirement: &str) -> ValidationException {
    field_validation_failed_over(vec![ValidationExceptionField {
        name: member.to_string(),
        message: field_constraint_clause(member, requirement),
    }])
}

/// The ONE envelope every field-validation refusal of this service is stated in, over the
/// members it names.
///
/// The modelled shape declares `reason` @required beside `message`, so a refusal carrying
/// only a message is not the modelled error at all: the reason a value the request carried
/// was refused is `ValidationExceptionReason`'s `fieldValidationFailed`, and `fieldList`
/// ("The field that caused the error, if applicable") names each refused member with the
/// clause its value failed. The message states those same clauses in the AWS validation
/// envelope, so the two halves of one refusal cannot disagree.
fn field_validation_failed_over(fields: Vec<ValidationExceptionField>) -> ValidationException {
    let clauses: Vec<&str> = fields.iter().map(|field| field.message.as_str()).collect();
    ValidationException {
        message: validation_envelope(&clauses),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(fields),
    }
}

/// The AWS validation envelope over `clauses`: `N validation error(s) detected: <clause>;
/// …`. Written in ONE place, so a refusal stated by a handler and one stated for a
/// modelled constraint are worded alike.
///
/// A refusal states at least one clause — there is no such thing as a validation error
/// with nothing wrong — so every caller checks that first, and the assertion keeps it that
/// way as callers are added rather than letting one render a count of none.
fn validation_envelope(clauses: &[&str]) -> String {
    debug_assert!(
        !clauses.is_empty(),
        "a validation refusal states at least one clause"
    );
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// One clause of a validation refusal: the member path and the requirement its value
/// failed.
pub fn field_constraint_clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// The clause [`unknown_query_identifiers`] states, in both the message and the
/// `fieldList` entry.
pub fn unknown_query_identifier_clause(member: &str) -> String {
    field_constraint_clause(member, UNKNOWN_QUERY_IDENTIFIER_REQUIREMENT)
}

/// The requirement half of [`unknown_query_identifier_clause`].
pub const UNKNOWN_QUERY_IDENTIFIER_REQUIREMENT: &str =
    "every query identifier must name a query of the compliance inquiry";

/// The `ValidationException` message this service states for a request the boundary
/// validator refused against the model's own constraints.
///
/// The AWS validation envelope — `N validation error(s) detected: <clause>; …`, each
/// clause naming the member path and the requirement it failed — with the REJECTED VALUE
/// left out of the clause: a present-but-invalid value is reported as `Value at '<path>'
/// …`, while an absent required member keeps the `Value null at '<path>' …` the framework
/// renders, because absence IS the fact the clause is stating. Returns `None` when there
/// is nothing to state, so the caller keeps the framework's default wording.
///
/// The requirement half of every clause comes from the framework's own rendering of the
/// violation, so a constraint it learns to check cannot drift from this wording; only the
/// echo of the value is removed.
pub fn constraint_violation_message(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause_without_rejected_value).collect();
    let clauses: Vec<&str> = clauses.iter().map(String::as_str).collect();
    Some(validation_envelope(&clauses))
}

/// The modelled `ValidationException` this service answers a request the boundary
/// validator refused against the model's own constraints with.
///
/// The SAME refusal the handlers state for a value they refuse themselves
/// ([`field_validation_failed_over`]): the AWS validation envelope in `message`, the
/// required `reason` set to `fieldValidationFailed`, and one `fieldList` entry per refused
/// member naming it and the clause its value failed. `None` when there is nothing to
/// state, so a caller with no violation to report keeps the framework's own answer.
pub fn constraint_violation_exception(violations: &[Violation]) -> Option<ValidationException> {
    if violations.is_empty() {
        return None;
    }
    Some(field_validation_failed_over(
        violations
            .iter()
            .map(|violation| ValidationExceptionField {
                // The member the clause is stated against is the violation's own path, so
                // the entry names exactly what the message names — including a path that
                // descends into a structure, list or map member.
                name: violation.path.clone(),
                message: clause_without_rejected_value(violation),
            })
            .collect(),
    ))
}

/// One violation's clause with the rejected value removed from it.
///
/// The framework renders `Value '<shown>' at '<path>' failed to satisfy constraint: …`;
/// the value is known here, so the echo is removed by name rather than by parsing the
/// sentence. A clause that does not start with the echo (an absent member's `Value null`,
/// or a wording the framework changes) is kept exactly as the framework rendered it.
fn clause_without_rejected_value(violation: &Violation) -> String {
    let rendered = violation.message();
    let Some(shown) = violation.value.as_deref() else {
        return rendered;
    };
    match rendered.strip_prefix(&format!("Value '{shown}' at ")) {
        Some(rest) => format!("Value at {rest}"),
        None => rendered,
    }
}

/// The SET a request's list member denotes: `values` ascending and without repeats.
///
/// Decided in ONE place for every list member of this service that names a set rather
/// than a sequence — the queries an export covers, the reasons a feedback submission
/// gives — because neither the order a request listed such values in nor a repeat of one
/// is part of what it named.
pub fn request_set<Value: Ord>(mut values: Vec<Value>) -> Vec<Value> {
    values.sort_unstable();
    values.dedup();
    values
}

/// The page size a paginated operation of this service serves for `max_results`.
///
/// `MaxResultsAttribute` is modelled `@range(min: 1, max: 300)` and the boundary
/// validator refused anything outside it before a handler ran, so the only case left is
/// an absent value: serve the largest page the model allows. The `clamp` states that
/// invariant rather than correcting a caller — a value reaching here outside the range is
/// a validator bug, which the debug assertion surfaces instead of silently repairing.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(REPORT_PAGE_LIMIT);
    debug_assert!(
        (1..=REPORT_PAGE_LIMIT).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    // Clamped into 1..=300 first, so the conversion cannot fail; it is still written
    // without a panicking combinator, because nothing a request carries may abort a
    // handler. The floor is the fallback for the same reason.
    usize::try_from(requested.clamp(1, REPORT_PAGE_LIMIT)).unwrap_or(1)
}

/// The position a paginated operation of this service resumes its collection at, for the
/// `nextToken` the request carried and the PARSE half of that operation's own cursor pair.
///
/// A request carrying no token starts at the beginning. A token is opaque, so the only
/// thing it has to be is one of that operation's own cursors: a value `parse` does not
/// place was never issued there, and is refused with the service's one fixed
/// invalid-token payload ([`invalid_token`]) rather than honoured as an offset.
///
/// The refusal is handed back for the caller to lift into its OWN modelled
/// `ValidationException` variant — the only part of this decision that differs between
/// the paginated operations.
pub fn resume_position(
    next_token: Option<&str>,
    parse: impl FnOnce(&str) -> Option<usize>,
) -> Result<usize, ValidationException> {
    match next_token {
        None => Ok(0),
        Some(token) => parse(token).ok_or_else(invalid_token),
    }
}

/// One page of `entries`, and the `nextToken` that resumes exactly after it.
///
/// The page-walk half of the pagination rule, decided in ONE place for every paginated
/// operation of this service, as [`page_size`] decides how large a page is and the cursor
/// codec decides what a token is: skip the entries already reported (`resume_at`), cut the
/// page by minting a token at the position reached once it holds `page_size` entries, and
/// otherwise report the entry's projection and advance.
///
/// So the position a token carries is the number of entries the operation has already
/// REPORTED, which is what every cursor of this service means. `entries` is therefore the
/// iterator of what the operation reports — already filtered, so an entry it passes over
/// occupies no position and a token is only ever cut where a further entry really follows
/// — and `project` is called only for the entries the page carries, so a projection that
/// materialises a clock-owed transition does so for exactly those.
pub fn walk_page<Entry, Projected>(
    entries: impl IntoIterator<Item = Entry>,
    resume_at: usize,
    page_size: usize,
    mint: impl Fn(usize) -> String,
    mut project: impl FnMut(Entry) -> Projected,
) -> (Vec<Projected>, Option<String>) {
    let entries = entries.into_iter();
    // The page holds at most what the collection still carries, and at most a page: the
    // reservation comes from the iterator's own bound and from [`page_size`], which is
    // already clamped into the modelled range, never from a number a request carried.
    let (_, remaining) = entries.size_hint();
    let mut items = Vec::with_capacity(remaining.map_or(page_size, |len| page_size.min(len)));
    let mut next_token = None;
    let mut position = 0usize;
    for entry in entries {
        if position < resume_at {
            position += 1;
            continue;
        }
        if items.len() >= page_size {
            next_token = Some(mint(position));
            break;
        }
        items.push(project(entry));
        position += 1;
    }
    (items, next_token)
}

/// A digest of the request one `@idempotencyToken` identifies, so the same token replayed
/// on a DIFFERENT request can be told from a retry of the same one.
///
/// Every member that shapes what the call recorded is absorbed, each labelled and
/// LENGTH-PREFIXED, so no two different requests can produce the same digest by splitting
/// their fields differently; an optional member absorbs its PRESENCE before its value, so
/// a member the request left out is distinguishable from every value it could have carried,
/// the empty one included.
#[derive(Debug, Default)]
pub struct RequestFingerprint {
    hasher: Sha256,
}

/// The presence marker [`RequestFingerprint`] absorbs for an optional member the request
/// named a value for.
const PRESENT: u8 = 1;

/// The presence marker [`RequestFingerprint`] absorbs for an optional member the request
/// left out.
const ABSENT: u8 = 0;

impl RequestFingerprint {
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb one labelled, length-prefixed field.
    pub fn absorb(&mut self, label: &str, value: &[u8]) {
        self.hasher
            .update(u64::try_from(label.len()).unwrap_or(u64::MAX).to_be_bytes());
        self.hasher.update(label.as_bytes());
        self.hasher
            .update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        self.hasher.update(value);
    }

    /// Absorb a string-valued member.
    pub fn absorb_str(&mut self, label: &str, value: &str) {
        self.absorb(label, value.as_bytes());
    }

    /// Absorb an optional string-valued member, PRESENCE included.
    pub fn absorb_optional_str(&mut self, label: &str, value: Option<&str>) {
        self.absorb_optional(label, value.map(str::as_bytes));
    }

    /// Absorb an optional integer member, PRESENCE included.
    pub fn absorb_optional_i32(&mut self, label: &str, value: Option<i32>) {
        match value {
            Some(value) => self.absorb_optional(label, Some(&value.to_be_bytes())),
            None => self.absorb_optional(label, None),
        }
    }

    /// Absorb an optional member's PRESENCE and then, when it has one, its value.
    ///
    /// The presence marker is what keeps a member the request left out distinguishable
    /// from every value it could have carried — including the empty one, which absorbing
    /// an absent member as a zero-length value would collide with.
    fn absorb_optional(&mut self, label: &str, value: Option<&[u8]>) {
        match value {
            Some(value) => {
                self.absorb(label, &[PRESENT]);
                self.absorb(label, value);
            }
            None => self.absorb(label, &[ABSENT]),
        }
    }

    /// The digest, as the hex string an idempotency record stores.
    pub fn finish(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsArtifactArtifactState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsArtifactArtifactState>)
}

// ---------------------------------------------------------------------------
// The seeded, service-owned catalog.
// ---------------------------------------------------------------------------

/// Seconds since the Unix epoch at 2023-01-01T00:00:00Z.
const T2023JAN: u64 = 1_672_531_200;
/// Seconds since the Unix epoch at 2023-04-01T00:00:00Z.
const T2023APR: u64 = 1_680_307_200;
/// Seconds since the Unix epoch at 2023-07-01T00:00:00Z.
const T2023JUL: u64 = 1_688_169_600;
/// Seconds since the Unix epoch at 2024-01-01T00:00:00Z.
const T2024JAN: u64 = 1_704_067_200;
/// Seconds since the Unix epoch at 2024-04-01T00:00:00Z.
const T2024APR: u64 = 1_711_929_600;
/// Seconds since the Unix epoch at 2024-07-01T00:00:00Z.
const T2024JUL: u64 = 1_719_792_000;
/// Seconds since the Unix epoch at 2025-01-01T00:00:00Z.
const T2025JAN: u64 = 1_735_689_600;
/// Seconds since the Unix epoch at 2025-04-01T00:00:00Z.
const T2025APR: u64 = 1_743_465_600;
/// Seconds since the Unix epoch at 2025-07-01T00:00:00Z.
const T2025JUL: u64 = 1_751_328_000;
/// Seconds since the Unix epoch at 2025-10-01T00:00:00Z.
const T2025OCT: u64 = 1_759_276_800;
/// Seconds since the Unix epoch at 2026-01-01T00:00:00Z.
const T2026JAN: u64 = 1_767_225_600;

// The ANNUAL AWS reporting cycle a published report's period runs on: 1 November to 31
// October of the following year. The corpus's recorded Artifact catalog bodies carry
// exactly such a window on a report entry (`periodStart` 2024-11-01, `periodEnd`
// 2025-10-31, quoted in
// `issues/things-that-do-not-work/20261022T085754Z-artifact-compliance-inquiry-projection-returns-epoch-timestamps-and.md`),
// so the report the recording names at three published versions carries that window and
// its earlier versions the preceding years' windows.

/// Seconds since the Unix epoch at 2018-11-01T00:00:00Z.
const T2018NOV: u64 = 1_541_030_400;
/// Seconds since the Unix epoch at 2019-10-31T00:00:00Z.
const T2019OCT31: u64 = 1_572_480_000;
/// Seconds since the Unix epoch at 2019-11-01T00:00:00Z.
const T2019NOV: u64 = 1_572_566_400;
/// Seconds since the Unix epoch at 2020-10-31T00:00:00Z.
const T2020OCT31: u64 = 1_604_102_400;
/// Seconds since the Unix epoch at 2020-11-01T00:00:00Z.
const T2020NOV: u64 = 1_604_188_800;
/// Seconds since the Unix epoch at 2021-10-31T00:00:00Z.
const T2021OCT31: u64 = 1_635_638_400;
/// Seconds since the Unix epoch at 2021-11-01T00:00:00Z.
const T2021NOV: u64 = 1_635_724_800;
/// Seconds since the Unix epoch at 2022-10-31T00:00:00Z.
const T2022OCT31: u64 = 1_667_174_400;
/// Seconds since the Unix epoch at 2022-11-01T00:00:00Z.
const T2022NOV: u64 = 1_667_260_800;
/// Seconds since the Unix epoch at 2023-10-31T00:00:00Z.
const T2023OCT31: u64 = 1_698_710_400;
/// Seconds since the Unix epoch at 2023-11-01T00:00:00Z.
const T2023NOV: u64 = 1_698_796_800;
/// Seconds since the Unix epoch at 2024-10-31T00:00:00Z.
const T2024OCT31: u64 = 1_730_332_800;
/// Seconds since the Unix epoch at 2024-11-01T00:00:00Z.
const T2024NOV: u64 = 1_730_419_200;
/// Seconds since the Unix epoch at 2025-10-31T00:00:00Z.
const T2025OCT31: u64 = 1_761_868_800;

/// The message `UploadState` `COMPLETE` carries — one of `StatusMessage`'s documented
/// upload messages.
const UPLOAD_COMPLETE_MESSAGE: &str = "Document upload is complete.";

/// One seeded report version, before it becomes a [`ReportVersion`].
struct SeedVersion {
    version: i64,
    period_start: u64,
    period_end: u64,
    published_at: u64,
    sequence_number: i64,
}

/// One seeded report, before it becomes a [`Report`].
struct SeedReport {
    id: &'static str,
    name: &'static str,
    /// The report's PUBLISHED description — the text the publisher writes about the
    /// document, which is what `ReportSummary$description` and `ReportDetail$description`
    /// report. Both target `LongStringAttribute` (`@length(min: 1, max: 2048)`,
    /// `@pattern ^[^<>]*$`), whose ceiling is the room a published description of several
    /// sentences needs: a reader of the catalog gets the document's own description, not a
    /// one-line label naming it.
    ///
    /// One per REPORT rather than per version: a description describes the document, and
    /// the window one version covers is reported by that version's own period members.
    description: &'static str,
    series: &'static str,
    category: &'static str,
    acceptance_type: &'static str,
    /// The file name each version's report document is published under. The version
    /// lives in the document's PATH, so two versions of one report publish the same
    /// file name and different documents.
    document_object: &'static str,
    /// The file name of the TERM document governing this report, which is also the
    /// term's identity: a grant minted under it covers every report it governs.
    term_object: &'static str,
    /// Where the compliance material this row publishes is PUBLISHED — the link a
    /// citation of this source reports ([`Report::source_link`]).
    ///
    /// Catalog data, as `description` above is: AWS publishes each compliance programme's
    /// material under a page of its own, and a programme that publishes several documents
    /// publishes them under that one page, so the rows of such a programme carry one link.
    /// The corpus discloses a published link for none of these rows, so the values below
    /// are this catalog's, written in the same register as the descriptions beside them.
    source_link: &'static str,
    versions: &'static [SeedVersion],
}

/// Every report in the catalog is published by AWS, under the AWS product.
const COMPANY_NAME: &str = "AWS";
/// See [`COMPANY_NAME`].
const PRODUCT_NAME: &str = "AWS";

/// The catalog this emulator serves: AWS Artifact's published reports.
///
/// AWS Artifact has no `CreateReport`, so this set is not something a caller can bring
/// into being — it is the published catalog itself, the same for every account, keyed by
/// the `ReportId` AWS assigned each report. A client's whole report flow starts by
/// listing it and then naming one of these ids (or one of these series) to
/// `GetReportMetadata`, `GetTermForReport` and `GetReport`, which is why the two rows the
/// corpus discloses an id for carry the id and the published name AWS publishes them
/// under, as a recording of a live `ListReports` shows them: `report-073F0fUCdVVNKJrF`
/// and `report-5eevElBv6MyRmQsD`. An id the corpus discloses names the report AWS
/// publishes under it and no other, so it is carried by THAT row: the other ids the same
/// recording discloses name reports of the accessibility-conformance class and are
/// therefore seeded in [`accessibility_reports`].
///
/// Every other row carries a report AWS really publishes in Artifact (the SOC reports, the
/// ISO certificates, the PCI DSS attestation, the FedRAMP, IRAP and C5 packages and the
/// CSA questionnaire) under an id this catalog assigns, because the recording names an id
/// only for the two above: those ids satisfy `ReportId`'s pattern and are this
/// emulator's, not a claim that AWS issued them.
///
/// This is one row per compliance PROGRAMME. The accessibility-conformance reports, which
/// AWS publishes one of per evaluated PRODUCT, are the catalog's other class and are
/// seeded in [`accessibility_reports`]; [`seed_catalog`] materialises both, so the catalog
/// `ListReports` enumerates is the whole published inventory this emulator holds.
///
/// Every row's `description` is the PUBLISHED description of its document — the prose a
/// publisher writes about a compliance document, which is what the member carries — and
/// never a label restating the row's `name`. It is as long as the publisher wrote it: the
/// rows whose published text the corpus records carry that text, one of them a single
/// sentence and one a paragraph, so nothing here pads a description out to a fixed shape.
/// The remaining rows' texts are this emulator's own, written in that same register,
/// because the corpus discloses no published description for them and no rule derives one.
///
/// The rows span what the report surface generalises over: nine compliance-programme
/// series (the accessibility-conformance series beside them makes ten), two categories,
/// both `AcceptanceType` values, one, two, three and seven published versions, and
/// reporting periods on the calendar-annual, the November-annual and the semiannual
/// windows AWS publishes on.
///
/// A row's period members are the window the REPORT itself covers, version by version: a
/// programme publishes a version for a cycle and the next version for the following one,
/// so a window belongs to the row that published it and is never shared with another row
/// because both happen to be listed on one page.
///
/// Every `id` matches `ReportId`'s `^report-[a-zA-Z0-9]{16}$`. A published title is
/// reported as it is published, punctuation included: `ShortStringAttribute`'s `@pattern`
/// bounds what a REQUEST may send, and the recorded catalog carries titles outside it (see
/// [`accessibility_reports`]).
///
/// Each report publishes its OWN term document, so a term grant minted for one report
/// covers every version of that report and no other report here. (Nothing in the rule
/// requires that: were two reports to share a term document, one acceptance would cover
/// both, which is what makes the grant a receipt for the TERM.)
const SEED: &[SeedReport] = &[
    SeedReport {
        id: "report-073F0fUCdVVNKJrF",
        name: "AWS Outposts Rack Information Security FAQ",
        description: "The AWS Outposts Information Security FAQ document is designed to help \
                      customer teams responsible for information security evaluate the \
                      service against their organizational requirements. The document is \
                      aligned to common questions relating to people, process, and \
                      technology that this audience often ask. This document is reviewed \
                      and updated regularly with the latest information.",
        series: "AWS Services",
        category: "Alignment Documents",
        acceptance_type: "PASSTHROUGH",
        document_object: "AWS-Outposts-Rack-Information-Security-FAQ.pdf",
        term_object: "AWS-Outposts-Rack-Information-Security-FAQ-Term.txt",
        source_link: "https://aws.amazon.com/compliance/resources/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2022NOV,
                period_end: T2023OCT31,
                published_at: T2022NOV,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2023NOV,
                period_end: T2024OCT31,
                published_at: T2023NOV,
                sequence_number: 2,
            },
            SeedVersion {
                version: 3,
                period_start: T2024NOV,
                period_end: T2025OCT31,
                published_at: T2024NOV,
                sequence_number: 3,
            },
        ],
    },
    SeedReport {
        // An id this catalog assigns. The recorded `ListReports` body names
        // `report-0c1IXmudeXuTlHJJ` as an accessibility-conformance report, so that id is
        // carried by that row in [`accessibility_reports`] and this certificate — which
        // AWS publishes but the recording discloses no id for — is keyed here.
        id: "report-T5xWnQ8dKb3VzL6p",
        name: "ISO 27001 2022 Certification",
        description: "The ISO/IEC 27001:2022 certificate records the certification of the \
                      AWS information security management system by an accredited \
                      independent registrar. It names the services and AWS Regions in the \
                      scope of the certification and the dates of the certification cycle \
                      it was issued for. The certificate is reissued after each \
                      surveillance and recertification audit.",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "ISO-27001-2022.pdf",
        term_object: "ISO-27001-2022-Term.txt",
        source_link: "https://aws.amazon.com/compliance/iso-27001-faqs/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2023JAN,
                period_end: T2024JAN,
                published_at: T2023JAN,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2024JAN,
                period_end: T2025JAN,
                published_at: T2024JAN,
                sequence_number: 2,
            },
            SeedVersion {
                version: 3,
                period_start: T2025JAN,
                period_end: T2026JAN,
                published_at: T2025JAN,
                sequence_number: 3,
            },
        ],
    },
    SeedReport {
        id: "report-5eevElBv6MyRmQsD",
        // The title AWS publishes this assessment under, as the recorded catalog entry for
        // this id carries it: the jurisdiction, the regulation assessed against and the
        // kind of document, spelled in full. A published title is reported as published —
        // an abbreviation of it names a document the publisher does not publish.
        name: "United Arab Emirates (UAE) Information Assurance Regulation (IAR) \
               Compliance Assessment Report",
        // The description AWS publishes for this document, as the corpus records it: one
        // sentence naming the assessment and the regulation it was made against. A
        // published description is as long as its publisher wrote it — this one is a single
        // sentence where the rows below carry a paragraph — so nothing here pads it out.
        description: "Independent compliance assessment against the United Arab Emirates \
                      (UAE) Information Assurance Regulation (IAR).",
        series: "UAE TDRA IAR",
        category: "Certifications and Attestations",
        // `PASSTHROUGH`, as the recorded catalog entry for this report carries it: no
        // click-through acceptance of its term is required of a downloader.
        acceptance_type: ACCEPTANCE_TYPE_PASSTHROUGH,
        document_object: "UAE-TDRA-IAR.pdf",
        term_object: "UAE-TDRA-IAR-Term.txt",
        source_link: "https://aws.amazon.com/compliance/uae-iar/",
        // This assessment is published on the ANNUAL November cycle: a version covers 1
        // November to 31 October of the following year, which is the window the recorded
        // catalog entry carries for the version it reports, and its earlier versions carry
        // the preceding years' windows of the same cycle. An assessment report is published
        // once the period it reports on has elapsed, so a version's `published_at` closes
        // its window.
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2018NOV,
                period_end: T2019OCT31,
                published_at: T2019OCT31,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2019NOV,
                period_end: T2020OCT31,
                published_at: T2020OCT31,
                sequence_number: 2,
            },
            SeedVersion {
                version: 3,
                period_start: T2020NOV,
                period_end: T2021OCT31,
                published_at: T2021OCT31,
                sequence_number: 3,
            },
            SeedVersion {
                version: 4,
                period_start: T2021NOV,
                period_end: T2022OCT31,
                published_at: T2022OCT31,
                sequence_number: 4,
            },
            SeedVersion {
                version: 5,
                period_start: T2022NOV,
                period_end: T2023OCT31,
                published_at: T2023OCT31,
                sequence_number: 5,
            },
            SeedVersion {
                version: 6,
                period_start: T2023NOV,
                period_end: T2024OCT31,
                published_at: T2024OCT31,
                sequence_number: 6,
            },
            SeedVersion {
                version: 7,
                period_start: T2024NOV,
                period_end: T2025OCT31,
                published_at: T2025OCT31,
                sequence_number: 7,
            },
        ],
    },
    SeedReport {
        id: "report-9pQ2mT8xLbV4sZ1c",
        name: "SOC 2 Type 2 Report",
        description: "The AWS SOC 2 Type 2 report describes the AWS control environment and \
                      the results of the independent auditor's tests of the controls \
                      relevant to the security, availability and confidentiality trust \
                      services criteria. It covers the services and AWS Regions listed in \
                      the report over the examination period stated on it. The report is \
                      confidential and is released under the term the customer accepts \
                      before downloading it.",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "SOC2-Type2.pdf",
        term_object: "SOC2-Type2-Term.txt",
        source_link: "https://aws.amazon.com/compliance/soc-faqs/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2024JAN,
                period_end: T2024JUL,
                published_at: T2024JUL,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2024JUL,
                period_end: T2025JAN,
                published_at: T2025JAN,
                sequence_number: 2,
            },
        ],
    },
    SeedReport {
        id: "report-Af3KdN7yRw2HqS5e",
        name: "PCI DSS Attestation of Compliance",
        description: "The PCI DSS Attestation of Compliance records the assessment of the \
                      AWS cardholder data environment against the Payment Card Industry \
                      Data Security Standard by a qualified security assessor. It lists \
                      the services and AWS Regions in scope and the requirements each one \
                      was assessed against. Customer teams use it as the service provider \
                      attestation for their own PCI DSS assessments.",
        series: "PCI",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "PCI-DSS-AOC.pdf",
        term_object: "PCI-DSS-AOC-Term.txt",
        source_link: "https://aws.amazon.com/compliance/pci-dss-level-1-faqs/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2023JAN,
                period_end: T2024JAN,
                published_at: T2024JAN,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2024JAN,
                period_end: T2025JAN,
                published_at: T2025JAN,
                sequence_number: 2,
            },
        ],
    },
    SeedReport {
        id: "report-Bq7WmF3xTk9ZpL2a",
        name: "SOC 1 Report",
        description: "The AWS SOC 1 report describes the AWS control environment and the \
                      results of the independent auditor's tests of the controls that are \
                      relevant to a customer's internal control over financial reporting. \
                      It covers the services and AWS Regions listed in the report over the \
                      examination period stated on it. Customer teams and their auditors \
                      use it where AWS is a service organization in the scope of a \
                      financial audit.",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "AWS-SOC-1.pdf",
        term_object: "AWS-SOC-1-Term.txt",
        source_link: "https://aws.amazon.com/compliance/soc-faqs/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2024JAN,
                period_end: T2024JUL,
                published_at: T2024JUL,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2024JUL,
                period_end: T2025JAN,
                published_at: T2025JAN,
                sequence_number: 2,
            },
        ],
    },
    SeedReport {
        id: "report-C4tYhR8nJd6VxQ3b",
        name: "SOC 3 Report",
        description: "The AWS SOC 3 report is the general use summary of the SOC 2 \
                      examination: it carries the independent auditor's opinion on the AWS \
                      control environment without the detailed tests of controls. It covers \
                      the services and AWS Regions listed in the report over the \
                      examination period stated on it. Unlike the SOC 1 and SOC 2 reports \
                      it may be shared freely.",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "AWS-SOC-3.pdf",
        term_object: "AWS-SOC-3-Term.txt",
        source_link: "https://aws.amazon.com/compliance/soc-faqs/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2024JUL,
            period_end: T2025JAN,
            published_at: T2025JAN,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-D9kPzX2mLb7TnR4c",
        name: "ISO 27017 Certification",
        description: "The ISO/IEC 27017:2015 certificate records the certification of the \
                      AWS information security controls for cloud services by an \
                      accredited independent registrar. It names the services and AWS \
                      Regions in the scope of the certification and the dates of the \
                      certification cycle it was issued for. The controls it covers extend \
                      the ISO/IEC 27002 guidance to the provision and use of cloud \
                      services.",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "ISO-27017.pdf",
        term_object: "ISO-27017-Term.txt",
        source_link: "https://aws.amazon.com/compliance/iso-27017-faqs/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2023JAN,
                period_end: T2024JAN,
                published_at: T2023JAN,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2024JAN,
                period_end: T2025JAN,
                published_at: T2024JAN,
                sequence_number: 2,
            },
        ],
    },
    SeedReport {
        id: "report-E2nVbK6yQw8MsZ5d",
        name: "ISO 27018 Certification",
        description: "The ISO/IEC 27018:2019 certificate records the certification of the \
                      AWS controls for the protection of personally identifiable \
                      information processed in a public cloud by an accredited independent \
                      registrar. It names the services and AWS Regions in the scope of the \
                      certification and the dates of the certification cycle it was issued \
                      for. Customer teams use it as evidence of the privacy controls a \
                      public cloud processor is certified against.",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "ISO-27018.pdf",
        term_object: "ISO-27018-Term.txt",
        source_link: "https://aws.amazon.com/compliance/iso-27018-faqs/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2024JAN,
            period_end: T2025JAN,
            published_at: T2024JAN,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-F6sJdL9xRt3WqN7e",
        name: "ISO 27701 Certification",
        description: "The ISO/IEC 27701:2019 certificate records the certification of the \
                      AWS privacy information management system by an accredited \
                      independent registrar. It names the services and AWS Regions in the \
                      scope of the certification and the dates of the certification cycle \
                      it was issued for. The certificate covers the extension of the \
                      information security management system to the processing of \
                      personally identifiable information.",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "ISO-27701.pdf",
        term_object: "ISO-27701-Term.txt",
        source_link: "https://aws.amazon.com/compliance/iso-27701-faqs/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2024JUL,
            period_end: T2025JUL,
            published_at: T2024JUL,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-G3mQfT7bXz5KpV8h",
        name: "ISO 9001 Certification",
        description: "The ISO 9001:2015 certificate records the certification of the AWS \
                      quality management system by an accredited independent registrar. It \
                      names the services and AWS Regions in the scope of the certification \
                      and the dates of the certification cycle it was issued for. Customer \
                      teams in regulated industries use it as evidence of the quality \
                      management AWS applies to the services in that scope.",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        document_object: "ISO-9001.pdf",
        term_object: "ISO-9001-Term.txt",
        source_link: "https://aws.amazon.com/compliance/iso-9001-faqs/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2023JUL,
            period_end: T2024JUL,
            published_at: T2023JUL,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-H8wRnP4cYk2JsL6m",
        name: "FedRAMP Moderate Customer Package",
        description: "The FedRAMP Moderate customer package carries the security \
                      documentation for the AWS Regions that hold a FedRAMP Moderate \
                      authorization, including the system security plan summary and the \
                      assessment results an agency reviews before it issues its own \
                      authority to operate. It names the services inside the authorization \
                      boundary over the period stated on the package. Access is limited to \
                      the customers the package may be released to.",
        series: "FedRAMP",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "FedRAMP-Moderate-Customer-Package.pdf",
        term_object: "FedRAMP-Moderate-Customer-Package-Term.txt",
        source_link: "https://aws.amazon.com/compliance/fedramp/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2024JAN,
            period_end: T2025JAN,
            published_at: T2024APR,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-J5zXkM9dVb3QtR7n",
        name: "IRAP Assessment Report",
        description: "The IRAP assessment report describes the assessment of the AWS Asia \
                      Pacific Region in its scope against the controls of the Australian \
                      Government Information Security Manual by an endorsed IRAP assessor. \
                      It lists the services assessed and the control set each one was \
                      assessed against over the period stated on the report. Australian \
                      government customers use it when accrediting their own systems.",
        series: "IRAP",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "IRAP-Assessment.pdf",
        term_object: "IRAP-Assessment-Term.txt",
        source_link: "https://aws.amazon.com/compliance/irap/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2024APR,
            period_end: T2025APR,
            published_at: T2024APR,
            sequence_number: 1,
        }],
    },
    SeedReport {
        id: "report-K7bNqS2fWx8LmT4p",
        name: "C5 Attestation Report",
        description: "The C5 attestation report records the attestation of the AWS controls \
                      against the Cloud Computing Compliance Controls Catalogue that the \
                      German Federal Office for Information Security publishes. It lists \
                      the services and AWS Regions in scope and the basic and additional \
                      criteria each one was attested against over the period stated on the \
                      report. Customer teams in Germany use it to meet their own cloud \
                      assurance requirements.",
        series: "C5",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        document_object: "C5-Attestation.pdf",
        term_object: "C5-Attestation-Term.txt",
        source_link: "https://aws.amazon.com/compliance/bsi-c5/",
        versions: &[
            SeedVersion {
                version: 1,
                period_start: T2023APR,
                period_end: T2024APR,
                published_at: T2023APR,
                sequence_number: 1,
            },
            SeedVersion {
                version: 2,
                period_start: T2024APR,
                period_end: T2025APR,
                published_at: T2024APR,
                sequence_number: 2,
            },
        ],
    },
    SeedReport {
        id: "report-L4dTvJ8hZn6RkQ9s",
        name: "Cloud Security Alliance CAIQ",
        description: "The Consensus Assessments Initiative Questionnaire is the Cloud \
                      Security Alliance questionnaire answered for AWS: it records the AWS \
                      response to each control question of the Cloud Controls Matrix. It \
                      covers the services and AWS Regions named in the questionnaire for \
                      the period stated on it. Customer teams use it to compare the AWS \
                      control environment with the CSA control framework.",
        series: "CSA",
        category: "Alignment Documents",
        acceptance_type: "PASSTHROUGH",
        document_object: "CSA-CAIQ.pdf",
        term_object: "CSA-CAIQ-Term.txt",
        source_link: "https://aws.amazon.com/compliance/csa/",
        versions: &[SeedVersion {
            version: 1,
            period_start: T2024JAN,
            period_end: T2025JAN,
            published_at: T2024JAN,
            sequence_number: 1,
        }],
    },
];

/// An instant from its seconds since the Unix epoch.
fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Build the catalog from every class the published inventory holds: the one row per
/// compliance programme in [`SEED`] and the one row per evaluated product in
/// [`accessibility_reports::ACCESSIBILITY_SEED`]. A report belongs to the catalog whatever
/// class seeds it, so every one of them is materialised here and `ListReports` enumerates
/// the union. Deterministic: the same records, keyed by `ReportId`, every time the state
/// is constructed.
fn seed_catalog() -> BTreeMap<String, Report> {
    SEED.iter()
        .chain(accessibility_reports::ACCESSIBILITY_SEED.iter())
        .map(|seed| {
            let report = Report {
                term: Document::new(seed.term_object, term_document(seed).into_bytes()),
                id: seed.id.to_string(),
                name: seed.name.to_string(),
                series: seed.series.to_string(),
                category: seed.category.to_string(),
                company_name: COMPANY_NAME.to_string(),
                product_name: PRODUCT_NAME.to_string(),
                acceptance_type: seed.acceptance_type.to_string(),
                // The published location of this compliance document, held beside its name
                // and its published text because it locates the SOURCE: a citation of it
                // reports this stored value rather than a locator assembled per read
                // ([`QueryCitation::source_link`]).
                source_link: seed.source_link.to_string(),
                // Every report AWS publishes in the Artifact catalog is visible to every
                // account; a report scoped to one customer is not part of this catalog.
                visibility: VISIBILITY_PUBLIC.to_string(),
                versions: seed
                    .versions
                    .iter()
                    .map(|version| {
                        (
                            version.version,
                            ReportVersion {
                                version: version.version,
                                state: "PUBLISHED".to_string(),
                                upload_state: "COMPLETE".to_string(),
                                status_message: Some(UPLOAD_COMPLETE_MESSAGE.to_string()),
                                description: seed.description.to_string(),
                                period_start: at(version.period_start),
                                period_end: at(version.period_end),
                                published_at: at(version.published_at),
                                sequence_number: version.sequence_number,
                                document: Document::new(
                                    seed.document_object,
                                    report_document(seed, version),
                                ),
                            },
                        )
                    })
                    .collect(),
            };
            (report.id.clone(), report)
        })
        .collect()
}

/// The term document governing one seeded report.
///
/// Plain text, and it says what it is: the real AWS Artifact term text is AWS's, so this
/// document states the acceptance the emulator records rather than inventing an
/// agreement. Accepting it through `GetTermForReport` is what mints the `termToken`
/// `GetReport` requires.
fn term_document(seed: &SeedReport) -> String {
    format!(
        "AWS Artifact term document for {name} ({series})\n\
         \n\
         Acceptance type: {acceptance_type}\n\
         Term document: {object}\n\
         \n\
         This term document is published by the AWS Artifact emulator. Accepting it with\n\
         GetTermForReport mints the termToken that GetReport requires to serve any\n\
         version of a report this term governs. It is not the text of AWS's own\n\
         agreement.\n",
        name = seed.name,
        series = seed.series,
        acceptance_type = seed.acceptance_type,
        object = seed.term_object,
    )
}

/// The report document one published version carries: a one-page PDF naming the report,
/// the version and the period it covers.
///
/// The bytes are written by the `pdf-writer` crate, so what the data plane serves is a
/// document a PDF reader opens rather than bytes shaped like one. What the page SAYS is
/// the stored record — an AWS Artifact report's real contents are AWS's, and nothing
/// here invents them.
fn report_document(seed: &SeedReport, version: &SeedVersion) -> Vec<u8> {
    pdf_page(&[
        format!("{} - version {}", seed.name, version.version),
        String::new(),
        format!("Report id: {}", seed.id),
        format!("Series: {}  Category: {}", seed.series, seed.category),
        format!("Description: {}", seed.description),
        format!(
            "Reporting period: {} to {} (seconds since the Unix epoch)",
            version.period_start, version.period_end
        ),
        String::new(),
        "Published by the AWS Artifact emulator. The contents of the real report are".to_string(),
        "AWS's own and are not reproduced here.".to_string(),
    ])
}

/// A one-page PDF document showing `lines`, one per line, in the Base-14 Helvetica face
/// every PDF reader carries (so nothing is embedded and the document stands alone).
fn pdf_page(lines: &[String]) -> Vec<u8> {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

    // The ISO 216 A4 page box in PDF points (1/72 inch), the size AWS Artifact publishes
    // its reports at.
    let a4 = Rect::new(0.0, 0.0, 595.28, 841.89);
    /// Point size of the body text, and the baseline-to-baseline distance that goes with
    /// it.
    const FONT_SIZE: f32 = 11.0;
    /// See [`FONT_SIZE`].
    const LEADING: f32 = 15.0;

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let page_id = Ref::new(3);
    let font_id = Ref::new(4);
    let content_id = Ref::new(5);
    let font_name = Name(b"F1");

    let mut content = Content::new();
    content.begin_text();
    content.set_font(font_name, FONT_SIZE);
    // The first move is from the page origin to the top-left text start; every later one
    // steps down one line.
    content.next_line(56.0, 785.0);
    for line in lines {
        content.show(Str(line.as_bytes()));
        content.next_line(0.0, -LEADING);
    }
    content.end_text();
    let content = content.finish();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);
    let mut page = pdf.page(page_id);
    page.parent(page_tree_id);
    page.media_box(a4);
    page.contents(content_id);
    page.resources().fonts().pair(font_name, font_id);
    page.finish();
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.stream(content_id, &content);
    pdf.finish()
}

// ---------------------------------------------------------------------------
// The seeded customer-agreement set.
// ---------------------------------------------------------------------------

/// One seeded customer-agreement, before it becomes a [`CustomerAgreement`].
struct SeedAgreement {
    id: &'static str,
    name: &'static str,
    agreement_id: &'static str,
    description: &'static str,
    agreement_type: &'static str,
    state: &'static str,
    effective_start: u64,
    effective_end: Option<u64>,
    acceptance_terms: &'static [&'static str],
    terminate_terms: &'static [&'static str],
}

/// The customer-agreement set this emulator serves for its one account.
///
/// The API has no operation that creates, accepts or terminates a customer-agreement, so
/// this set is the account's established agreements rather than anything a caller made:
/// five records spanning every `AgreementType` and every `CustomerAgreementState`, so the
/// state rule `ListCustomerAgreements` applies is observable. Every value satisfies its
/// member's modelled constraints — `id` matches `CustomerAgreementIdAttribute`'s
/// `^customer-agreement-[a-zA-Z0-9]{16}$`, and each name, description and term matches
/// `LongStringAttribute`'s `^[^<>]*$` within 1..2048 bytes.
const AGREEMENT_SEED: &[SeedAgreement] = &[
    SeedAgreement {
        id: "customer-agreement-baaagreement0001",
        name: "AWS Business Associate Addendum",
        agreement_id: "AWS-BAA",
        description: "HIPAA Business Associate Addendum to the AWS Customer Agreement",
        agreement_type: AGREEMENT_TYPE_DEFAULT,
        state: AGREEMENT_STATE_ACTIVE,
        effective_start: T2024JAN,
        effective_end: None,
        acceptance_terms: &[
            "Acceptance by an authorized representative of the account",
            "Designation of the AWS services used to process protected health information",
        ],
        terminate_terms: &["Written notice of termination by an authorized representative"],
    },
    SeedAgreement {
        id: "customer-agreement-cjisagreement001",
        name: "AWS CJIS Security Addendum",
        agreement_id: "AWS-CJIS",
        description: "Criminal Justice Information Services Security Addendum",
        agreement_type: AGREEMENT_TYPE_DEFAULT,
        state: AGREEMENT_STATE_CUSTOMER_TERMINATED,
        effective_start: T2023JAN,
        effective_end: Some(T2024JUL),
        acceptance_terms: &["Acceptance by an authorized representative of the account"],
        terminate_terms: &["Written notice of termination by an authorized representative"],
    },
    SeedAgreement {
        id: "customer-agreement-gdprsccagreement",
        name: "AWS GDPR Data Processing Addendum",
        agreement_id: "AWS-GDPR-DPA",
        description: "Data Processing Addendum with the EU standard contractual clauses",
        agreement_type: AGREEMENT_TYPE_MODIFIED,
        state: AGREEMENT_STATE_ACTIVE,
        effective_start: T2024JUL,
        effective_end: None,
        acceptance_terms: &[
            "Acceptance by an authorized representative of the account",
            "Identification of the data exporter and the data importer",
            "Selection of the standard contractual clauses module that applies",
        ],
        terminate_terms: &[
            "Written notice of termination by an authorized representative",
            "Completion of the deletion or return of personal data",
        ],
    },
    SeedAgreement {
        id: "customer-agreement-ndaagreement0001",
        name: "AWS Nondisclosure Agreement",
        agreement_id: "AWS-NDA",
        description: "Mutual nondisclosure agreement negotiated for this customer",
        agreement_type: AGREEMENT_TYPE_CUSTOM,
        state: AGREEMENT_STATE_ACTIVE,
        effective_start: T2025JAN,
        effective_end: None,
        acceptance_terms: &["Signature by an authorized representative of the account"],
        terminate_terms: &["Expiry of the confidentiality period"],
    },
    SeedAgreement {
        id: "customer-agreement-pcidssagreement1",
        name: "AWS PCI DSS Merchant Addendum",
        agreement_id: "AWS-PCI-DSS",
        description: "Payment Card Industry Data Security Standard merchant addendum",
        agreement_type: AGREEMENT_TYPE_MODIFIED,
        state: AGREEMENT_STATE_AWS_TERMINATED,
        effective_start: T2023JUL,
        effective_end: Some(T2025JAN),
        acceptance_terms: &["Acceptance by an authorized representative of the account"],
        terminate_terms: &["Written notice of termination by AWS"],
    },
];

/// Build the customer-agreement set from [`AGREEMENT_SEED`]. Deterministic: the same
/// records, in the same order, every time the state is constructed.
fn seed_customer_agreements() -> BTreeMap<String, CustomerAgreement> {
    AGREEMENT_SEED
        .iter()
        .map(|seed| {
            let agreement = CustomerAgreement {
                id: seed.id.to_string(),
                name: seed.name.to_string(),
                description: seed.description.to_string(),
                agreement_id: seed.agreement_id.to_string(),
                agreement_type: seed.agreement_type.to_string(),
                state: seed.state.to_string(),
                effective_start: at(seed.effective_start),
                effective_end: seed.effective_end.map(at),
                acceptance_terms: seed
                    .acceptance_terms
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect(),
                terminate_terms: seed
                    .terminate_terms
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect(),
            };
            (agreement.id.clone(), agreement)
        })
        .collect()
}
