// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mailmanager-mailmanagersvc service.
//!
//! Timestamps are stored as epoch MILLISECONDS (`i64`) and rendered into the modelled
//! `Timestamp` members with [`::aws_smithy_types::DateTime::from_millis`] on the way
//! out. Keeping an integer in the store (rather than a `DateTime`) is what lets
//! [`bump_millis`] guarantee that an update is *observably* later than the create that
//! preceded it even when both land inside one clock tick.

/// Whether the customer managed KMS key a `CreateArchive` names is one this service may
/// encrypt an archive's mail under — the question KMS, which owns keys, answers.
pub mod archive_encryption_key;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ArchiveBooleanToEvaluate, ArchiveFilterCondition, ArchiveFilters, ArchiveRetention,
    ArchiveStringToEvaluate, ExportStatus, PolicyStatement, Row, Rule, SearchStatus, Tag,
};

/// The two `ArchiveState` values this service's model declares; see
/// `crate::ops::types::ARCHIVE_STATE_VALUES`, which is generated from the model's
/// `ArchiveState` enum.
pub const ARCHIVE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ARCHIVE_STATE_ACTIVE`]. The model documents it as "The archive has been marked
/// for deletion and will be permanently deleted in 30 days. No further modifications can
/// be made in this state."
pub const ARCHIVE_STATE_PENDING_DELETION: &str = "PENDING_DELETION";

/// How long an archive stays in `PENDING_DELETION` before it is permanently removed.
///
/// This duration is DOCUMENTED, not chosen: the model's `DeleteArchive` documentation
/// states "The archive and all of its contents will be permanently deleted 30 days after
/// entering the pending deletion state, regardless of the configured retention period",
/// and `Archive$ArchiveState` repeats "will be permanently deleted in 30 days".
pub const PENDING_DELETION_WINDOW_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The prefix every `ArchiveId` this service mints carries.
///
/// The identifier an archive is created as is modelled by TWO shapes: `ArchiveIdString`
/// (`@length 1..66`, no pattern), which `CreateArchive` returns it through, and the
/// tighter `ArchiveId` (`@length 3..66`, `@pattern ^a-[\w]{1,64}$`), which every
/// archive-scoped job operation accepts it through. The constrained one is authoritative
/// about the lexical form of the identifier space, so a minted id carries this prefix and
/// a body of word characters: the value handed out is then accepted by every operation
/// that takes it back.
pub const ARCHIVE_ID_PREFIX: &str = "a-";

/// The `RetentionPeriod` recorded for an archive created without a `Retention`.
///
/// The model declares no `@default` for `CreateArchiveRequest$Retention` yet makes
/// `GetArchiveResponse$Retention` **required**, so a record must carry one. `PERMANENT`
/// is the one `RetentionPeriod` value (see `crate::ops::types::RETENTION_PERIOD_VALUES`)
/// under which no mail is ever automatically deleted, so it is the only default that
/// cannot discard data the caller never asked to have discarded.
pub const DEFAULT_RETENTION_PERIOD: &str = "PERMANENT";

/// The region an ARN carries when the request names none; `::engine::emulator_arn`
/// documents `us-east-1` as the emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most address lists this emulator keeps, i.e. its per-account address-list quota.
///
/// `CreateAddressList` models `ServiceQuotaExceededException` ("Occurs when an operation
/// exceeds a predefined service quota or limit") precisely because the real service caps
/// the resource. The model does not state the number, so this value is CHOSEN on the same
/// reasoning as [`MAX_ARCHIVES`]: large enough that ordinary use never reaches it, small
/// enough that a caller cannot grow the process's memory without limit. It bounds the
/// whole address-list side of the store: `address_list_names` holds one entry per live
/// address list, and `address_list_idempotency` is only written on a create that
/// succeeded.
pub const MAX_ADDRESS_LISTS: usize = 100;

/// The most archives this emulator keeps, i.e. its per-account archive quota.
///
/// `CreateArchive` is the only operation that grows this service's store, and it models
/// `ServiceQuotaExceededException` ("Occurs when an operation exceeds a predefined service
/// quota or limit") precisely because the real service caps the resource. The model does
/// not state the number, so this value is CHOSEN: it is large enough that ordinary use
/// never reaches it and small enough that a caller cannot grow the process's memory
/// without limit. It bounds the whole store, not just `archives`: `archive_names` holds
/// one entry per live archive, and `idempotency` is only written on a create that
/// succeeded, so neither can exceed this count, and `purge_permanently_deleted`'s walk is
/// bounded by it too.
pub const MAX_ARCHIVES: usize = 100;

/// One stored email archive. The single source of truth for every archive operation:
/// `CreateArchive` writes it, `GetArchive`/`ListArchives` read it back, and
/// `UpdateArchive`/`DeleteArchive` mutate it in place.
#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    pub archive_id: String,
    pub archive_name: String,
    pub archive_arn: String,
    /// `ARCHIVE_STATE_ACTIVE` or `ARCHIVE_STATE_PENDING_DELETION`.
    pub archive_state: String,
    pub retention: ArchiveRetention,
    pub kms_key_arn: Option<String>,
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub tags: Vec<Tag>,
    /// When the archive becomes permanently deleted, set by `DeleteArchive` to
    /// [`PENDING_DELETION_WINDOW_MILLIS`] after the call. `None` while the archive is
    /// `ACTIVE`.
    pub pending_deletion_at_millis: Option<i64>,
}

/// How an archive-scoped job operation (an archive search, an archive export) words the
/// refusal of an `ArchiveId` that resolves to no stored archive.
///
/// The text is entirely fixed and echoes no id back — a job operation states that the
/// archive it was asked about is not there, not which value it was handed. Carried by the
/// modelled `ResourceNotFoundException` ("Occurs when a requested resource is not found")
/// at [`ARCHIVE_JOB_LOOKUP_STATUS`].
pub const ARCHIVE_NOT_FOUND: &str = "Archive not found.";

/// The HTTP status an archive-scoped job operation answers an `ArchiveId` it cannot resolve
/// with.
///
/// These operations classify an archive that is not there as a fault in the REQUEST, so the
/// status line is the one a caller-input failure carries even though the error shape it
/// carries is modelled `@httpError 404`: the error code names the condition, the status says
/// whose fault it is — the same reading this service applies to an address-list lookup (see
/// [`ADDRESS_LIST_LOOKUP_STATUS`]). Returned through
/// `crate::scaffolded::Scaffolded::modeled_with_status`, which keeps the modelled shape, its
/// `Message` member and its wire error code and changes only the status.
pub const ARCHIVE_JOB_LOOKUP_STATUS: u16 = 400;

/// How an archive RESOURCE operation (`GetArchive`, `UpdateArchive`, `DeleteArchive`)
/// words the refusal of an `ArchiveId` that resolves to no stored archive.
///
/// The fixed text is "Archive " followed by " not found." and the one variable part is the
/// id the request named, copied verbatim — the same answer whichever of the three did the
/// resolving. A job operation states the condition without the id instead; see
/// [`ARCHIVE_NOT_FOUND`].
pub fn archive_not_found_message(archive_id: &str) -> String {
    format!("Archive {archive_id} not found.")
}

/// How an archive-scoped job operation words the refusal of an archive that is stored but
/// no longer [`ARCHIVE_STATE_ACTIVE`].
///
/// The text is entirely fixed and names no id: an archive in `PENDING_DELETION` is one
/// "marked for deletion ... No further modifications can be made in this state" (the
/// model's `Archive$ArchiveState`), so it neither starts new jobs nor reports the jobs it
/// held. Carried by the modelled `ValidationException`, which is `@httpError 400`.
pub const ARCHIVE_NOT_ACTIVE: &str = "Archive is not active.";

/// Why an archive-scoped job operation (an archive search or an archive export) refuses
/// the `ArchiveId` it was handed; see
/// [`ComAmazonawsMailmanagerMailmanagersvcState::check_archive_for_job`]. Each operation
/// models its own error enum, so the resolution is shared here and the wording of the
/// refusal is built by the handler out of [`archive_not_found_message`] and
/// [`ARCHIVE_NOT_ACTIVE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveJobRefusal {
    /// No archive of that id is stored.
    NotFound,
    /// An archive of that id is stored, but it is not [`ARCHIVE_STATE_ACTIVE`].
    NotActive,
}

/// One stored address list: "a list of emails and domains that are used in MailManager
/// Ingress endpoints and Rules for email management" (the model's `AddressList` doc).
///
/// The single source of truth for every address-list operation: `CreateAddressList` writes
/// it and `GetAddressList` reads it back, and the member operations
/// (`RegisterMemberToAddressList`, `GetMemberOfAddressList`, `ListMembersOfAddressList`)
/// read and write [`Self::members`]. The model declares no status member for an address
/// list, so a created one is immediately usable — there is no transition for the clock to
/// owe.
#[derive(Debug, Clone)]
pub struct AddressListRecord {
    pub address_list_id: String,
    pub address_list_name: String,
    pub address_list_arn: String,
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub tags: Vec<Tag>,
    /// The addresses that are members of this list: each stored address
    /// ([`member_address_key`]) against the instant it was added, which is the
    /// `CreatedTimestamp` a member read reports.
    ///
    /// A `BTreeMap`, so one address is one member however often it is registered,
    /// `ListMembersOfAddressList` walks the members in one stable order, and a continuation
    /// token can name the address to resume at. Bounded by [`MAX_ADDRESS_LIST_MEMBERS`].
    /// The membership lives INSIDE the list record, so deleting the list takes it along.
    pub members: BTreeMap<String, i64>,
}

/// The most members this emulator keeps in one address list, i.e. its per-list membership
/// quota.
///
/// `RegisterMemberToAddressList` models `ServiceQuotaExceededException` ("Occurs when an
/// operation exceeds a predefined service quota or limit") precisely because the real
/// service caps a list's membership. The model does not state the number, so this value is
/// CHOSEN on the same reasoning as [`MAX_ADDRESS_LISTS`]: an `Address` is at most 320
/// bytes, so [`MAX_ADDRESS_LISTS`] lists of this many members bound the addresses this
/// process holds to tens of megabytes, while leaving ordinary use far from the ceiling.
pub const MAX_ADDRESS_LIST_MEMBERS: usize = 1_000;

/// The key an address-list member is stored and looked up under: the address with its
/// DOMAIN folded to lower case and its local part left exactly as the caller spelled it.
///
/// A domain is a DNS name, which is compared without regard to case, while the local part
/// of an address is not — so `user@Example.COM` and `user@example.com` name ONE member,
/// while `User@example.com` and `user@example.com` name two. A value that is a bare domain
/// has no local part at all, so it folds whole.
///
/// The reading is the workspace's one (`::service_common::ses_identities::identity_key`),
/// which the SES surfaces key their email identities by: these are the same mail addresses
/// under the same rule, so there is one answer here to what folds and what does not.
pub fn member_address_key(address: &str) -> String {
    ::service_common::ses_identities::identity_key(address)
}

/// Whether `address` is a value an address list can hold.
///
/// `SavedAddress$Address` is "The email or domain that constitutes the address", so either
/// an email address or a bare domain name; the form of each is the workspace's one reading
/// (`::service_common::email_address`). `Address` is modelled only as a 3..320 string, so
/// the boundary validator admits any text of that length and reading the form is the
/// handler's job: a value that is neither names no mailbox and no domain.
pub fn is_registrable_address(address: &str) -> bool {
    ::service_common::email_address::is_email_address(address)
        || ::service_common::email_address::is_domain_name(address)
}

/// What a member operation reports for an `Address` that is neither an email address nor a
/// domain name ([`is_registrable_address`]).
///
/// `Address` is `@sensitive`, so the refusal names the member and the requirement and
/// states no value — the same half a boundary refusal drops for such a member.
pub const ADDRESS_NOT_AN_ADDRESS: &str = "Address must be an email address or a domain name.";

/// How every address-list operation words the refusal of an `AddressListId` that resolves
/// to no stored address list.
///
/// The fixed text is "Could not find AddressList with id " and the one variable part is the
/// id the request named, copied verbatim — the same answer whichever address-list operation
/// did the resolving. See [`ADDRESS_LIST_LOOKUP_STATUS`] for the status it is answered
/// with.
pub fn address_list_not_found_message(address_list_id: &str) -> String {
    format!("Could not find AddressList with id {address_list_id}")
}

/// The HTTP status this service answers a request naming an address-list resource it cannot
/// resolve with — an `AddressListId` that names no stored list, and equally an `Address`
/// that names no member of a list that does resolve.
///
/// This service classifies a lookup that resolves to nothing as a fault in the REQUEST, so
/// the status line is the one a caller-input failure carries even where the error shape it
/// carries is modelled `@httpError 404`: the error code names the condition, the status says
/// whose fault it is. Returned through `crate::scaffolded::Scaffolded::modeled_with_status`,
/// which keeps the modelled shape, its `Message` member and its wire error code and changes
/// only the status.
pub const ADDRESS_LIST_LOOKUP_STATUS: u16 = 400;

/// How a member read words the refusal of an `Address` that is no member of an address list
/// that DOES resolve.
///
/// The fixed text is "Could not find Address in AddressList " followed by " of AWS account ",
/// and the two variable parts are the list id the request named, copied verbatim, and the
/// account that OWNS that list — the one account this emulator presents
/// ([`::engine::emulator_account_id`]), which is also the account segment of every
/// [`address_list_arn`] it mints, so the clause is read off the emulator's own identity
/// rather than written as a literal. `Address` is `@sensitive`, so the refusal states no
/// address at all: neither the value it was handed nor the key that value folds to. See
/// [`ADDRESS_LIST_LOOKUP_STATUS`] for the status it is answered with — a member that is not
/// there is an unresolvable address-list resource like any other.
pub fn member_of_address_list_not_found_message(address_list_id: &str) -> String {
    format!(
        "Could not find Address in AddressList {address_list_id} of AWS account {}",
        ::engine::emulator_account_id()
    )
}

/// How every address-list import-job operation words the refusal of a `JobId` that
/// resolves to no stored import job.
///
/// The text is entirely fixed: it states that the `JobId` member is not acceptable and
/// echoes no id back, which is why this is the modelled `ValidationException` (already
/// `@httpError 400`) rather than the address-list answer above. The one wording every
/// validation refusal of an unresolvable identifier in this service carries — "Invalid "
/// followed by the input member's name (see [`SEARCH_ID_INVALID`]).
pub const IMPORT_JOB_ID_INVALID: &str = "Invalid JobId";

/// The `ImportJobStatus` a created address-list import job carries.
///
/// `CreateAddressListImportJob` creates the job and hands back the URL its input file is
/// uploaded to, so nothing has been imported when the call returns; `CREATED` is the one
/// `ImportJobStatus` (see `crate::ops::types::IMPORT_JOB_STATUS_VALUES`, generated from
/// the model's enum) that says exactly that, and the model's `ImportJob$Status` leaves
/// `StartTimestamp`, `CompletedTimestamp` and both item counts optional precisely because
/// a job in it has none of them yet.
pub const IMPORT_JOB_STATUS_CREATED: &str = "CREATED";

/// The most address-list import jobs this emulator keeps.
///
/// A resource guard this emulator OWNS, not an AWS-published quota:
/// `CreateAddressListImportJob` models no quota error at all, so a create past this
/// ceiling is refused with the modelled `ValidationException`. It bounds both the job
/// records and the input files uploaded against them, so the store holds at most this
/// many times [`MAX_IMPORT_FILE_BYTES`] of caller-supplied bytes.
pub const MAX_IMPORT_JOBS: usize = 100;

/// The most bytes one import job's uploaded input file is retained as.
///
/// The bytes live in this process's heap for as long as the job record does, so the upload
/// plane bounds them: a larger body is refused there rather than stored. A resource guard
/// this emulator owns, not an AWS-published limit; an import file is a list of addresses,
/// so this holds tens of thousands of them.
pub const MAX_IMPORT_FILE_BYTES: usize = 1024 * 1024;

/// The input file that was uploaded to one import job's pre-signed URL: the bytes, and
/// when they arrived.
#[derive(Debug, Clone)]
pub struct UploadedImportFile {
    pub content: Vec<u8>,
    pub uploaded_at_millis: i64,
}

/// One stored address-list import job: "an import job for an address list" (the model's
/// `CreateAddressListImportJob` doc).
///
/// The single source of truth for every import-job operation:
/// `CreateAddressListImportJob` writes it and `GetAddressListImportJob` reads it back. The
/// members the model leaves optional are the ones only a RUNNING import fills
/// ([`Self::started_at_millis`] and below), so a created job carries none of them.
#[derive(Debug, Clone)]
pub struct ImportJobRecord {
    pub job_id: String,
    pub address_list_id: String,
    pub name: String,
    /// One of `crate::ops::types::IMPORT_JOB_STATUS_VALUES`;
    /// [`IMPORT_JOB_STATUS_CREATED`] until an import runs.
    pub status: String,
    /// The `ImportDataFormat$ImportDataType` the job was created for, one of
    /// `crate::ops::types::IMPORT_DATA_TYPE_VALUES`.
    pub import_data_type: String,
    /// The URL the input file is uploaded to, minted once when the job was created: a
    /// caller that reads the job back gets the same target it was first handed.
    pub pre_signed_url: String,
    pub created_at_millis: i64,
    /// What the upload plane received at [`Self::pre_signed_url`], if anything yet.
    pub uploaded_file: Option<UploadedImportFile>,
    pub started_at_millis: Option<i64>,
    pub completed_at_millis: Option<i64>,
    pub imported_items_count: Option<i32>,
    pub failed_items_count: Option<i32>,
    pub error: Option<String>,
}

/// The `SearchState` values an archive search job of this emulator reports; see
/// `crate::ops::types::SEARCH_STATE_VALUES`, generated from the model's `SearchState`
/// enum.
///
/// A job starts `QUEUED`: `StartArchiveSearch` only SUBMITS the search — its output is a
/// `SearchId` and nothing else — and the model's `SearchStatus$CompletionTimestamp` is
/// "the timestamp of when the search completed (if finished)", so a job that has just been
/// submitted is not finished.
pub const SEARCH_STATE_QUEUED: &str = "QUEUED";
/// See [`SEARCH_STATE_QUEUED`]: the state a search job reaches once its completion is due.
pub const SEARCH_STATE_COMPLETED: &str = "COMPLETED";

/// The `ExportState` values an archive export job of this emulator reports; see
/// `crate::ops::types::EXPORT_STATE_VALUES`, generated from the model's `ExportState`
/// enum. The reasoning is [`SEARCH_STATE_QUEUED`]'s: a submitted job is not a finished
/// one.
pub const EXPORT_STATE_QUEUED: &str = "QUEUED";
/// See [`EXPORT_STATE_QUEUED`]: the state an export job reaches once its completion is
/// due.
pub const EXPORT_STATE_COMPLETED: &str = "COMPLETED";

/// How long after it is submitted an archive job (a search, an export) completes.
///
/// CHOSEN, not documented: the model publishes no duration for either job, and the
/// non-terminal `QUEUED` state together with the optional `CompletionTimestamp` is what
/// says a submitted job is still running. The span is never zero, so a caller that reads
/// the job straight back still observes the state a real client polls through, and it is
/// never slept for: the instant it falls due is stored on the record
/// ([`SearchJobRecord::completes_at_millis`]) and the state is read off the clock.
pub const ARCHIVE_JOB_COMPLETION_MILLIS: i64 = 3_000;

/// The most archive search jobs this emulator keeps, i.e. its per-account search-job
/// quota.
///
/// `StartArchiveSearch` models `ServiceQuotaExceededException` ("Occurs when an operation
/// exceeds a predefined service quota or limit") precisely because the real service caps
/// concurrent searches. The model does not state the number, so this value is CHOSEN on
/// the same reasoning as [`MAX_ARCHIVES`]: a search record is a handful of fields and its
/// filters, so this many bound the store to a trivial amount of memory while leaving
/// ordinary use far from the ceiling.
pub const MAX_ARCHIVE_SEARCHES: usize = 1_000;

/// One stored archive search job: "a search job for an archive" — what
/// `StartArchiveSearch` submits, `GetArchiveSearch` reads back and `ListArchiveSearches`
/// projects into a `SearchSummary`.
///
/// The two timestamp BOUNDS are kept as the modelled `Timestamp` the caller supplied
/// rather than as epoch milliseconds (what this module keeps every instant it mints
/// itself in): they are echoed back verbatim by `GetArchiveSearch`, so storing them as
/// handed over is what keeps the value a caller reads identical to the one it sent.
#[derive(Debug, Clone)]
pub struct SearchJobRecord {
    pub search_id: String,
    /// The archive this job searches. The job is bound to it: the archive going away takes
    /// its searches with it (see
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::purge_permanently_deleted`]).
    pub archive_id: String,
    pub filters: Option<ArchiveFilters>,
    pub from_timestamp: ::aws_smithy_types::DateTime,
    pub to_timestamp: ::aws_smithy_types::DateTime,
    pub max_results: i32,
    pub submitted_at_millis: i64,
    /// When this job's completion falls due: [`ARCHIVE_JOB_COMPLETION_MILLIS`] after it
    /// was submitted.
    pub completes_at_millis: i64,
}

impl SearchJobRecord {
    /// This job's `SearchState` at `now_millis`.
    pub fn state(&self, now_millis: i64) -> &'static str {
        if now_millis >= self.completes_at_millis {
            SEARCH_STATE_COMPLETED
        } else {
            SEARCH_STATE_QUEUED
        }
    }

    /// This job's modelled `SearchStatus` at `now_millis`.
    ///
    /// `CompletionTimestamp` is the instant the completion fell due, not the instant it
    /// was first read: the job completed when the clock said so whether or not anybody
    /// was looking, so two reads of a finished job report the same timestamp.
    pub fn status(&self, now_millis: i64) -> SearchStatus {
        SearchStatus {
            submission_timestamp: Some(timestamp(self.submitted_at_millis)),
            completion_timestamp: (now_millis >= self.completes_at_millis)
                .then(|| timestamp(self.completes_at_millis)),
            state: Some(self.state(now_millis).to_string()),
            // "An error message if the search failed": nothing fails a search here, so no
            // job carries one.
            error_message: None,
        }
    }
}

/// One stored archive export job: what `StartArchiveExport` submits and
/// `ListArchiveExports` projects into an `ExportSummary` (`ExportId` and `Status`).
///
/// It holds what the identity and the status of a job are read off — the job's own id, the
/// archive it belongs to, and the two instants its state is computed from. The members
/// only an export's DESTINATION read needs (`GetArchiveExport` reports the filters, the
/// bounds and the `ExportDestinationConfiguration` too) belong to the operation that
/// writes them.
#[derive(Debug, Clone)]
pub struct ExportJobRecord {
    pub export_id: String,
    /// The archive this job exports from; bound to it exactly as
    /// [`SearchJobRecord::archive_id`] is.
    pub archive_id: String,
    pub submitted_at_millis: i64,
    /// When this job's completion falls due; see [`SearchJobRecord::completes_at_millis`].
    pub completes_at_millis: i64,
}

impl ExportJobRecord {
    /// This job's `ExportState` at `now_millis`.
    pub fn state(&self, now_millis: i64) -> &'static str {
        if now_millis >= self.completes_at_millis {
            EXPORT_STATE_COMPLETED
        } else {
            EXPORT_STATE_QUEUED
        }
    }

    /// This job's modelled `ExportStatus` at `now_millis`; see
    /// [`SearchJobRecord::status`].
    pub fn status(&self, now_millis: i64) -> ExportStatus {
        ExportStatus {
            submission_timestamp: Some(timestamp(self.submitted_at_millis)),
            completion_timestamp: (now_millis >= self.completes_at_millis)
                .then(|| timestamp(self.completes_at_millis)),
            state: Some(self.state(now_millis).to_string()),
            error_message: None,
        }
    }
}

/// The one `ArchiveStringOperator` value the model declares; see
/// `crate::ops::types::ARCHIVE_STRING_OPERATOR_VALUES`, generated from that enum.
pub const ARCHIVE_STRING_OPERATOR_CONTAINS: &str = "CONTAINS";

/// One of the six `ArchiveStringEmailAttribute` values the model declares; see
/// `crate::ops::types::ARCHIVE_STRING_EMAIL_ATTRIBUTE_VALUES`, generated from that enum.
/// The six of them are what a string filter condition can be evaluated over.
pub const ARCHIVE_STRING_ATTRIBUTE_TO: &str = "TO";
/// See [`ARCHIVE_STRING_ATTRIBUTE_TO`].
pub const ARCHIVE_STRING_ATTRIBUTE_FROM: &str = "FROM";
/// See [`ARCHIVE_STRING_ATTRIBUTE_TO`].
pub const ARCHIVE_STRING_ATTRIBUTE_CC: &str = "CC";
/// See [`ARCHIVE_STRING_ATTRIBUTE_TO`].
pub const ARCHIVE_STRING_ATTRIBUTE_SUBJECT: &str = "SUBJECT";
/// See [`ARCHIVE_STRING_ATTRIBUTE_TO`]: read off the SMTP envelope rather than a header.
pub const ARCHIVE_STRING_ATTRIBUTE_ENVELOPE_FROM: &str = "ENVELOPE_FROM";
/// See [`ARCHIVE_STRING_ATTRIBUTE_ENVELOPE_FROM`].
pub const ARCHIVE_STRING_ATTRIBUTE_ENVELOPE_TO: &str = "ENVELOPE_TO";

/// The one `ArchiveBooleanEmailAttribute` value the model declares; see
/// `crate::ops::types::ARCHIVE_BOOLEAN_EMAIL_ATTRIBUTE_VALUES`.
pub const ARCHIVE_BOOLEAN_ATTRIBUTE_HAS_ATTACHMENTS: &str = "HAS_ATTACHMENTS";

/// One of the two `ArchiveBooleanOperator` values; see
/// `crate::ops::types::ARCHIVE_BOOLEAN_OPERATOR_VALUES`.
pub const ARCHIVE_BOOLEAN_OPERATOR_IS_TRUE: &str = "IS_TRUE";
/// See [`ARCHIVE_BOOLEAN_OPERATOR_IS_TRUE`]: the other `ArchiveBooleanOperator` value.
pub const ARCHIVE_BOOLEAN_OPERATOR_IS_FALSE: &str = "IS_FALSE";

/// One stored archived email message: what an archive holds, and what an archive search
/// over that archive reports one `Row` of.
///
/// The two values a search RESOLVES a message by — the archive it was archived into and
/// the instant it was received — are held beside the metadata rather than read back out of
/// it, so neither the range match nor the ordering depends on a rendered timestamp. The
/// rest of the modelled `Row` is carried exactly as it was archived.
#[derive(Debug, Clone)]
pub struct ArchivedMessageRecord {
    pub archived_message_id: String,
    /// The archive this message lives in. The message is bound to it exactly as a job is
    /// (see [`SearchJobRecord::archive_id`]): the archive going away takes its messages
    /// with it.
    pub archive_id: String,
    /// When this message was received, in epoch milliseconds: what `Row$ReceivedTimestamp`
    /// reports, and what a search's `FromTimestamp`..`ToTimestamp` range is matched
    /// against.
    pub received_at_millis: i64,
    /// The message's remaining metadata, as the search reports it. `ArchivedMessageId` and
    /// `ReceivedTimestamp` are filled from the two members above on the way out, so the
    /// identity and the instant a reader sees are the ones this record is keyed and
    /// ordered by.
    pub metadata: Row,
}

impl ArchivedMessageRecord {
    /// This message as the modelled `Row` an archive search reports.
    pub fn row(&self) -> Row {
        Row {
            archived_message_id: Some(self.archived_message_id.clone()),
            received_timestamp: Some(timestamp(self.received_at_millis)),
            ..self.metadata.clone()
        }
    }

    /// Whether `filters` admit this message.
    ///
    /// `ArchiveFilters` carries "the filter conditions for emails to include" and "the
    /// filter conditions for emails to exclude", so a message is admitted when EVERY
    /// `Include` condition holds for it and NO `Unless` condition does. Filters a search
    /// was started without admit every message of the archive.
    pub fn admitted_by(&self, filters: Option<&ArchiveFilters>) -> bool {
        let Some(filters) = filters else {
            return true;
        };
        let included = filters
            .include
            .as_deref()
            .unwrap_or_default()
            .iter()
            .all(|condition| self.matches(condition));
        let excluded = filters
            .unless
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|condition| self.matches(condition));
        included && !excluded
    }

    /// Whether one filter condition holds for this message.
    ///
    /// A string condition holds when one of the attribute's values CONTAINS one of the
    /// strings the condition lists, compared without regard to case: an address's domain is
    /// a DNS name, which is case-insensitive, and a header's text is matched the way a mail
    /// client searches it — the same case-folding this store applies to an address-list
    /// member (see [`member_address_key`]). A boolean condition holds when the attribute's
    /// flag has the asserted value. An operator or attribute outside what the model declares
    /// holds for no message, so a condition this service cannot evaluate admits nothing
    /// rather than everything.
    fn matches(&self, condition: &ArchiveFilterCondition) -> bool {
        match condition {
            ArchiveFilterCondition::StringExpression(expression) => {
                if expression.operator != ARCHIVE_STRING_OPERATOR_CONTAINS {
                    return false;
                }
                let ArchiveStringToEvaluate::Attribute(attribute) = &expression.evaluate;
                let evaluated = self.string_values(attribute);
                expression.values.iter().any(|wanted| {
                    let wanted = wanted.to_lowercase();
                    evaluated
                        .iter()
                        .any(|value| value.to_lowercase().contains(&wanted))
                })
            }
            ArchiveFilterCondition::BooleanExpression(expression) => {
                let ArchiveBooleanToEvaluate::Attribute(attribute) = &expression.evaluate;
                if attribute != ARCHIVE_BOOLEAN_ATTRIBUTE_HAS_ATTACHMENTS {
                    return false;
                }
                // "A flag indicating if the email has attachments": a message archived
                // without the flag carries none.
                let has_attachments = self.metadata.has_attachments.unwrap_or(false);
                match expression.operator.as_str() {
                    ARCHIVE_BOOLEAN_OPERATOR_IS_TRUE => has_attachments,
                    ARCHIVE_BOOLEAN_OPERATOR_IS_FALSE => !has_attachments,
                    _ => false,
                }
            }
        }
    }

    /// The values a string condition over `attribute` evaluates, where `attribute` is one of
    /// `crate::ops::types::ARCHIVE_STRING_EMAIL_ATTRIBUTE_VALUES` (generated from the
    /// model's `ArchiveStringEmailAttribute` enum).
    ///
    /// The envelope attributes read the SMTP envelope rather than the headers, and the
    /// envelope carries every recipient, so `ENVELOPE_TO` evaluates each of them. An
    /// attribute this message does not carry evaluates nothing, so no condition over it
    /// holds.
    fn string_values(&self, attribute: &str) -> Vec<&str> {
        let envelope = self.metadata.envelope.as_ref();
        let header = match attribute {
            ARCHIVE_STRING_ATTRIBUTE_TO => self.metadata.to.as_deref(),
            ARCHIVE_STRING_ATTRIBUTE_FROM => self.metadata.from.as_deref(),
            ARCHIVE_STRING_ATTRIBUTE_CC => self.metadata.cc.as_deref(),
            ARCHIVE_STRING_ATTRIBUTE_SUBJECT => self.metadata.subject.as_deref(),
            ARCHIVE_STRING_ATTRIBUTE_ENVELOPE_FROM => {
                envelope.and_then(|envelope| envelope.from.as_deref())
            }
            ARCHIVE_STRING_ATTRIBUTE_ENVELOPE_TO => {
                return envelope
                    .and_then(|envelope| envelope.to.as_deref())
                    .unwrap_or_default()
                    .iter()
                    .map(String::as_str)
                    .collect()
            }
            _ => None,
        };
        header.into_iter().collect()
    }
}

/// How `GetArchiveSearchResults` words the refusal of a search job that has not completed.
///
/// The operation "returns the results of a COMPLETED email archive search job", so a job
/// still on its way to [`SEARCH_STATE_COMPLETED`] has no results to report and the request
/// conflicts with the state the job is in — the modelled `ConflictException` ("The request
/// configuration has conflicts", `@httpError 409`). The text is entirely fixed and echoes
/// no id back.
pub const SEARCH_NOT_COMPLETED: &str = "Search is not completed yet.";

/// How the search reads word the refusal of a `SearchId` that resolves to no search job
/// this store serves — one that was never minted, and one whose archive is gone or no
/// longer [`ARCHIVE_STATE_ACTIVE`].
///
/// The search reads model no `ResourceNotFoundException` at all, so a `SearchId` that names
/// nothing is a fault in the REQUEST, carried by the modelled `ValidationException`
/// (already `@httpError 400`); a validation refusal of an identifier states that the input
/// MEMBER is unacceptable, so the text is "Invalid " followed by the member's name as the
/// model spells it — entirely fixed, echoing neither the id it was handed nor anything from
/// the store. The same wording the import-job read's `JobId` carries
/// ([`IMPORT_JOB_ID_INVALID`]).
pub const SEARCH_ID_INVALID: &str = "Invalid SearchId";

/// The most rule sets this emulator keeps, i.e. its per-account rule-set quota.
///
/// `CreateRuleSet` models `ServiceQuotaExceededException` ("Occurs when an operation exceeds
/// a predefined service quota or limit") precisely because the real service caps the
/// resource. The model does not state the number, so this value is CHOSEN on the same
/// reasoning as [`MAX_ARCHIVES`]: large enough that ordinary use never reaches it, small
/// enough that a caller cannot grow the process's memory without limit. It bounds the whole
/// rule-set side of the store: `rule_sets` holds one entry per live rule set, and
/// `rule_set_idempotency` is only written on a create that succeeded.
pub const MAX_RULE_SETS: usize = 100;

/// One stored rule set: "a list of rules that are evaluated in order. Each rule is evaluated
/// sequentially for each email" (the model's `RuleSet` doc).
///
/// The single source of truth for every rule-set operation: `CreateRuleSet` writes it and
/// `GetRuleSet` reads it back. The model declares no status member for a rule set, so a
/// created one is immediately usable — there is no transition for the clock to owe.
///
/// [`Self::rules`] holds the modelled `Rule` values exactly as they were submitted, in the
/// submitted ORDER, because the rules of a rule set are "evaluated in order": a read that
/// reported them in any other order would describe a different rule set.
#[derive(Debug, Clone)]
pub struct RuleSetRecord {
    pub rule_set_id: String,
    pub rule_set_name: String,
    pub rule_set_arn: String,
    pub created_at_millis: i64,
    /// The instant the rule set was last written, which is its creation instant until an
    /// update rewrites it — what a rule-set read reports as `LastModificationDate`.
    pub last_modified_at_millis: i64,
    pub rules: Vec<Rule>,
    pub tags: Vec<Tag>,
}

/// How a rule-set operation words the refusal of a `RuleSetId` that resolves to no stored
/// rule set.
///
/// The fixed text is "Could not find RuleSet with id " and the one variable part is the id
/// the request named, copied verbatim — the shape this service's resource lookups state a
/// miss in (see [`address_list_not_found_message`]), carried by the modelled
/// `ResourceNotFoundException` ("Occurs when a requested resource is not found").
pub fn rule_set_not_found_message(rule_set_id: &str) -> String {
    format!("Could not find RuleSet with id {rule_set_id}")
}

/// The most traffic policies this emulator keeps, i.e. its per-account traffic-policy quota.
///
/// `CreateTrafficPolicy` models `ServiceQuotaExceededException` ("Occurs when an operation
/// exceeds a predefined service quota or limit") precisely because the real service caps the
/// resource. The model does not state the number, so this value is CHOSEN on the same
/// reasoning as [`MAX_ARCHIVES`]: large enough that ordinary use never reaches it, small
/// enough that a caller cannot grow the process's memory without limit. It bounds the whole
/// traffic-policy side of the store: `traffic_policies` holds one entry per live traffic
/// policy, and `traffic_policy_idempotency` is only written on a create that succeeded.
pub const MAX_TRAFFIC_POLICIES: usize = 100;

/// One stored traffic policy: "a container for policy statements" (the model's
/// `TrafficPolicy` doc), each statement filtering the email traffic that reaches an ingress
/// point.
///
/// The single source of truth for every traffic-policy operation: `CreateTrafficPolicy`
/// writes it and `GetTrafficPolicy` reads it back. The model declares no status member for a
/// traffic policy, so a created one is immediately usable — there is no transition for the
/// clock to owe.
///
/// [`Self::policy_statements`] holds the modelled `PolicyStatement` values exactly as they
/// were submitted, in the submitted ORDER: a statement matches or it does not, and the
/// order the statements are evaluated in is part of what was created, so a read reporting
/// them in another order would describe a different policy.
#[derive(Debug, Clone)]
pub struct TrafficPolicyRecord {
    pub traffic_policy_id: String,
    pub traffic_policy_name: String,
    pub traffic_policy_arn: String,
    pub policy_statements: Vec<PolicyStatement>,
    /// `DefaultAction`, one of `AcceptAction`'s values (`ALLOW` | `DENY`): what the policy
    /// does with a message no statement addresses. The model makes it **required** on the
    /// create, so a record always carries one.
    pub default_action: String,
    /// `MaxMessageSizeBytes`, which the model leaves optional and declares no `@default`
    /// for: a policy created without one has no size ceiling of its own, and the read
    /// reports the member as absent rather than inventing a limit.
    pub max_message_size_bytes: Option<i32>,
    pub created_at_millis: i64,
    /// The instant the traffic policy was last written, which is its creation instant until
    /// an update rewrites it — what a traffic-policy read reports as
    /// `LastUpdatedTimestamp`.
    pub last_updated_at_millis: i64,
    pub tags: Vec<Tag>,
}

/// How a traffic-policy operation words the refusal of a `TrafficPolicyId` that resolves to
/// no stored traffic policy.
///
/// The fixed text is "Could not find TrafficPolicy with id " and the one variable part is
/// the id the request named, copied verbatim — the shape this service's resource lookups
/// state a miss in (see [`address_list_not_found_message`] and
/// [`rule_set_not_found_message`]), carried by the modelled `ResourceNotFoundException`
/// ("Occurs when a requested resource is not found").
pub fn traffic_policy_not_found_message(traffic_policy_id: &str) -> String {
    format!("Could not find TrafficPolicy with id {traffic_policy_id}")
}

/// Per-service state for com-amazonaws-mailmanager-mailmanagersvc. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap`, so `ListArchives` walks the archives in one stable,
/// deterministic order and a pagination token can name the next key to resume from.
///
/// BOUNDED: `CreateArchive` refuses a create that would take `archives` past
/// [`MAX_ARCHIVES`], `CreateAddressList` one that would take `address_lists` past
/// [`MAX_ADDRESS_LISTS`] and `CreateAddressListImportJob` one that would take
/// `import_jobs` past [`MAX_IMPORT_JOBS`]; every other map carries at most one entry per
/// live archive, address list or import job, so no caller can grow this store without
/// limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsMailmanagerMailmanagersvcState {
    /// Archives by `ArchiveId`.
    pub archives: BTreeMap<String, ArchiveRecord>,
    /// `ArchiveName` -> `ArchiveId`; the index behind the name-uniqueness
    /// `ConflictException`.
    pub archive_names: BTreeMap<String, String>,
    /// `ClientToken` -> the create it completed; what makes a retried `CreateArchive`
    /// return the id the first call minted instead of creating a second archive.
    pub idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding [`ComAmazonawsMailmanagerMailmanagersvcState::mint_archive_id`];
    /// it is what makes a minted id unique even within one clock tick.
    pub archive_sequence: u64,
    /// Address lists by `AddressListId`.
    pub address_lists: BTreeMap<String, AddressListRecord>,
    /// `AddressListName` -> `AddressListId`; the index behind the name-uniqueness
    /// `ConflictException`.
    pub address_list_names: BTreeMap<String, String>,
    /// `ClientToken` -> the `CreateAddressList` it completed. Separate from
    /// [`Self::idempotency`] so a token recorded for one resource is never read as a retry
    /// of a create of the other.
    pub address_list_idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_address_list_id`]; see
    /// [`Self::archive_sequence`].
    pub address_list_sequence: u64,
    /// Address-list import jobs by `JobId`.
    pub import_jobs: BTreeMap<String, ImportJobRecord>,
    /// `ClientToken` -> the `CreateAddressListImportJob` it completed. Separate from the
    /// other two token maps so a token recorded for one resource is never read as a retry
    /// of a create of another.
    pub import_job_idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_import_job_id`]; see
    /// [`Self::archive_sequence`].
    pub import_job_sequence: u64,
    /// The secret the upload token of an import job's published locator is authenticated
    /// under; see [`import_job_upload_token`].
    ///
    /// Its own key, separate from [`Self::page_token_key`], so a value minted for one
    /// purpose is never read as the other.
    pub import_job_upload_key: PageTokenKey,
    /// Archive search jobs by `SearchId`, across every archive.
    ///
    /// A `BTreeMap`, so `ListArchiveSearches` walks one archive's jobs in one stable order
    /// and a continuation token can name the `SearchId` to resume at. Bounded by
    /// [`MAX_ARCHIVE_SEARCHES`].
    pub archive_searches: BTreeMap<String, SearchJobRecord>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_search_id`]; see
    /// [`Self::archive_sequence`].
    pub archive_search_sequence: u64,
    /// Archive export jobs by `ExportId`, across every archive — the jobs
    /// `ListArchiveExports` reports. Ordered and keyed exactly as [`Self::archive_searches`]
    /// is, and written by `StartArchiveExport`, which is still a scaffold: an archive on
    /// which no export has been started has none, which is a listing of zero jobs and not a
    /// failure.
    pub archive_exports: BTreeMap<String, ExportJobRecord>,
    /// Archived email messages by `ArchivedMessageId`, across every archive — what an
    /// archive search searches.
    ///
    /// Ordered and keyed exactly as [`Self::archive_searches`] is, and written by mail
    /// ARRIVING in an archive, which no operation this crate serves does yet: an archive
    /// nothing has been archived into holds none, which is a search that matches zero
    /// messages and not a failure. Bounded by the archives it belongs to: a message of a
    /// permanently deleted archive goes with it.
    pub archived_messages: BTreeMap<String, ArchivedMessageRecord>,
    /// The secret THIS store's pagination tokens are authenticated under; see
    /// [`encode_page_token`].
    pub page_token_key: PageTokenKey,
    /// Rule sets by `RuleSetId`.
    ///
    /// A `BTreeMap`, so `ListRuleSets` walks them in one stable, deterministic order and a
    /// continuation token can name the next key to resume from. Bounded by
    /// [`MAX_RULE_SETS`].
    pub rule_sets: BTreeMap<String, RuleSetRecord>,
    /// `ClientToken` -> the `CreateRuleSet` it completed. Separate from the other token maps
    /// so a token recorded for one resource is never read as a retry of a create of another.
    pub rule_set_idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_rule_set_id`]; see
    /// [`Self::archive_sequence`].
    pub rule_set_sequence: u64,
    /// Traffic policies by `TrafficPolicyId`.
    ///
    /// A `BTreeMap`, so `ListTrafficPolicies` walks them in one stable, deterministic order
    /// and a continuation token can name the next key to resume from. Bounded by
    /// [`MAX_TRAFFIC_POLICIES`].
    pub traffic_policies: BTreeMap<String, TrafficPolicyRecord>,
    /// `ClientToken` -> the `CreateTrafficPolicy` it completed. Separate from the other token
    /// maps so a token recorded for one resource is never read as a retry of a create of
    /// another.
    pub traffic_policy_idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_traffic_policy_id`]; see
    /// [`Self::archive_sequence`].
    pub traffic_policy_sequence: u64,
}

impl ComAmazonawsMailmanagerMailmanagersvcState {
    /// Materialise the removals the clock already owes: drop every archive whose
    /// pending-deletion window has elapsed, along with its name and idempotency index
    /// entries.
    ///
    /// Called by the reporting operations too. That is the one write a read is allowed:
    /// the archive was due to be gone at this instant whether or not anybody called, so
    /// applying it here cannot change what a later caller would otherwise have seen.
    pub fn purge_permanently_deleted(&mut self, now_millis: i64) {
        let expired: Vec<String> = self
            .archives
            .values()
            .filter(|record| {
                record
                    .pending_deletion_at_millis
                    .is_some_and(|due| due <= now_millis)
            })
            .map(|record| record.archive_id.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        for id in &expired {
            if let Some(record) = self.archives.remove(id) {
                // Only drop the name index entry if it still points at THIS archive: a
                // later archive may legitimately have taken the freed name.
                if self.archive_names.get(&record.archive_name) == Some(&record.archive_id) {
                    self.archive_names.remove(&record.archive_name);
                }
            }
        }
        let live: std::collections::BTreeSet<String> = self.archives.keys().cloned().collect();
        self.idempotency
            .retain(|_, create| live.contains(&create.resource_id));
        // A job belongs to its archive: once the archive is permanently gone there is
        // nothing left for a search or an export of it to report on.
        self.archive_searches
            .retain(|_, job| live.contains(&job.archive_id));
        self.archive_exports
            .retain(|_, job| live.contains(&job.archive_id));
        // "The archive and all of its contents will be permanently deleted": the mail the
        // archive held goes with it, so a search can never report a message of an archive
        // that is gone.
        self.archived_messages
            .retain(|_, message| live.contains(&message.archive_id));
    }

    /// Whether this store serves archive-scoped job operations (an archive search, an
    /// archive export) on `archive_id`, and why not when it does not.
    ///
    /// Shared by every operation that names an archive to start or report a job on, so the
    /// four of them resolve one `ArchiveId` the same way: an id that is stored and `ACTIVE`
    /// is served, and the two ways it is not are told apart for the caller.
    pub fn check_archive_for_job(&self, archive_id: &str) -> Result<(), ArchiveJobRefusal> {
        match self.archives.get(archive_id) {
            None => Err(ArchiveJobRefusal::NotFound),
            Some(record) if record.archive_state != ARCHIVE_STATE_ACTIVE => {
                Err(ArchiveJobRefusal::NotActive)
            }
            Some(_) => Ok(()),
        }
    }

    /// Mint a fresh `ArchiveId`: the [`ARCHIVE_ID_PREFIX`] and 32 lowercase hex characters
    /// (see [`mint_id`]).
    ///
    /// 34 characters of `a-` and word characters, which satisfy BOTH shapes the identifier
    /// travels as: `ArchiveIdString`'s modelled 1..66 length, which `CreateArchive` returns
    /// it through, and `ArchiveId`'s 3..66 length and `^a-[\w]{1,64}$` pattern, which every
    /// archive-scoped job operation takes it back through.
    pub fn mint_archive_id(&mut self, now_millis: i64) -> String {
        self.archive_sequence = self.archive_sequence.wrapping_add(1);
        format!(
            "{ARCHIVE_ID_PREFIX}{}",
            mint_id(self.archive_sequence, now_millis)
        )
    }

    /// Mint a fresh `AddressListId`, the same way [`Self::mint_archive_id`] mints an
    /// archive id: 32 lowercase hex characters, which satisfy `AddressListId`'s modelled
    /// 1..255 length and `^[a-zA-Z0-9-]+$` pattern.
    pub fn mint_address_list_id(&mut self, now_millis: i64) -> String {
        self.address_list_sequence = self.address_list_sequence.wrapping_add(1);
        mint_id(self.address_list_sequence, now_millis)
    }

    /// Mint a fresh import-job `JobId`, the same way [`Self::mint_archive_id`] mints an
    /// archive id: 32 lowercase hex characters, which satisfy `JobId`'s modelled 1..255
    /// length and `^[a-zA-Z0-9-]+$` pattern.
    pub fn mint_import_job_id(&mut self, now_millis: i64) -> String {
        self.import_job_sequence = self.import_job_sequence.wrapping_add(1);
        mint_id(self.import_job_sequence, now_millis)
    }

    /// The search job `search_id` names, if this store still serves it.
    ///
    /// Shared by the two search reads, so one `SearchId` resolves the same way for both: a
    /// job of an archive that is gone, or one no longer [`ARCHIVE_STATE_ACTIVE`], is no
    /// longer reachable and is reported exactly as one that was never minted (see
    /// [`SEARCH_ID_INVALID`]).
    pub fn resolve_search_job(&self, search_id: &str) -> Option<&SearchJobRecord> {
        self.archive_searches
            .get(search_id)
            .filter(|job| self.check_archive_for_job(&job.archive_id).is_ok())
    }

    /// The rows a COMPLETED archive search reports: the messages of the archive it searched
    /// whose received instant is inside the job's range and which the job's filters admit,
    /// most recently received first, capped at the job's `MaxResults`.
    ///
    /// Computed from the store on every read rather than frozen when the job completed:
    /// the two are the same answer, because a message is archived by mail arriving and the
    /// range a job searches is fixed when it is submitted.
    ///
    /// The ORDER is CHOSEN — the model publishes none — but it is total: two messages
    /// received in the same millisecond are ordered by their id, so one job reports one
    /// stable sequence of rows and the cap always drops the same messages.
    pub fn search_rows(&self, job: &SearchJobRecord) -> Vec<Row> {
        // A bound this emulator cannot represent as an instant cannot exclude anything:
        // such a range is read as open at that end.
        let from = job.from_timestamp.to_millis().unwrap_or(i64::MIN);
        let to = job.to_timestamp.to_millis().unwrap_or(i64::MAX);
        let mut matching: Vec<&ArchivedMessageRecord> = self
            .archived_messages
            .values()
            .filter(|message| {
                message.archive_id == job.archive_id
                    && (from..=to).contains(&message.received_at_millis)
                    && message.admitted_by(job.filters.as_ref())
            })
            .collect();
        matching.sort_by(|left, right| {
            right
                .received_at_millis
                .cmp(&left.received_at_millis)
                .then_with(|| left.archived_message_id.cmp(&right.archived_message_id))
        });
        matching
            .into_iter()
            .take(usize::try_from(job.max_results).unwrap_or(0))
            .map(ArchivedMessageRecord::row)
            .collect()
    }

    /// Mint a fresh `SearchId`, the same way [`Self::mint_archive_id`] mints an archive id:
    /// 32 lowercase hex characters, well inside `SearchId`'s modelled 1..64 length (the
    /// shape declares no pattern).
    pub fn mint_search_id(&mut self, now_millis: i64) -> String {
        self.archive_search_sequence = self.archive_search_sequence.wrapping_add(1);
        mint_id(self.archive_search_sequence, now_millis)
    }

    /// Mint a fresh `RuleSetId`, the same way [`Self::mint_archive_id`] mints an archive id:
    /// 32 lowercase hex characters, well inside `RuleSetId`'s modelled 1..100 length (the
    /// shape declares no pattern).
    pub fn mint_rule_set_id(&mut self, now_millis: i64) -> String {
        self.rule_set_sequence = self.rule_set_sequence.wrapping_add(1);
        mint_id(self.rule_set_sequence, now_millis)
    }

    /// Mint a fresh `TrafficPolicyId`, the same way [`Self::mint_archive_id`] mints an
    /// archive id: 32 lowercase hex characters, well inside `TrafficPolicyId`'s modelled
    /// 1..100 length (the shape declares no pattern).
    pub fn mint_traffic_policy_id(&mut self, now_millis: i64) -> String {
        self.traffic_policy_sequence = self.traffic_policy_sequence.wrapping_add(1);
        mint_id(self.traffic_policy_sequence, now_millis)
    }
}

/// The identifier minted for the `sequence`-th resource of its kind: 32 lowercase hex
/// characters.
///
/// The low half is a bijection of that per-kind sequence number, so two resources can
/// never share an id; the high half mixes in the creation instant so ids are not
/// consecutive and a caller cannot derive the next one from the last.
fn mint_id(sequence: u64, now_millis: i64) -> String {
    let high = splitmix64((now_millis as u64) ^ sequence.rotate_left(32));
    let low = splitmix64(sequence);
    format!("{high:016x}{low:016x}")
}

/// The secret this service's page tokens are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED.
///
/// A continuation token carries the position itself (there is no page kept on the side to
/// look up), so only the authenticator over that position tells a token this store minted
/// from one a caller wrote: a paginated read can then refuse a value it did not issue
/// instead of serving a page from a position the caller chose. A token minted under another
/// store's key — another emulator process, a restarted one — names no position here either.
pub use ::service_common::page_tokens::PageTokenKey;

/// Encode `cursor` as an opaque `NextToken` this store can later recognise as its own: the
/// authenticated bytes of the cursor, hex-encoded.
///
/// Hex is this service's spelling for an opaque token — it is what makes the value one a
/// caller round-trips rather than constructs. The widest cursor is an `Address` (320 bytes
/// at most), so the token is at most 704 characters, inside `PaginationToken`'s modelled
/// 1..2048 length.
pub fn encode_page_token(key: &PageTokenKey, cursor: &str) -> String {
    ::hex::encode(key.mint(cursor))
}

/// The cursor a `NextToken` carries, or `None` for any token THIS store did not mint — one
/// that is not hex, one too short to carry an authenticator, one whose authenticator does
/// not match the cursor it travels with, and one whose cursor is not UTF-8.
pub fn decode_page_token(key: &PageTokenKey, token: &str) -> Option<String> {
    key.cursor_of(&::hex::decode(token).ok()?)
}

/// The walk a continuation token of an archive-scoped job listing (`ListArchiveSearches`,
/// `ListArchiveExports`) belongs to: the archive being listed, ahead of the position
/// itself.
///
/// A token continues ONE walk, so the position it carries travels with the archive it was
/// issued for and a token presented to another archive's listing names no position in it.
/// `ArchiveId` is `@pattern ^a-[\w]{1,64}$`, so an id cannot carry the separator and the
/// boundary between the scope and the position is unambiguous.
pub fn archive_job_page_scope(archive_id: &str) -> String {
    format!("{archive_id}\u{1f}")
}

/// The message a handler reports when the shared store cannot be reached.
///
/// Each handler still builds its own modelled error (the generated error enums are
/// per-operation), so this is the one part of that answer they can share.
pub const STORE_UNAVAILABLE: &str = "service state is unavailable; retry the request";

/// What one `ClientToken` recorded: which resource the first call created, and a
/// description of the request it created it from.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    pub resource_id: String,
    pub fingerprint: String,
}

/// A stable description of the members an archive is created from.
///
/// An `@idempotencyToken` identifies a RETRY of one request, so a token presented with a
/// different request is not a retry: comparing this description is what lets
/// `CreateArchive` tell the two apart instead of answering the second with the first
/// one's id. `Tags` are part of it too, because the record stores them.
pub fn create_archive_fingerprint(
    archive_name: &str,
    retention: Option<&ArchiveRetention>,
    kms_key_arn: Option<&str>,
    tags: Option<&[Tag]>,
) -> String {
    let retention = match retention {
        Some(ArchiveRetention::RetentionPeriod(period)) => period.as_str(),
        None => "",
    };
    // `\u{1e}` (record separator) cannot appear in any of these members, so the joined
    // description is unambiguous.
    format!(
        "{archive_name}\u{1e}{retention}\u{1e}{}\u{1e}{}",
        kms_key_arn.unwrap_or_default(),
        render_tags(tags)
    )
}

/// A stable description of the members an address list is created from; see
/// [`create_archive_fingerprint`] for what it is compared against and why.
pub fn create_address_list_fingerprint(address_list_name: &str, tags: Option<&[Tag]>) -> String {
    format!("{address_list_name}\u{1e}{}", render_tags(tags))
}

/// A stable description of the members a rule set is created from; see
/// [`create_archive_fingerprint`] for what it is compared against and why.
///
/// The rules are part of it because the record stores them, and they are rendered with the
/// derived `Debug` formatting of the modelled shapes: a `Rule` is a tree of structures and
/// unions over strings, numbers and enums, so that rendering is total, deterministic and
/// injective enough that two different rule lists cannot describe each other.
pub fn create_rule_set_fingerprint(
    rule_set_name: &str,
    rules: &[Rule],
    tags: Option<&[Tag]>,
) -> String {
    format!(
        "{rule_set_name}\u{1e}{rules:?}\u{1e}{}",
        render_tags(tags)
    )
}

/// A stable description of the members a traffic policy is created from; see
/// [`create_archive_fingerprint`] for what it is compared against and why.
///
/// The policy statements are part of it, rendered with the derived `Debug` formatting of the
/// modelled shapes for the same reason [`create_rule_set_fingerprint`] renders rules that
/// way, and so are the default action and the message-size ceiling, because the record
/// stores them all.
pub fn create_traffic_policy_fingerprint(
    traffic_policy_name: &str,
    policy_statements: &[PolicyStatement],
    default_action: &str,
    max_message_size_bytes: Option<i32>,
    tags: Option<&[Tag]>,
) -> String {
    format!(
        "{traffic_policy_name}\u{1e}{policy_statements:?}\u{1e}{default_action}\u{1e}{}\u{1e}{}",
        max_message_size_bytes
            .map(|bytes| bytes.to_string())
            .unwrap_or_default(),
        render_tags(tags)
    )
}

/// A stable description of the members an address-list import job is created from; see
/// [`create_archive_fingerprint`] for what it is compared against and why.
pub fn create_import_job_fingerprint(
    address_list_id: &str,
    name: &str,
    import_data_type: &str,
) -> String {
    format!("{address_list_id}\u{1e}{name}\u{1e}{import_data_type}")
}

/// The tags of a create request, rendered into one unambiguous string for a fingerprint.
///
/// Sorted: the same set in a different order is the same request. `\u{1f}` (unit
/// separator) and `\u{1e}` (record separator) cannot appear in a `TagKey` or `TagValue`
/// (both are patterned), so the rendering is injective.
fn render_tags(tags: Option<&[Tag]>) -> String {
    let mut rendered: Vec<String> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| format!("{}\u{1f}{}", tag.key, tag.value))
        .collect();
    rendered.sort();
    rendered.join("\u{1e}")
}

/// SplitMix64, a standard 64-bit mixing function: a bijection on `u64`, so distinct
/// inputs always give distinct outputs. Used only to scatter minted identifiers.
///
/// Per-service, like [`now_millis`] and [`request_region`], because the framework has no
/// shared home for any of them: `vendor/` is read-only from here, and a shared crate under
/// `services/` would exist for three helpers of a few lines each. Other bootstrapped
/// services carry their own copies for the same reason.
fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The current wall-clock instant as epoch milliseconds. Per-service for the reason given
/// on [`splitmix64`].
pub fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The new `LastUpdatedTimestamp` for a record last written at `previous`.
///
/// `now` unless the clock has not advanced since `previous`, in which case one
/// millisecond past `previous` — so an update is always observably later than the write
/// before it, which is what a caller comparing two reads relies on.
pub fn bump_millis(previous: i64, now: i64) -> i64 {
    now.max(previous.saturating_add(1))
}

/// Render a stored epoch-millisecond instant as the modelled `Timestamp`.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request was signed for, for the ARNs the response carries.
///
/// A signed AWS SDK request names it in the SigV4 credential scope; an unsigned one
/// names no region, so the emulator's default stands in.
///
/// Per-service by necessity: the natural shared home for this is beside
/// `emulator_server::sigv4_credential_scope`, and that crate is vendored and read-only
/// from here, so each bootstrapped service wraps it in its own `state.rs` until the
/// framework grows the accessor. See also the note on [`splitmix64`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an archive, in this service's ARN namespace.
///
/// `smithy_query(query="service_info")` reports `arnNamespace: ses` for MailManager, and
/// the archive resource type is `mail-manager-archive`.
pub fn archive_arn(region: &str, archive_id: &str) -> String {
    ::engine::emulator_arn("ses", region, &format!("mail-manager-archive/{archive_id}"))
}

/// The ARN of an address list, in this service's ARN namespace.
///
/// `AddressListArn` carries `@arnReference(service: "ses")`
/// (`smithy_query(query="shape",shape="AddressListArn")`), and SES names the Mail Manager
/// address-list resource type `mailmanager-address-list` in an ARN.
pub fn address_list_arn(region: &str, address_list_id: &str) -> String {
    ::engine::emulator_arn(
        "ses",
        region,
        &format!("mailmanager-address-list/{address_list_id}"),
    )
}

/// The ARN of a rule set, in this service's ARN namespace.
///
/// `RuleSetArn` carries `@arnReference`
/// (`smithy_query(query="shape",shape="RuleSetArn")`), and SES names the Mail Manager
/// rule-set resource type `mailmanager-rule-set` in an ARN.
pub fn rule_set_arn(region: &str, rule_set_id: &str) -> String {
    ::engine::emulator_arn(
        "ses",
        region,
        &format!("mailmanager-rule-set/{rule_set_id}"),
    )
}

/// The ARN of a traffic policy, in this service's ARN namespace.
///
/// `TrafficPolicyArn` carries `@arnReference`
/// (`smithy_query(query="shape",shape="TrafficPolicyArn")`), and SES names the Mail Manager
/// traffic-policy resource type `mailmanager-traffic-policy` in an ARN.
pub fn traffic_policy_arn(region: &str, traffic_policy_id: &str) -> String {
    ::engine::emulator_arn(
        "ses",
        region,
        &format!("mailmanager-traffic-policy/{traffic_policy_id}"),
    )
}

/// The segment that names THIS service's upload plane behind the reserved data-plane path
/// namespace (`::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`), so the
/// locator [`import_job_upload_url`] hands out and the route that accepts the upload
/// (`crate::service`) cannot drift, and so the path collides with no modelled route of any
/// service.
pub const IMPORT_JOB_UPLOAD_PLANE: &str = "address-list-import-jobs";

/// The capability the published locator of the import job `job_id` carries in its last path
/// segment: the `JobId` AUTHENTICATED under this store's upload key, in this service's
/// opaque spelling ([`encode_page_token`]).
///
/// What makes holding the locator — and not merely knowing a `JobId`, which every job read
/// and listing hands out — the thing that admits a write: no caller can produce the
/// authenticator for a job id of its own, and the token this store minted is reported
/// nowhere but in the locator itself. [`import_job_of_upload_token`] reads it back.
pub fn import_job_upload_token(key: &PageTokenKey, job_id: &str) -> String {
    encode_page_token(key, job_id)
}

/// The `JobId` the upload token `upload_token` carries, or `None` for any value THIS store's
/// upload key did not mint — a token a caller assembled, one whose authenticator does not
/// match the id it travels with, and one minted by another store (another emulator process,
/// a restarted one).
///
/// The comparison is the authenticator's own constant-time one, so a refusal tells a caller
/// nothing about how close its value was.
pub fn import_job_of_upload_token(key: &PageTokenKey, upload_token: &str) -> Option<String> {
    decode_page_token(key, upload_token)
}

/// The path the input file behind the locator `upload_token` is uploaded at:
/// `/_aws/{plane}/{resource}`, the reserved shape for a data plane this emulator serves on
/// its one endpoint.
pub fn import_job_upload_path(upload_token: &str) -> String {
    format!(
        "/{}/{IMPORT_JOB_UPLOAD_PLANE}/{upload_token}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The upload token `path` presents, or `None` for every other path.
///
/// Reads the same namespace and plane segments the locator is built from. The token has to
/// have the FORM this store mints one in, so neither a path of the same shape addressed to
/// another mounted service nor one built from a job's own `JobId` is mistaken for a locator
/// this service published. WHICH job the token addresses is a question only the store's key
/// answers ([`import_job_of_upload_token`]).
pub fn parse_import_job_upload_path(path: &str) -> Option<&str> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};

    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != IMPORT_JOB_UPLOAD_PLANE {
        return None;
    }
    let upload_token = rest.strip_prefix('/')?;
    is_upload_token_shaped(upload_token).then_some(upload_token)
}

/// Whether `candidate` has the FORM of an upload token this store mints: the hexadecimal of
/// at least an authenticator and a one-character cursor, and no longer than a token over the
/// longest `JobId` the model admits (length 1..255).
///
/// A shape test only, so the plane answers a well-formed locator rather than appearing
/// absent; whether the token is one this store minted is
/// [`import_job_of_upload_token`]'s answer, and only it admits a write.
fn is_upload_token_shaped(candidate: &str) -> bool {
    let authenticated = 2 * ::service_common::page_tokens::AUTHENTICATOR_BYTES;
    (authenticated + 2..=authenticated + 2 * 255).contains(&candidate.len())
        && candidate.len().is_multiple_of(2)
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Where a client uploads the input file of the import job the locator `upload_token`
/// addresses: `CreateAddressListImportJobResponse$PreSignedUrl`, "The pre-signed URL target
/// for uploading the input file".
///
/// AWS answers an Amazon S3 pre-signed URL here. This emulator provisions no object store,
/// so the URL names THIS job's input file on the endpoint THIS REQUEST ARRIVED AT, and
/// this service accepts the upload on that path itself (see `crate::service`) — a URL a
/// client can really write the file to, rather than one pointing at the real service.
///
/// What AUTHORISES that write is the locator itself, as it is for a pre-signed URL: its last
/// segment is the `JobId` authenticated under this store's upload key
/// ([`import_job_upload_token`]), the upload plane admits a write only for a token that key
/// minted, and no modelled operation reports it — so a caller holding only the `JobId` every
/// job read and listing hands out addresses no upload target at all. No SigV4 signature is
/// attached, because this service's locator carries its own authenticated capability instead
/// and a fabricated signature would claim a computation nothing performed.
pub fn import_job_upload_url(host: &str, upload_token: &str) -> String {
    format!("https://{host}{}", import_job_upload_path(upload_token))
}

/// The endpoint host this request reached the emulator on, so a URL this service hands out
/// resolves back to where the caller is already talking rather than naming a host nothing
/// serves.
///
/// The reading is the workspace's one
/// (`::service_common::data_plane_addressing::reached_endpoint_host`): the host the request
/// addressed — its `Host` header, or the authority of an absolute-form URI for a request
/// that carries its endpoint there — when it is shaped like a host, and this service's
/// published endpoint for the region it was signed for otherwise.
pub fn request_endpoint_host(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> String {
    ::service_common::data_plane_addressing::reached_endpoint_host(
        &ctx.parts().uri.to_string(),
        ctx.parts()
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        crate::service::ENDPOINT_PREFIX,
        region,
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMailmanagerMailmanagersvcState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMailmanagerMailmanagersvcState>)
}
