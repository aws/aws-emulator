// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medicalimaging-ahigatewayservice service.
//!
//! One data store map is the whole control plane: a data store owns its DICOM import
//! jobs and its image sets, so every handler reaches one record rather than joining
//! two parallel maps.
//!
//! # The clock, not the reader, moves a status
//!
//! AWS HealthImaging answers `CreateDatastore` with `CREATING` and `DeleteDatastore`
//! with `DELETING`, `StartDICOMImportJob` with `SUBMITTED`, and `CopyImageSet` with a
//! `LOCKED` source and destination — non-terminal statuses the service itself advances
//! afterwards. This crate stores the instant each
//! transition falls DUE and materialises it in [`ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`],
//! which every handler calls first. Nothing sleeps, and a reporting operation
//! (`GetDatastore`, `ListDatastores`, `GetDICOMImportJob`, `GetImageSet`) only ever
//! writes a change the clock already owed at that instant — so a read never moves a
//! status that was not already due.
//!
//! One owed change cannot be made by the clock, because it is not a write into this
//! store: the output manifests a finished DICOM import job owes the Amazon S3 prefix it
//! was pointed at ([`import_job_output`]). Such a job carries that delivery, reaches its
//! terminal status only once it has been made, and the delivery is made by the job's own
//! timer or by the first job read that finds it due — the same materialisation, at the
//! same instant, in the one place that can make it.
//!
//! # A data store belongs to ONE region
//!
//! AWS HealthImaging is a regional service: a data store exists in the region the
//! `CreateDatastore` request was addressed to, and in no other. Each record carries
//! that region ([`Datastore::region`]), every id-keyed handler reaches a store through
//! [`ComAmazonawsMedicalimagingAhigatewayserviceState::datastore_in`] with the region
//! of the request being served ([`request_region`]), and `ListDatastores` enumerates
//! [`ComAmazonawsMedicalimagingAhigatewayserviceState::reported_datastores_in`] — so a
//! request addressed to another region reaches nothing and is answered exactly as it
//! would be for an id nothing ever minted.
//!
//! # A finished deletion is reported before it is forgotten
//!
//! `DELETED` is a status the model declares on the data store shapes and accepts as the
//! `ListDatastores` filter, so a deletion that has landed is OBSERVABLE in it: the
//! record survives as a tombstone for [`DATASTORE_DELETED_RETENTION`], reported by
//! `GetDatastore` and by the listing (filtered or not), and the clock then reaps it.
//! A tombstone is reportable and not reachable — it holds no quota slot, and every call
//! that would act on it is answered as it is for an id nothing ever minted, which is the
//! split between
//! [`ComAmazonawsMedicalimagingAhigatewayserviceState::reported_datastore_in`] and
//! [`ComAmazonawsMedicalimagingAhigatewayserviceState::datastore_in`].
//!
//! # Pixel data belongs to the image set
//!
//! An image set owns the image frames it holds ([`ImageFrame`]), keyed by image frame
//! id, because `GetImageFrame` takes no version: every version of an image set reaches
//! the same frames. A frame is readable only once something has DELIVERED it — the copy
//! that completes carries its source's frames to its destination — so nothing is served
//! that was never stored.
//!
//! Serving or delivering that content is an ACCESS of the image set, and an access is
//! the only thing [`ImageSet::last_accessed_at`] reports: an image set carries the
//! instant it came into existence until something reads the content it holds or writes
//! content into it, and the instant of the latest such access from then on. Reporting an
//! image set's properties is not an access of its content, so no reader of those moves
//! it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{DICOMTags, ImportConfiguration};

/// What a DICOM import READS, and the image sets it materialises from what it read.
mod dicom_import;
pub use dicom_import::*;

/// What a DICOM import job's completion WRITES, and what its outcome is decided from.
pub mod import_job_output;

/// How long after `CreateDatastore` the data store leaves `CREATING` for `ACTIVE`.
///
/// AWS publishes no duration for it, so this is a chosen span: long enough that a
/// caller polling straight after the create still observes `CREATING`, short enough
/// that a poll loop terminates promptly.
pub const DATASTORE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteDatastore` the data store leaves `DELETING` for `DELETED`.
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const DATASTORE_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long a finished deletion stays OBSERVABLE in `DELETED` before its record is
/// reaped and the data store is gone for good.
///
/// `DELETED` is a status the model declares on `DatastoreProperties` / `DatastoreSummary`
/// and accepts as the `ListDatastores` `datastoreStatus` filter, so a finished deletion
/// is reportable in it rather than vanishing the instant it lands. AWS publishes no
/// duration for how long it stays reportable, so this is a chosen span, for the same
/// reason as [`DATASTORE_ACTIVATION_DELAY`].
pub const DATASTORE_DELETED_RETENTION: Duration = Duration::from_millis(500);

/// How long after `StartDICOMImportJob` the job leaves `SUBMITTED` for `IN_PROGRESS`.
///
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`], and HALF of
/// [`IMPORT_JOB_COMPLETION_DELAY`]: `SUBMITTED` and `IN_PROGRESS` are both non-terminal
/// statuses the model declares, so each of them is observable for the same part of the
/// window the job is in flight for.
pub const IMPORT_JOB_IN_PROGRESS_DELAY: Duration = Duration::from_secs(2);

/// How long after `StartDICOMImportJob` the job reaches its terminal status and gets
/// `endedAt`.
///
/// A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`], but a WIDER one:
/// a job in flight is what `DeleteDatastore` refuses a delete of
/// ([`DATASTORE_RUNNING_IMPORT_JOBS_MESSAGE`]), so the window has to stay open long enough
/// for a client that submits a job and then calls another operation to observe it, rather
/// than being a window only a tight loop can catch.
pub const IMPORT_JOB_COMPLETION_DELAY: Duration = Duration::from_secs(4);

/// How long after `CopyImageSet` the copy completes: the source image set leaves
/// `COPYING_WITH_READ_ONLY_ACCESS` and the destination leaves `COPYING`, both returning
/// to `ACTIVE`. A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`] —
/// AWS publishes no duration for a copy, and a caller reading either image set straight
/// after the call still observes the `LOCKED` copy in flight.
pub const IMAGE_SET_COPY_DELAY: Duration = Duration::from_millis(400);

/// How long after `DeleteImageSet` the deletion completes: the image set leaves the
/// `LOCKED` / `DELETING` pair the delete put it in for `DELETED` / `DELETED`, and gets
/// its `deletedAt`. A chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`]
/// — AWS publishes no duration for a delete, and a caller reading the image set straight
/// after the call still observes the deletion in flight.
pub const IMAGE_SET_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long after `UpdateImageSetMetadata` the update completes: the image set leaves
/// the `LOCKED` / `UPDATING` pair the update put it in for `ACTIVE` / `UPDATED`. A
/// chosen span, for the same reason as [`DATASTORE_ACTIVATION_DELAY`] — AWS publishes no
/// duration for a metadata update, and a caller reading the image set straight after the
/// call still observes the update in flight.
pub const IMAGE_SET_UPDATE_DELAY: Duration = Duration::from_millis(400);

/// The external version identifier an image set's FIRST version carries. AWS numbers
/// image set versions from 1 (`ImageSetExternalVersionId` is `^\d+$`), and each later
/// version is one past the one it supersedes.
pub const FIRST_IMAGE_SET_VERSION_ID: u32 = 1;

/// The smallest page a listing will serve — the model's `maxResults` minimum, which is
/// the same 1..50 range on every paginated operation that declares one
/// (`smithy_query(query="operation",operation="ListDatastores,ListDICOMImportJobs,SearchImageSets")`).
pub const MIN_LIST_PAGE_SIZE: usize = 1;

/// The largest page a listing will serve — the model's `maxResults` maximum, and
/// also the page size used when the caller names no `maxResults`. Every page is clamped
/// into `MIN_LIST_PAGE_SIZE..=MAX_LIST_PAGE_SIZE` by [`list_page_size`] rather than
/// trusting the boundary validator alone, so one request can never materialise an
/// unbounded response.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// How many data stores this emulator will hold at once IN ONE REGION.
///
/// A memory bound, not an AWS quota: `CreateDatastore` persists caller-supplied data
/// (name, ARNs, the whole tag map) in process memory, so without a cap a caller could
/// grow it without limit. Past the cap the call is refused with the operation's own
/// `ServiceQuotaExceededException` (HTTP 402). Counted per region, because a data store
/// is a REGIONAL resource: the stores of one region are the only ones a request
/// addressed to it can reach, so they are the only ones its quota can count.
pub const MAX_DATASTORES: usize = 128;

/// How many data stores this emulator will hold at once across EVERY region.
///
/// The process-wide companion to the per-region [`MAX_DATASTORES`]: a per-region cap
/// alone bounds no memory, because a caller is free to address as many regions as it
/// likes. Past this cap `CreateDatastore` is refused with the same
/// `ServiceQuotaExceededException`.
///
/// This one counts RECORDS, the tombstone of a finished deletion included
/// ([`DATASTORE_DELETED_RETENTION`]), so what this emulator holds is bounded by it even
/// while a create/delete loop is leaving tombstones behind.
///
/// No test drives this refusal: reaching it costs this many successful creates, which
/// is minutes of wall clock in a suite. The per-region cap, which shares the one code
/// path and the one error, is exercised instead.
pub const MAX_DATASTORES_ALL_REGIONS: usize = 1024;

/// How many import jobs one data store will hold at once — the same memory bound as
/// [`MAX_DATASTORES`], applied per data store so the product of the two is bounded too.
/// Past the cap `StartDICOMImportJob` is refused with `ServiceQuotaExceededException`.
pub const MAX_IMPORT_JOBS_PER_DATASTORE: usize = 128;

/// How many image sets one data store will HOLD at once — the same memory bound as
/// [`MAX_IMPORT_JOBS_PER_DATASTORE`], applied per data store. Past the cap an operation
/// that would mint another image set (`CopyImageSet` without a destination image set)
/// is refused with `ServiceQuotaExceededException`.
///
/// Counted over the image sets the data store still holds
/// ([`Datastore::held_image_sets`]), exactly as the per-region [`MAX_DATASTORES`] is
/// counted over the data stores a region still holds: a tombstone has given up the
/// content it held, so it takes no place here and a deletion gives its place back.
/// [`MAX_IMAGE_SET_RECORDS_PER_DATASTORE`] is what bounds the records.
pub const MAX_IMAGE_SETS_PER_DATASTORE: usize = 128;

/// How many image set RECORDS one data store will hold at once, the tombstones of
/// finished deletions included.
///
/// The companion of [`MAX_IMAGE_SETS_PER_DATASTORE`] that bounds MEMORY, as
/// [`MAX_DATASTORES_ALL_REGIONS`] is the companion of the per-region data store cap: a
/// retired image set gives up its place among the image sets the store holds, but its
/// record lives on to be reported in `DELETED`, so a mint/delete loop would otherwise
/// grow one data store without limit. Past this cap a mint is refused with the same
/// `ServiceQuotaExceededException`.
///
/// Well above the cap on what a store HOLDS, so that reaching it takes a caller that is
/// cycling image sets rather than one using the data store as the service intends.
pub const MAX_IMAGE_SET_RECORDS_PER_DATASTORE: usize = 1024;

/// How many versions one image set will hold at once.
///
/// A memory bound, not an AWS quota: every copy into an existing image set keeps the
/// version it superseded, so without a cap a caller could grow one image set's version
/// list without limit. Past the cap the call is refused with the operation's own
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_IMAGE_SET_VERSIONS: usize = 128;

/// How many attributes OUTSIDE the ones the model names one image set will hold at once
/// ([`ImageSetAttributes::extra_attributes`]).
///
/// A memory bound, not an AWS quota, and the companion of [`MAX_IMAGE_SET_VERSIONS`]: a
/// metadata update persists the attributes it names, and every version the image set keeps
/// holds a snapshot of them, so without a cap repeated updates could grow one image set
/// without limit. Past the cap the update is refused with its own
/// `ServiceQuotaExceededException` (HTTP 402). The modelled attributes need no such cap:
/// there are sixteen of them, whatever is written.
///
/// EVERY writer of these attributes is bounded by it, counted over what the image set would
/// hold once the write had landed ([`ImageSetAttributes::written_attributes`]) and refused
/// before anything is written: the metadata update that writes them, and the COPY that
/// delivers them into a destination that already holds some of its own.
pub const MAX_IMAGE_SET_EXTRA_ATTRIBUTES: usize = 512;

/// The `ServiceQuotaExceededException` message every writer of the attributes outside the
/// modelled ones is refused past [`MAX_IMAGE_SET_EXTRA_ATTRIBUTES`] with — one wording,
/// because it reports one bound.
pub fn extra_attributes_quota_message() -> String {
    format!(
        "this emulator holds at most {MAX_IMAGE_SET_EXTRA_ATTRIBUTES} attributes per image \
         set beyond the ones the service models"
    )
}

/// How many `@idempotencyToken` entries the ledger keeps.
///
/// The ledger outlives the resources it names (a token must stay honoured for a while
/// after its resource is gone), so it is bounded two ways: entries whose resource has
/// been retired are dropped in
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`], and the
/// remainder is capped here with FIFO eviction of the oldest token.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// The service's `arnNamespace` / sigv4 signing name, used to build data store ARNs.
pub const ARN_SERVICE: &str = "medical-imaging";

/// The region an ARN names when the request carries no SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// How this service WORDS a refusal
//
// Three wording rules live here because more than one handler — and the
// `override_refusal` seam in `crate::service` — states them, and a text spelled
// twice drifts:
//
// 1. A missing resource is stated by its KIND, as a constant: the text carries no
//    identifier from the request, so one caller probing ids cannot read them back
//    out of the 404 body ([`DATASTORE_NOT_FOUND_MESSAGE`],
//    [`IMPORT_JOB_NOT_FOUND_MESSAGE`]) — including on the ARN-keyed tagging routes,
//    whose lookup reports the absent kind rather than the ARN it was named by
//    ([`MissingResource`]).
// 2. A continuation token this service did not issue is refused with ONE fixed
//    sentence that does not vary with the token supplied
//    ([`INVALID_NEXT_TOKEN_MESSAGE`]).
// 3. An input-constraint violation is stated in the AWS envelope over the framework's
//    own clauses, with every `@pattern` quoted WITHOUT the anchors the model spells it
//    with — and with the rejected value withheld for the members this service does not
//    reflect ([`constraint_clause`], [`VALUE_WITHHOLDING_MEMBERS`],
//    [`validation_envelope`]).
//
// A malformed resource ARN is NOT one of them: the ARN parse runs before constraint
// validation, so `crate::service` answers it with the modelled `BadRequestException`
// and [`MALFORMED_ARN_MESSAGE`] instead.
//
// Over all three, and over the state-conflict texts below them, one shape holds: a
// refusal sentence states the RULE the request broke, as a constant of this module, and
// carries no identifier naming the resource the request addressed — a store id, an image
// set id, a job id, an ARN or a token. The only request-derived value any of them
// interpolates is a COUNT measured against a bound the same sentence names
// ([`too_many_tags_message`]), which says which side of the published limit the caller landed
// on and names no resource.
// ---------------------------------------------------------------------------

/// The text this service answers a request naming a data store it does not hold with.
///
/// A constant of the service, not of the request: the recorded corpus shows the same
/// sentence from `GetDatastore`, `DeleteDatastore`, `GetDICOMImportJob` and
/// `StartDICOMImportJob`, with no data store identifier in it.
pub const DATASTORE_NOT_FOUND_MESSAGE: &str = "Datastore resource is not found.";

/// The text this service answers a request naming an import job a data store does not
/// hold with.
///
/// The resource KIND is what the sentence states, exactly as
/// [`DATASTORE_NOT_FOUND_MESSAGE`] does for a data store; AWS's own wording for an
/// absent import job is not recorded, so this spelling is chosen by analogy. What is
/// NOT chosen is the shape: no identifier the caller supplied appears in it.
pub const IMPORT_JOB_NOT_FOUND_MESSAGE: &str = "ImportJob resource is not found.";

/// The text this service answers a request naming an image set a data store does not
/// hold with — the resource KIND, by the same rule as [`DATASTORE_NOT_FOUND_MESSAGE`],
/// with no identifier from the request in it.
pub const IMAGE_SET_NOT_FOUND_MESSAGE: &str = "ImageSet resource is not found.";

/// The text this service answers a request naming a VERSION of an image set it holds no
/// such version of with. Stated by kind, like [`IMAGE_SET_NOT_FOUND_MESSAGE`].
pub const IMAGE_SET_VERSION_NOT_FOUND_MESSAGE: &str =
    "ImageSet version resource is not found.";

/// The text this service refuses a copy whose caller named a version that is no longer
/// the image set's latest with. A `latestVersionId` is an optimistic-concurrency check:
/// a stale one means the caller's view of the image set is out of date, which is what
/// `ConflictException` (HTTP 409) reports. Constant, echoing neither version.
pub const IMAGE_SET_VERSION_CONFLICT_MESSAGE: &str =
    "The latest image set version identifier provided does not match the image set's latest version.";

/// The text this service refuses a copy of (or into) an image set that is not `ACTIVE`
/// with — one still `LOCKED` by a copy in flight, or already `DELETED`. Constant, and
/// stated by rule rather than by which image set it was.
pub const IMAGE_SET_NOT_ACTIVE_MESSAGE: &str =
    "The image set is not in the ACTIVE state, so it cannot take part in a copy.";

/// The text this service refuses a DELETE of an image set that is `LOCKED` by work
/// already in flight (a copy) with. A delete admitted mid-copy would leave the copy and
/// the delete disagreeing about the image set, which is what `ConflictException`
/// (HTTP 409) reports. Constant, and stated by rule rather than by which image set it
/// was; a delete the service is ALREADY performing is not refused at all — it is
/// idempotent.
pub const IMAGE_SET_NOT_DELETABLE_MESSAGE: &str =
    "The image set is locked by an action already in flight, so it cannot be deleted.";

/// The text this service refuses a METADATA UPDATE of an image set that cannot take one
/// with — one that is not `ACTIVE`, or one an action already in flight is holding. An
/// update admitted there would leave that action and the update disagreeing about the
/// image set, which is what `ConflictException` (HTTP 409) reports. Constant, and stated
/// by rule rather than by which image set it was, like [`IMAGE_SET_NOT_ACTIVE_MESSAGE`]
/// one operation across.
pub const IMAGE_SET_NOT_UPDATABLE_MESSAGE: &str =
    "The image set is not in the ACTIVE state, so its metadata cannot be updated.";

/// The text this service refuses a copy into an image set whose patient, study or series
/// level DICOM metadata disagrees with the copied content's, where the request did not ask
/// for the copy to be completed anyway.
///
/// `CopyImageSet`'s `force` is documented as the flag that forces completion "even if there
/// are inconsistent Patient, Study, and/or Series level metadata elements between the
/// sourceImageSet and destinationImageSet", so without it such a copy is not completed: the
/// destination would end up describing two different studies, which is the inconsistent
/// state `ConflictException` (HTTP 409) reports. Constant, and stated by rule rather than
/// by which element disagreed, like [`IMAGE_SET_NOT_ACTIVE_MESSAGE`] beside it.
pub const IMAGE_SET_COPY_INCONSISTENT_METADATA_MESSAGE: &str =
    "The source and destination image sets have inconsistent Patient, Study and/or Series \
     level metadata elements, so the copy cannot be completed without the force flag.";

/// The text this service answers a request naming an image frame an image set does not
/// hold with — the resource KIND, by the same rule as [`IMAGE_SET_NOT_FOUND_MESSAGE`],
/// with no identifier from the request in it.
pub const IMAGE_FRAME_NOT_FOUND_MESSAGE: &str = "ImageFrame resource is not found.";

/// The `Patient` level of the aggregated DICOM metadata document — the document
/// `GetImageSetMetadata` serves, `UpdateImageSetMetadata` writes and `CopyImageSet`'s
/// subset selector is written in. Held here because more than one operation reads a
/// document by these level names, so they are spelled once.
pub const METADATA_PATIENT_KEY: &str = "Patient";

/// The `Study` level of that document.
pub const METADATA_STUDY_KEY: &str = "Study";

/// The map of series inside the study level of that document, keyed by Series Instance
/// UID.
pub const METADATA_SERIES_KEY: &str = "Series";

/// The map of SOP instances inside one series of that document, keyed by SOP Instance UID.
pub const METADATA_INSTANCES_KEY: &str = "Instances";

/// The object holding one level's DICOM attributes in that document, keyed by attribute
/// keyword.
pub const METADATA_DICOM_KEY: &str = "DICOM";

/// The key of that document declaring which SHAPE of document it is, written at the top of
/// every document `GetImageSetMetadata` serves.
pub const METADATA_SCHEMA_VERSION_KEY: &str = "SchemaVersion";

/// The key of that document naming the data store the document was built from.
pub const METADATA_DATASTORE_ID_KEY: &str = "DatastoreID";

/// The key of that document naming the image set the document was built from.
pub const METADATA_IMAGE_SET_ID_KEY: &str = "ImageSetID";

/// The top-level keys of that document the service COMPUTES from the records it served the
/// document out of, rather than naming an attribute of an image set: which shape of
/// document it is, and which data store and image set it was built from. A document a
/// caller sends carries them to say what it is written against, so there is nothing in them
/// to store — which is why `UpdateImageSetMetadata` stores none of them and
/// `GetImageSetMetadata` always reports its own.
pub const METADATA_COMPUTED_KEYS: [&str; 3] = [
    METADATA_SCHEMA_VERSION_KEY,
    METADATA_DATASTORE_ID_KEY,
    METADATA_IMAGE_SET_ID_KEY,
];

/// The text this service refuses a call that would act on a data store, or on the
/// contents of one, that has not settled into `ACTIVE` with — one still `CREATING` or
/// already `DELETING`.
///
/// AWS's own sentence for this conflict: the whole of it is fixed text, carrying neither
/// the data store's identifier nor the status it was in — stated by RULE rather than by
/// which store it was, exactly like [`IMAGE_SET_NOT_ACTIVE_MESSAGE`] one resource kind
/// down.
pub const DATASTORE_NOT_ACTIVE_MESSAGE: &str = "The datastore is not active.";

/// The text this service refuses the DELETE of a data store that still has a DICOM import
/// job running on it with.
///
/// A constant, carrying neither the data store it was nor which jobs were in flight —
/// AWS's own sentence for this conflict, which interpolates nothing from the request or
/// from state.
pub const DATASTORE_RUNNING_IMPORT_JOBS_MESSAGE: &str =
    "Cannot delete a datastore with running import jobs";

/// The text this service refuses the DELETE of a data store whose creation is still in
/// flight with. A separate sentence from [`DATASTORE_NOT_ACTIVE_MESSAGE`] because the
/// delete is the one call a `DELETING` store still answers (idempotently), so the only
/// status it refuses is `CREATING`.
pub const DATASTORE_STILL_CREATING_MESSAGE: &str =
    "The data store is still being created, so it cannot be deleted yet.";

/// The text this service refuses the DELETE of a data store that still holds image sets
/// with.
///
/// The model states the precondition the sentence restates — "before a data store can be
/// deleted, you must first delete all image sets within it" — so the refusal is worded by
/// that RULE: a constant carrying neither the data store it was nor how many image sets
/// it held, exactly like [`DATASTORE_STILL_CREATING_MESSAGE`] above it, the sibling
/// refusal of the same operation. AWS's own wording for this conflict is not recorded, so
/// the spelling is chosen by analogy; what is NOT chosen is the shape.
pub const DATASTORE_STILL_HOLDS_IMAGE_SETS_MESSAGE: &str =
    "The data store still contains image sets, which must all be deleted before the data \
     store can be deleted.";

/// The storage tier every image set this emulator holds reports.
///
/// `StorageTier`'s other value, `ARCHIVE_INSTANT_ACCESS`, is reached by archiving, and
/// this service's model binds no operation that archives an image set
/// (`smithy_query(query="list_operations")`) — so an image set here never leaves the
/// tier a new one is created in.
pub const DEFAULT_STORAGE_TIER: &str = "FREQUENT_ACCESS";

/// The text this service refuses a `nextToken` it did not issue with — fixed, and
/// independent of the token supplied, so the refusal reveals nothing about how a real
/// token is built.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "Failed to perform requested operation due to invalid NextToken. Please try again.";

/// The words that join a constraint clause's subject to its requirement. The framework
/// renders `Value '<v>' at '<path>'` before it and the requirement after it, so it is
/// also where [`constraint_clause`] cuts the framework's clause in two.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// One constraint violation, worded the way this service words it.
///
/// The framework already states every violation class in the AWS sentence
/// (`Value '<v>' at '<path>' failed to satisfy constraint: <requirement>`), so that
/// rendering is REUSED — a constraint class this service has never seen is still phrased
/// like the rest — and exactly two things are changed:
///
/// * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` the model
///   spells it with. AWS advertises the expression a value must satisfy, not the
///   anchoring that makes the match total, and a client reconstructing the advertised
///   pattern from the message must get the one the service really emits.
/// * a violation on one of [`VALUE_WITHHOLDING_MEMBERS`] (or a DICOM attribute) is
///   stated as `<path> failed to satisfy constraint: <requirement>`, with no value at
///   all: this service does not reflect free text a caller composed, a key reference or
///   patient detail back into an error body. Every other member — including one that
///   names another AWS resource — keeps the echoed value. Both forms are in the
///   recorded corpus, from the same operation: the clauses about the data store's name
///   and its KMS key carry no value where the one about the authorizer's function ARN
///   does. See [`withholds_value`].
///
/// The framework's clause is split at its LAST separator, and a `@pattern` is replaced
/// in the requirement half only: the echoed value precedes it and is caller-controlled,
/// so a caller who sends the separator text — or the pattern itself — as the value
/// cannot have its own value rewritten or read as a requirement.
pub fn constraint_clause(violation: &Violation) -> String {
    let rendered = violation.message();
    let Some((subject, requirement)) = rendered.rsplit_once(CONSTRAINT_SEPARATOR) else {
        // A clause the framework states in some other shape: keep it verbatim rather
        // than guessing at its parts.
        return rendered;
    };
    let requirement = service_requirement(&violation.kind, requirement);
    if withholds_value(&violation.path) {
        return format!("{}{CONSTRAINT_SEPARATOR}{requirement}", violation.path);
    }
    format!("{subject}{CONSTRAINT_SEPARATOR}{requirement}")
}

/// The requirement half of a clause as THIS service words it: the framework's own words,
/// with a `@pattern` quoted without the `^` / `$` the model spells it with — the one
/// rewording [`constraint_clause`] applies to a requirement, kept here so a refusal
/// stating a requirement on its own is worded by the same rule.
fn service_requirement(kind: &ViolationKind, rendered: &str) -> String {
    match kind {
        ViolationKind::Pattern(pattern) => rendered.replace(pattern.as_str(), unanchored(pattern)),
        _ => rendered.to_string(),
    }
}

/// The REQUIREMENT one violation kind states, with no subject in front of it — the words
/// a clause carries after `failed to satisfy constraint: `.
///
/// For a constraint a HANDLER applies to a value it found INSIDE a member the boundary
/// validator does not walk, such as an attribute of a JSON document carried in a blob:
/// the clause names the blob member it was carried in ([`handler_requirement_error`]) and
/// the requirement says which constraint of which attribute the value failed. Worded
/// through the framework's own rendering, so a bound, a length or a pattern reads exactly
/// as the boundary states the same constraint on a member it does walk.
pub fn constraint_requirement(kind: ViolationKind) -> String {
    let violation = Violation {
        path: String::new(),
        kind,
        value: None,
        sensitive: true,
    };
    let rendered = violation.message();
    match rendered.rsplit_once(CONSTRAINT_SEPARATOR) {
        Some((_, requirement)) => service_requirement(&violation.kind, requirement),
        None => rendered,
    }
}

/// The members whose value a refusal WITHHOLDS, spelled out so the rule's scope is
/// visible — every one of them is an input member this service's model constrains, in
/// the lowerCamel spelling a validation path uses, lowercased for comparison:
///
/// * `datastoreName` and `jobName` carry a caller-chosen NAME: free text a caller
///   composed, which in a clinical service can carry patient detail.
/// * `kmsKeyArn` names an encryption key, which is credential-adjacent.
/// * `copiableAttributes` carries a caller-composed JSON document naming the SOP
///   instances a copy takes, which the model marks `@sensitive`
///   (`smithy_query(query="shape",shape="MetadataCopies")`) — the model itself saying
///   the value must not be reproduced.
/// * `updatableAttributes` and `removableAttributes` carry a caller-composed document of
///   DICOM attributes, which the model marks `@sensitive` the same way
///   (`smithy_query(query="shape",shape="DICOMUpdates")`): both target `DICOMAttribute`,
///   so a document of patient detail is never reproduced in an error body.
///
/// A member that names another AWS resource (`lambdaAuthorizerArn`, `dataAccessRoleArn`)
/// is in neither class and keeps its echoed value — which is what the recorded corpus
/// shows: the clauses about the data store's name and its KMS key carry no value where
/// the one about the authorizer's function ARN carries it. The resource IDENTIFIERS the
/// request URI carries are their own class, keyed on the whole validation path rather
/// than on the member name ([`URI_LABEL_MEMBERS`]).
const VALUE_WITHHOLDING_MEMBERS: &[&str] = &[
    "datastorename",
    "jobname",
    "kmskeyarn",
    "copiableattributes",
    "updatableattributes",
    "removableattributes",
];

/// The input members this service's model binds to the request URI's PATH
/// (`@httpLabel`), in the same lowercased spelling — the resource identifiers a request
/// addresses a resource by.
///
/// A clause about one of them carries NO value: the recorded corpus words the refusal of
/// a `datastoreId` outside its `@pattern` as `datastoreId failed to satisfy constraint:
/// …`, with no `Value '<v>' at '<path>'` prefix — the rejected value is the URI the
/// caller addressed the request to rather than something a member of the request held.
/// Every `@httpLabel` of this service is listed (`smithy_query(query="digest")`:
/// `datastoreId`, `imageSetId`, `sourceImageSetId`, `jobId`, and the `resourceArn` the
/// tagging operations take), so the whole class is worded the one way rather than only
/// the member the corpus happens to record.
///
/// Matched on the WHOLE validation path, which is why this is a separate list from
/// [`VALUE_WITHHOLDING_MEMBERS`]: an `@httpLabel` is always a top-level member of the
/// input, while a member of a nested payload that shares its name
/// (`copyImageSetInformation.destinationImageSet.imageSetId`) is a value the request
/// CARRIED, and keeps its echoed value.
const URI_LABEL_MEMBERS: &[&str] = &[
    "datastoreid",
    "imagesetid",
    "sourceimagesetid",
    "jobid",
    "resourcearn",
];

/// The DICOM attribute members, in the same lowercased spelling. The model marks every
/// one of them `@sensitive`
/// (`smithy_query(query="shape",shape="SearchByAttributeValue,DICOMStudyDateAndTime")`:
/// the five search attributes plus the study date and time), which is the model itself
/// saying the value must not be reproduced — so a refusal about one withholds it, by the
/// same rule. Listed one by one rather than matched by prefix, so the scope is exactly
/// the `@sensitive` set the model declares and a future non-sensitive `DICOM*` member
/// does not silently join it.
const SENSITIVE_DICOM_MEMBERS: &[&str] = &[
    "dicompatientid",
    "dicomaccessionnumber",
    "dicomstudyid",
    "dicomstudyinstanceuid",
    "dicomseriesinstanceuid",
    "dicomstudydate",
    "dicomstudytime",
];

/// Whether a refusal about `path` withholds the value it refused.
///
/// Keyed on the MEMBER — the last segment of the validation path — never on what the
/// caller sent: the same member withholds its value whatever the violation and whatever
/// the value was. The resource identifiers the URI binds are keyed on the WHOLE path
/// instead, for the reason [`URI_LABEL_MEMBERS`] states.
fn withholds_value(path: &str) -> bool {
    let whole = path.to_ascii_lowercase();
    if URI_LABEL_MEMBERS.contains(&whole.as_str()) {
        return true;
    }
    let member = path.rsplit('.').next().unwrap_or(path).to_ascii_lowercase();
    VALUE_WITHHOLDING_MEMBERS.contains(&member.as_str())
        || SENSITIVE_DICOM_MEMBERS.contains(&member.as_str())
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
/// it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `ValidationException` message for one or more violations: the AWS envelope
/// (`N validation error(s) detected: `) over one [`constraint_clause`] each, in the
/// order they were accumulated. `None` when there is nothing to state.
pub fn validation_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(constraint_clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The envelope for a single violation a HANDLER found itself, so a refusal raised past
/// the boundary validator reads exactly like one the boundary stated. Delegates to
/// [`validation_envelope`], so the envelope text is spelled in one place.
pub fn one_validation_error(violation: &Violation) -> String {
    validation_envelope(::std::slice::from_ref(violation)).unwrap_or_default()
}

/// The `ValidationException` message for a requirement no [`ViolationKind`] states —
/// one a HANDLER checks itself, such as a member the model documents as holding a JSON
/// document, or two members that cannot hold together. Worded through the same envelope
/// and the same `<subject> failed to satisfy constraint: <requirement>` clause as every
/// refusal the boundary validator states, so the two read alike.
///
/// `value` is the rejected value, and is echoed under exactly the rule the boundary
/// refusals follow: withheld for a member [`withholds_value`] covers, and withheld when
/// the caller passes `None` because the requirement is about the member's presence or
/// its relation to another member rather than about what it held.
pub fn handler_requirement_error(path: &str, value: Option<&str>, requirement: &str) -> String {
    let clause = match value {
        Some(shown) if !withholds_value(path) => {
            format!("Value '{shown}' at '{path}'{CONSTRAINT_SEPARATOR}{requirement}")
        }
        _ => format!("{path}{CONSTRAINT_SEPARATOR}{requirement}"),
    };
    format!("1 validation error detected: {clause}")
}

/// The requirement a member documented as carrying a document in the shape of the
/// aggregated DICOM metadata document states when its text does not parse as JSON at all.
pub const METADATA_DOCUMENT_REQUIREMENT: &str = "Member must be a JSON document";

/// The requirement that same member states when its text parses as JSON but is not an
/// OBJECT — a scalar, `null` or an array names no level of the document and so names
/// neither an attribute nor a SOP instance.
pub const METADATA_OBJECT_REQUIREMENT: &str = "Member must be a JSON object of DICOM attributes";

/// The aggregated-metadata document `text` carries, or the requirement it fails.
///
/// Every member this service documents as carrying "a document" in the shape of the
/// aggregated metadata document — the object [`METADATA_SCHEMA_VERSION_KEY`],
/// [`METADATA_PATIENT_KEY`] and [`METADATA_STUDY_KEY`] are the levels of, which
/// `GetImageSetMetadata` serves — is read through here, so one rule decides them all: the
/// text has to parse as JSON, and what it parses to has to be an OBJECT, because any other
/// JSON value carries no level of that document and therefore names nothing the member is
/// documented to name. The error is the REQUIREMENT the value failed, for the caller to
/// word through [`handler_requirement_error`] with the path of the member it is about; the
/// value itself is never part of it, since every member read this way is `@sensitive`.
pub fn metadata_document(
    text: &[u8],
) -> Result<::serde_json::Map<String, ::serde_json::Value>, &'static str> {
    match ::serde_json::from_slice::<::serde_json::Value>(text) {
        Ok(::serde_json::Value::Object(document)) => Ok(document),
        Ok(_) => Err(METADATA_OBJECT_REQUIREMENT),
        Err(_) => Err(METADATA_DOCUMENT_REQUIREMENT),
    }
}

/// The validation path of ONE ELEMENT of an input list, for a refusal a handler raises
/// about that element.
///
/// A list element is named by its enclosing member, its position in the list and the
/// literal `member` segment the element's own schema contributes — and the position is
/// 1-BASED, counting from the first element, which is how the boundary validator's
/// walk spells it (`names.2.member`, `keySchema.1.member.keyType`). `position` is the
/// 0-based offset `Iterator::enumerate` yields, so one caller does the `+ 1` for all of
/// them and no handler can report the element before the one it refused.
///
/// A member of a STRUCTURE element is named by appending that member's own segment to
/// this path, exactly as the nested paths above do.
pub fn list_element_path(list_member: &str, position: usize) -> String {
    format!("{list_member}.{}.member", position + 1)
}

/// The `@pattern` of `com.amazonaws.medicalimaging#Arn`
/// (`smithy_query(query="shape",shape="Arn")`), the shape every `resourceArn` path
/// label targets, copied verbatim.
///
/// It is the single source for two things: [`parse_resource_arn`] below, and the way
/// [`crate::service`] recognises a malformed RESOURCE ARN — a refused value is one
/// when the `@pattern` it failed is this one. No other member targets this shape:
/// `kmsKeyArn`, `lambdaAuthorizerArn` and `dataAccessRoleArn` each target their own
/// ARN shape with its own pattern, so they keep the generic constraint refusal.
pub const ARN_PATTERN: &str = "^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$";

/// The ARN partitions [`ARN_PATTERN`] admits: `aws` plus the four suffixed ones
/// (`(-us-gov)|(-iso)|(-iso-b)|(-cn))?`).
pub const ARN_PARTITIONS: [&str; 5] = ["aws", "aws-us-gov", "aws-iso", "aws-iso-b", "aws-cn"];

/// How many tags one resource will hold at once.
///
/// A memory bound, not an AWS quota: the tagging operations persist caller-supplied
/// keys and values in process memory, so without a cap one caller could grow a
/// resource's tag map without limit. Past the cap EVERY operation that installs tags
/// — `CreateDatastore` as much as `TagResource` — is refused with its own modelled
/// `ValidationException` (HTTP 400). The value matches AWS's general per-resource tag
/// limit of 50.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The text this service refuses a request that would leave one resource holding more
/// than [`MAX_TAGS_PER_RESOURCE`] tags with. `requested` is how many tags the refused
/// request accounts for — the count the map it carries would leave a resource holding
/// none of its keys when the map is judged on its own ([`tag_map_refusal`]), and the
/// count the write would actually leave once the resource's own map is in hand. Either
/// way a count computed from the request, so the sentence says which side of the bound
/// the caller landed on without echoing any key.
///
/// One sentence for both write paths: the create path and the tag path enforce one
/// bound, so they state it with one text.
pub fn too_many_tags_message(requested: usize) -> String {
    format!(
        "this emulator holds at most {MAX_TAGS_PER_RESOURCE} tags per resource; this \
         request would leave {requested}"
    )
}

// ---------------------------------------------------------------------------
// What makes a TAG KEY legal
//
// Every rule about a tag KEY is applied here, by every handler that accepts one,
// because the boundary validator applies NONE of them: the validating walk captures a
// collection's ELEMENT constraints only ("Map keys are not captured" —
// `vendor/smithy-macros/src/lib.rs`), so a `TagMap` key is never visited at all, and
// `TagKey`'s `@pattern` opens with a negative lookahead the `regex` crate cannot
// compile, which `pattern_matches`
// (`vendor/smithy-schema-server/src/constraints.rs`) treats as matching. A tag VALUE
// needs nothing here: it IS the map's element, so `TagValue`'s `@length` reaches it
// through the `ElementConstraintsTrait` the boundary already runs.
// ---------------------------------------------------------------------------

/// The `@pattern` the model declares on `TagKey`, the key shape of `TagMap`
/// (`smithy_query(query="shape",shape="TagMap,TagKey")`), copied verbatim: it is the
/// text a refusal quotes, and the two halves applied below are
/// [`TAG_KEY_RESERVED_PREFIX`] and the character class [`is_tag_key_character`] tests
/// membership of.
pub const TAG_KEY_PATTERN: &str = "^(?!aws:)[a-zA-Z+-=._:/]+$";

/// The prefix [`TAG_KEY_PATTERN`]'s leading `(?!aws:)` refuses: the tag namespace AWS
/// reserves for itself. A negative lookahead anchored at the start of an expression says
/// exactly "the value does not start with this", so it is applied as a `starts_with` —
/// and in the one case the model spells it with, not case-insensitively, because the
/// pattern is what the service advertises.
pub const TAG_KEY_RESERVED_PREFIX: &str = "aws:";

/// `TagKey`'s `@length` minimum (len 1..128). Also what [`TAG_KEY_PATTERN`]'s `+`
/// quantifier says: a key is a non-empty run of admitted characters.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// `TagKey`'s `@length` maximum (len 1..128) — the one tag-key rule the pattern does not
/// also state.
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// Whether `character` is one the class `[a-zA-Z+-=._:/]` of [`TAG_KEY_PATTERN`] admits.
///
/// That class admits the ASCII letters, `_`, and — because `+-=` inside a character
/// class is the RANGE from `+` (0x2B) to `=` (0x3D) — every character in that range: the
/// digits, `+`, `,`, `-`, `.`, `/`, `:`, `;`, `<` and `=`. The `.`, `:` and `/` the class
/// also lists one by one are inside that range too, so the three terms below are the
/// whole class. A character class is a MEMBERSHIP test rather than a pattern language, so
/// it is written out: the check then costs one pass over the key with no regex engine on
/// a path that reads request text.
fn is_tag_key_character(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || ('+'..='=').contains(&character)
}

/// Whether `key` satisfies [`TAG_KEY_PATTERN`]: it lies outside AWS's reserved namespace
/// AND it is a non-empty run of characters the class admits — which is exactly what the
/// pattern's lookahead, class and `+` quantifier say between the `^` and the `$`.
pub fn tag_key_matches_pattern(key: &str) -> bool {
    !key.starts_with(TAG_KEY_RESERVED_PREFIX)
        && !key.is_empty()
        && key.chars().all(is_tag_key_character)
}

/// The violation `key` commits as a `TagKey` at `path`, or `None` when it satisfies
/// every constraint the model puts on one.
///
/// The bounds are checked before the pattern, in the order the boundary validator's own
/// leaf walk runs them (`run_leaf_traits`), so a key that breaks both is reported the way
/// the framework would have reported it. The caller words the result through
/// [`one_validation_error`], so a refusal raised here reads like one the boundary stated.
pub fn tag_key_violation(path: &str, key: &str) -> Option<Violation> {
    let length = key.chars().count();
    let kind = if length < TAG_KEY_MIN_LENGTH {
        ViolationKind::MinLength(TAG_KEY_MIN_LENGTH as i64)
    } else if length > TAG_KEY_MAX_LENGTH {
        ViolationKind::MaxLength(TAG_KEY_MAX_LENGTH as i64)
    } else if !tag_key_matches_pattern(key) {
        ViolationKind::Pattern(TAG_KEY_PATTERN.to_string())
    } else {
        return None;
    };
    Some(Violation {
        path: path.to_string(),
        kind,
        value: Some(echoed_value(key)),
        sensitive: false,
    })
}

/// The violation the keys of a supplied `TagMap` commit, or `None` when every key is
/// legal. `member` is the map member's own path (`tags`), which is where the refusal is
/// reported: a key is not a member of the map, so it has no path of its own to be
/// reported at, and the key that failed is carried as the clause's echoed VALUE.
///
/// The keys are examined in sorted order so that a map with more than one illegal key is
/// refused with the same clause every time — a `HashMap`'s iteration order is not
/// stable, and a refusal that varies run to run is not a contract.
pub fn tag_map_violation(member: &str, tags: &HashMap<String, String>) -> Option<Violation> {
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    keys.into_iter()
        .find_map(|key| tag_key_violation(member, key))
}

/// The `ValidationException` message a supplied tag MAP earns, or `None` when the map is
/// installable by itself. `member` is the map member's own path (`tags`), as
/// [`tag_map_violation`] reports it.
///
/// These are the rules of the tag MAP rather than of any operation, so EVERY write path
/// that installs one applies them here — one place, hence one order, so the same map is
/// refused for the same reason whichever operation carries it. They are applied before
/// any state is read, so what a map is refused for never depends on what this emulator
/// holds:
///
/// 1. the ENTRY COUNT first, as the boundary validator checks a collection's size before
///    its elements: a map carrying more than [`MAX_TAGS_PER_RESOURCE`] entries would
///    leave a resource holding more tags than one may hold whatever it holds already, so
///    no resource has to be resolved to refuse it.
/// 2. then the KEYS, against every constraint the model puts on a `TagKey` — which the
///    boundary validator never visits ([`tag_map_violation`]).
///
/// How much room the TARGET resource has left is not a rule of the map: it is a fact
/// about that resource, so the operation that resolves one checks the count the install
/// would leave AFTER this passes.
pub fn tag_map_refusal(member: &str, tags: &HashMap<String, String>) -> Option<String> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Some(too_many_tags_message(tags.len()));
    }
    tag_map_violation(member, tags).map(|violation| one_validation_error(&violation))
}

/// The rejected value as a MESSAGE carries it: itself when short, a
/// character-boundary-safe prefix with an elision when not.
///
/// Mirrors the framework's own elision (`echoed` in
/// `vendor/smithy-schema-server/src/constraints.rs`), which is private to that crate, cap
/// and rendering included. A tag key is a value NO boundary check bounds first — the walk
/// never visits a map key — so without this a caller could turn a one-megabyte key into a
/// one-megabyte message. Rendering ONLY: every check above runs on the original key.
fn echoed_value(value: &str) -> String {
    /// The longest rejected value a clause carries, as the framework caps it.
    const MAX_ECHOED_VALUE: usize = 256;

    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    // Truncate on a char boundary, never mid-UTF-8.
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// `DatastoreStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="DatastoreStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatastoreStatus {
    Creating,
    CreateFailed,
    Active,
    Deleting,
    Deleted,
}

impl DatastoreStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::CreateFailed => "CREATE_FAILED",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CREATING" => Some(Self::Creating),
            "CREATE_FAILED" => Some(Self::CreateFailed),
            "ACTIVE" => Some(Self::Active),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// `JobStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="JobStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    InProgress,
    Completed,
    Failed,
}

impl JobStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "SUBMITTED" => Some(Self::Submitted),
            "IN_PROGRESS" => Some(Self::InProgress),
            "COMPLETED" => Some(Self::Completed),
            "FAILED" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Whether this is a status a job never leaves: the two the model declares as
    /// outcomes of an import, as against the two it is in while the import runs.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// One persisted DICOM import job, owned by the data store it imports into.
#[derive(Debug, Clone)]
pub struct ImportJob {
    pub job_id: String,
    /// `StartDICOMImportJob` leaves `jobName` optional; `DICOMImportJobProperties`
    /// makes it required, so [`ImportJob::reported_name`] supplies the fallback.
    pub job_name: Option<String>,
    pub job_status: JobStatus,
    pub datastore_id: String,
    pub data_access_role_arn: String,
    pub input_s3_uri: String,
    pub output_s3_uri: String,
    pub input_owner_account_id: Option<String>,
    pub import_configuration: Option<ImportConfiguration>,
    pub submitted_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    /// The `message` this job's terminal status is to carry, decided when its input was
    /// read: `None` for a job that is to COMPLETE
    /// ([`import_job_output::outcome_of`]). Reported only once the job has FAILED
    /// ([`ImportJob::reported_message`]), because the model documents `message` as "The
    /// error message thrown if an import job fails".
    pub failure: Option<String>,
    /// The job output this job still owes the prefix it names, or `None` when it owes
    /// none — either because the delivery has been made or because nothing was
    /// established about its input at all. A job that owes one does not reach its
    /// terminal status until it is delivered, so the manifests exist before any read
    /// reports the job finished ([`import_job_output`]).
    pub output_owed: Option<import_job_output::JobOutput>,
    /// When the job is due to leave `SUBMITTED`.
    pub in_progress_due_at: SystemTime,
    /// When the job is due to reach its terminal status.
    pub completed_due_at: SystemTime,
}

impl ImportJob {
    /// The job name `DICOMImportJobProperties` reports. The properties shape makes
    /// `jobName` `@required` while the start request leaves it optional, so a job
    /// submitted without one reports its own identifier — which satisfies the member's
    /// `^[A-Za-z0-9._/#-]+$` pattern.
    pub fn reported_name(&self) -> String {
        self.job_name.clone().unwrap_or_else(|| self.job_id.clone())
    }

    /// The `message` this job reports: "The error message thrown if an import job fails"
    /// (`smithy_query(query="shape",shape="DICOMImportJobProperties")`), so it is the
    /// message of a job that HAS failed and nothing else — a job still running states no
    /// outcome, and a job that completed has no error to state.
    pub fn reported_message(&self) -> Option<String> {
        (self.job_status == JobStatus::Failed)
            .then(|| self.failure.clone())
            .flatten()
    }

    /// Materialise every status change the clock owed this job by `now`.
    fn advance_to(&mut self, now: SystemTime) {
        if self.job_status.is_terminal() {
            return;
        }
        if now >= self.in_progress_due_at && self.job_status == JobStatus::Submitted {
            self.job_status = JobStatus::InProgress;
        }
        // A job that still owes its output keeps the non-terminal status it is in until
        // that delivery is made: the clock cannot write an object into another service,
        // and a job reported finished whose manifests are not there would be dressing
        // absence as success. The delivery is claimed by the job's own timer or by the
        // next read that finds it due ([`import_job_output::deliver_due_outputs`]), and
        // the status it settles is the one the clock owed at `completed_due_at`.
        if now >= self.completed_due_at && self.output_owed.is_none() {
            self.settle_terminal();
        }
    }
}

/// The `contentType` `GetImageFrame` reports for pixel data whose stored transfer
/// syntax the response documentation does not list — the documented default
/// ("Default is `application/octet-stream`").
pub const DEFAULT_IMAGE_FRAME_CONTENT_TYPE: &str = "application/octet-stream";

/// The `contentType` `GetImageFrame` reports when the stored transfer syntax is
/// MISSING, which the documentation gives the same answer as UID
/// `1.2.840.10008.1.2.4.202`.
pub const MISSING_TRANSFER_SYNTAX_CONTENT_TYPE: &str = "image/jph";

/// Every stored DICOM transfer syntax UID the `GetImageFrame` response documents a
/// `contentType` for, with the content type it documents for it.
///
/// Transcribed from the member's own documentation —
/// `smithy_query(query="doc",shape="GetImageFrameResponse",member="contentType")` —
/// including `video/H256` for HEVC/H.265, which is how AWS spells it there. A UID this
/// table does not list falls back to [`DEFAULT_IMAGE_FRAME_CONTENT_TYPE`].
pub const IMAGE_FRAME_CONTENT_TYPES: [(&str, &str); 22] = [
    ("1.2.840.10008.1.2.1", "application/octet-stream"),
    ("1.2.840.10008.1.2.4.50", "image/jpeg"),
    ("1.2.840.10008.1.2.4.91", "image/j2c"),
    // MPEG2.
    ("1.2.840.10008.1.2.4.100", "video/mpeg"),
    ("1.2.840.10008.1.2.4.100.1", "video/mpeg"),
    ("1.2.840.10008.1.2.4.101", "video/mpeg"),
    ("1.2.840.10008.1.2.4.101.1", "video/mpeg"),
    // MPEG-4 AVC/H.264.
    ("1.2.840.10008.1.2.4.102", "video/mp4"),
    ("1.2.840.10008.1.2.4.102.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.103", "video/mp4"),
    ("1.2.840.10008.1.2.4.103.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.104", "video/mp4"),
    ("1.2.840.10008.1.2.4.104.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.105", "video/mp4"),
    ("1.2.840.10008.1.2.4.105.1", "video/mp4"),
    ("1.2.840.10008.1.2.4.106", "video/mp4"),
    ("1.2.840.10008.1.2.4.106.1", "video/mp4"),
    // HEVC/H.265.
    ("1.2.840.10008.1.2.4.107", "video/H256"),
    ("1.2.840.10008.1.2.4.108", "video/H256"),
    // High-Throughput JPEG 2000.
    ("1.2.840.10008.1.2.4.202", "image/jph"),
    ("1.2.840.10008.1.2.4.203", "image/jphc"),
    // JPEG XL.
    ("1.2.840.10008.1.2.4.112", "image/jxl"),
];

/// The `contentType` `GetImageFrame` reports for pixel data STORED in
/// `transfer_syntax_uid`: what [`IMAGE_FRAME_CONTENT_TYPES`] documents for it, the
/// missing-transfer-syntax answer when the frame declares none, and the documented
/// default for a UID the table does not list.
pub fn image_frame_content_type(transfer_syntax_uid: Option<&str>) -> &'static str {
    let Some(uid) = transfer_syntax_uid else {
        return MISSING_TRANSFER_SYNTAX_CONTENT_TYPE;
    };
    IMAGE_FRAME_CONTENT_TYPES
        .iter()
        .find(|(documented, _)| *documented == uid)
        .map_or(DEFAULT_IMAGE_FRAME_CONTENT_TYPE, |(_, content_type)| {
            *content_type
        })
}

/// One image frame — the pixel data of one frame of one image set, as it was stored.
///
/// Pixel data belongs to the image SET rather than to one of its versions:
/// `GetImageFrame` takes no version, so every version of an image set reaches the same
/// frames.
#[derive(Debug, Clone)]
pub struct ImageFrame {
    /// `ImageFrameId` (`^[0-9a-z]{32}$`) — the identifier this frame is reached by.
    pub image_frame_id: String,
    /// The SOP Instance UID of the DICOM instance this frame's pixel data came from,
    /// `None` when whatever stored the pixel data recorded no instance.
    ///
    /// Frames are keyed by image frame id rather than by instance, because
    /// `GetImageFrame` takes no instance — this is what lets the aggregated metadata
    /// document place the pixel data an image set holds in the patient / study / series /
    /// instance hierarchy it belongs to. A frame stored without one is still served by
    /// `GetImageFrame`; it just appears under no instance in that document, because
    /// nothing recorded which instance it belongs to.
    ///
    /// Carried wherever a frame is: a copy clones the frames it delivers, so the
    /// destination reports them under the same instances, and a DICOM import records the
    /// instance each frame's pixel data was read from ([`imported_image_sets`]).
    pub sop_instance_uid: Option<String>,
    /// The DICOM transfer syntax UID the pixel data is STORED in, `None` when the
    /// instance it came from declared none. `GetImageFrame`'s `contentType` is computed
    /// from it (see [`image_frame_content_type`]); the bytes themselves are never
    /// transcoded.
    pub transfer_syntax_uid: Option<String>,
    /// The pixel data, byte for byte as it was stored.
    pub pixel_data: Vec<u8>,
}

/// `ImageSetState`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="ImageSetState")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSetState {
    Active,
    Locked,
    Deleted,
}

impl ImageSetState {
    /// The wire form of this state.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Locked => "LOCKED",
            Self::Deleted => "DELETED",
        }
    }

    /// Parse a wire state, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ACTIVE" => Some(Self::Active),
            "LOCKED" => Some(Self::Locked),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// `ImageSetWorkflowStatus`, exactly the values the model declares
/// (`smithy_query(query="shape",shape="ImageSetWorkflowStatus")`) — what last happened
/// to an image set, as against [`ImageSetState`], which is whether it can be read and
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSetWorkflowStatus {
    Created,
    Copied,
    Copying,
    CopyingWithReadOnlyAccess,
    CopyFailed,
    Updating,
    UpdatingForStudyConsistency,
    Updated,
    UpdateFailed,
    Deleting,
    Deleted,
    Importing,
    Imported,
    ImportFailed,
}

impl ImageSetWorkflowStatus {
    /// The wire form of this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Copied => "COPIED",
            Self::Copying => "COPYING",
            Self::CopyingWithReadOnlyAccess => "COPYING_WITH_READ_ONLY_ACCESS",
            Self::CopyFailed => "COPY_FAILED",
            Self::Updating => "UPDATING",
            Self::UpdatingForStudyConsistency => "UPDATING_FOR_STUDY_CONSISTENCY",
            Self::Updated => "UPDATED",
            Self::UpdateFailed => "UPDATE_FAILED",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
            Self::Importing => "IMPORTING",
            Self::Imported => "IMPORTED",
            Self::ImportFailed => "IMPORT_FAILED",
        }
    }

    /// Parse a wire status, or `None` when the string names no modelled value.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "CREATED" => Some(Self::Created),
            "COPIED" => Some(Self::Copied),
            "COPYING" => Some(Self::Copying),
            "COPYING_WITH_READ_ONLY_ACCESS" => Some(Self::CopyingWithReadOnlyAccess),
            "COPY_FAILED" => Some(Self::CopyFailed),
            "UPDATING" => Some(Self::Updating),
            "UPDATING_FOR_STUDY_CONSISTENCY" => Some(Self::UpdatingForStudyConsistency),
            "UPDATED" => Some(Self::Updated),
            "UPDATE_FAILED" => Some(Self::UpdateFailed),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            "IMPORTING" => Some(Self::Importing),
            "IMPORTED" => Some(Self::Imported),
            "IMPORT_FAILED" => Some(Self::ImportFailed),
            _ => None,
        }
    }
}

/// One VERSION of an image set — what `GetImageSet`, `ListImageSetVersions` and the
/// copy properties report.
///
/// State and workflow status live HERE and nowhere else: an image set's current state
/// is its latest version's, so the two can never disagree.
#[derive(Debug, Clone)]
pub struct ImageSetVersion {
    /// The external version identifier, `1` for an image set's first version. The wire
    /// form is `ImageSetExternalVersionId` (`^\d+$`), rendered by
    /// [`ImageSetVersion::wire_version_id`].
    pub version_id: u32,
    pub image_set_state: ImageSetState,
    pub workflow_status: ImageSetWorkflowStatus,
    /// When this version last changed. Set from the instant the change fell DUE, never
    /// from `now`, so it does not move between reads.
    pub updated_at: SystemTime,
    /// `Overrides.forced`: whether the `force` flag was used when creating this
    /// version, which is exactly what the model says the member reports.
    pub forced: bool,
    /// `message` — the error message an image set action failed with, absent otherwise.
    pub message: Option<String>,
    /// The DICOM attributes this version held, recorded at the instant a LATER version
    /// superseded it — what reverting the image set to this version puts back.
    ///
    /// `None` on the LATEST version, whose attributes are the ones the image set holds
    /// now ([`ImageSet::dicom_tags`], [`ImageSet::extra_attributes`]), so a version and
    /// the image set can never disagree about the content the image set is serving.
    pub superseded_attributes: Option<ImageSetAttributes>,
}

/// The DICOM attributes one image set holds, in the two parts this service keeps them in.
///
/// The model NAMES sixteen of them (`DICOMTags`), which is what `SearchImageSets` filters
/// on and reports; DICOM itself defines thousands, and a metadata update may write any of
/// them. So an attribute the model does not name is persisted too — under the path of the
/// aggregated metadata document it was written at — rather than accepted and dropped.
#[derive(Debug, Clone, Default)]
pub struct ImageSetAttributes {
    pub dicom_tags: DICOMTags,
    /// Every attribute the model does not name, nested exactly as the aggregated
    /// metadata document nests it (`Patient` → `DICOM` → keyword, `Study` → `Series` →
    /// series UID → …), so what is reported is what was written and nothing besides.
    pub extra_attributes: ::serde_json::Map<String, ::serde_json::Value>,
}

impl ImageSetAttributes {
    /// DELIVER `delivered` into these attributes: every attribute the delivery carries
    /// lands, taking the place of whatever sat at that attribute's own place, and every
    /// attribute held that the delivery carries nothing for stays where it was.
    ///
    /// The rule a delivery of content follows, in both halves of the attribute state and
    /// for the pixel data alike: content is delivered PER ITEM — a frame lands under the
    /// image frame id it is stored by ([`PendingImageSetTransition::delivered_image_frames`]),
    /// an attribute lands at its own path — so what an image set holds after a delivery is
    /// what was delivered together with what it already held and the delivery did not
    /// describe. Attributes holding nothing take the delivered ones whole, which is what a
    /// freshly minted copy destination reports.
    pub fn merge(&mut self, delivered: ImageSetAttributes) {
        merge_dicom_tags(&mut self.dicom_tags, delivered.dicom_tags);
        merge_written_attributes(&mut self.extra_attributes, delivered.extra_attributes);
    }

    /// How many attributes outside the modelled ones these attributes hold — what
    /// [`MAX_IMAGE_SET_EXTRA_ATTRIBUTES`] bounds, for every writer of them.
    ///
    /// An attribute is a LEAF of the aggregated metadata document: a level of the document
    /// is an object, and anything else is one attribute's value, so an array (the image
    /// frames of one SOP instance) counts as the one attribute it is.
    pub fn written_attributes(&self) -> usize {
        count_written_attributes(&self.extra_attributes)
    }
}

/// The descending half of [`ImageSetAttributes::written_attributes`].
///
/// Walks the levels on an explicit worklist rather than by recursion, for the reason
/// [`merge_written_attributes`] states: the document's nesting is chosen by whoever wrote
/// it, so the descent has to cost heap rather than stack.
fn count_written_attributes(attributes: &::serde_json::Map<String, ::serde_json::Value>) -> usize {
    let mut pending: Vec<&::serde_json::Map<String, ::serde_json::Value>> = vec![attributes];
    let mut leaves = 0;
    while let Some(level) = pending.pop() {
        for value in level.values() {
            match value {
                ::serde_json::Value::Object(nested) => pending.push(nested),
                _ => leaves += 1,
            }
        }
    }
    leaves
}

/// Deliver `delivered` into the modelled attributes `held`, ELEMENT BY ELEMENT: an element
/// the delivery carries takes the place of the one held, and an element it carries nothing
/// for leaves what was held where it was.
///
/// The two study counts are modelled as plain numbers rather than optional ones, so zero is
/// what an image set nothing has stored a count for holds — absent, not a count of none —
/// which is the reading every other caller of these two members gives them.
fn merge_dicom_tags(held: &mut DICOMTags, delivered: DICOMTags) {
    fn text(held: &mut Option<String>, delivered: Option<String>) {
        if delivered.is_some() {
            *held = delivered;
        }
    }
    text(&mut held.dicom_patient_id, delivered.dicom_patient_id);
    text(&mut held.dicom_patient_name, delivered.dicom_patient_name);
    text(
        &mut held.dicom_patient_birth_date,
        delivered.dicom_patient_birth_date,
    );
    text(&mut held.dicom_patient_sex, delivered.dicom_patient_sex);
    text(
        &mut held.dicom_study_instance_uid,
        delivered.dicom_study_instance_uid,
    );
    text(&mut held.dicom_study_id, delivered.dicom_study_id);
    text(
        &mut held.dicom_study_description,
        delivered.dicom_study_description,
    );
    text(&mut held.dicom_study_date, delivered.dicom_study_date);
    text(&mut held.dicom_study_time, delivered.dicom_study_time);
    text(
        &mut held.dicom_accession_number,
        delivered.dicom_accession_number,
    );
    text(
        &mut held.dicom_series_instance_uid,
        delivered.dicom_series_instance_uid,
    );
    text(&mut held.dicom_series_modality, delivered.dicom_series_modality);
    text(
        &mut held.dicom_series_body_part,
        delivered.dicom_series_body_part,
    );
    if delivered.dicom_number_of_study_related_series != 0 {
        held.dicom_number_of_study_related_series = delivered.dicom_number_of_study_related_series;
    }
    if delivered.dicom_number_of_study_related_instances != 0 {
        held.dicom_number_of_study_related_instances =
            delivered.dicom_number_of_study_related_instances;
    }
    if delivered.dicom_series_number.is_some() {
        held.dicom_series_number = delivered.dicom_series_number;
    }
}

/// Deliver `delivered` into the attributes kept as written `held`, ATTRIBUTE BY ATTRIBUTE:
/// a delivered attribute takes the place of whatever was held at its own path, and a path
/// the delivery says nothing about keeps what was written there.
///
/// Descends wherever both sides hold a LEVEL of the document, so a level the delivery
/// describes in part gains what it carries without losing the rest, and a level it does not
/// mention at all is untouched. Where one side holds a level and the other an attribute's
/// value, the delivered shape stands: what a path names is decided by what was last written
/// there, exactly as writing an attribute into the document decides it.
///
/// The descent walks the levels on an explicit stack rather than by recursion, because
/// how deeply the document NESTS is chosen by whoever wrote it: a level costs an entry in
/// a heap-allocated vector, which a request can grow without touching the thread stack,
/// while a recursive descent would spend one stack frame per level and a deep enough
/// document would exhaust the stack — an abort no request can be answered around.
fn merge_written_attributes(
    held: &mut ::serde_json::Map<String, ::serde_json::Value>,
    delivered: ::serde_json::Map<String, ::serde_json::Value>,
) {
    /// One level of the descent: the level being merged, the delivered entries it has
    /// still to take, and the name it sits under one level up (`None` for the document
    /// itself, which lands back in `held`).
    struct Level {
        name: Option<String>,
        held: ::serde_json::Map<String, ::serde_json::Value>,
        delivered: ::serde_json::map::IntoIter,
    }

    let mut descent = vec![Level {
        name: None,
        held: ::std::mem::take(held),
        delivered: delivered.into_iter(),
    }];

    while !descent.is_empty() {
        let standing = descent.len() - 1;
        let Some((name, delivered_value)) = descent[standing].delivered.next() else {
            // This level is merged: it lands where it belongs, one level up.
            let finished = descent.pop().expect("the level the descent is standing on");
            match finished.name {
                // A nested level lands back under the name it sits at.
                Some(name) => {
                    let parent = descent
                        .last_mut()
                        .expect("a named level sits inside the level above it");
                    parent
                        .held
                        .insert(name, ::serde_json::Value::Object(finished.held));
                }
                // The root level IS the merged document.
                None => *held = finished.held,
            }
            continue;
        };
        match (descent[standing].held.remove(&name), delivered_value) {
            (
                Some(::serde_json::Value::Object(held_level)),
                ::serde_json::Value::Object(delivered_level),
            ) => descent.push(Level {
                name: Some(name),
                held: held_level,
                delivered: delivered_level.into_iter(),
            }),
            (_, delivered_value) => {
                descent[standing].held.insert(name, delivered_value);
            }
        }
    }
}

impl ImageSetVersion {
    /// The version identifier as `ImageSetExternalVersionId` spells it (`^\d+$`).
    pub fn wire_version_id(&self) -> String {
        self.version_id.to_string()
    }
}

/// A change to an image set the CLOCK owes — the copy a `CopyImageSet` call started,
/// which lands on the latest version once [`IMAGE_SET_COPY_DELAY`] has passed.
#[derive(Debug, Clone)]
pub struct PendingImageSetTransition {
    pub due_at: SystemTime,
    pub image_set_state: ImageSetState,
    pub workflow_status: ImageSetWorkflowStatus,
    /// The pixel data this transition DELIVERS when it lands, keyed by image frame id —
    /// the frames a copy in flight is writing into its destination. `None` for a
    /// transition that delivers none, such as the release of the copy's read-only lock
    /// on its source. Nothing is readable through `GetImageFrame` before the transition
    /// that delivers it falls due, because nothing holds it yet.
    pub delivered_image_frames: Option<HashMap<String, ImageFrame>>,
    /// The DICOM attributes this transition DELIVERS when it lands, by the same rule as
    /// [`PendingImageSetTransition::delivered_image_frames`]: a copy carries its
    /// source's DICOM content to its destination, so the destination answers a search
    /// over those attributes exactly when the copy that delivered them completed, and
    /// not before. Delivered attribute by attribute, as the frames are delivered frame
    /// by frame ([`ImageSet::merge_attributes`]) — these are the attributes the delivery
    /// DESCRIBES, not the whole attribute state the image set is to hold.
    pub delivered_attributes: Option<ImageSetAttributes>,
    /// Whether this transition RETIRES the image set — the landing of the delete a
    /// `DeleteImageSet` call started. The record stays readable afterwards, because
    /// `deletedAt` ("the timestamp when the image set properties were deleted") is a
    /// reported member, but the CONTENT goes with the deletion — pixel data and DICOM
    /// attributes alike ([`ImageSet::release_content`]): nothing holds it once the image
    /// set is gone.
    pub retires_image_set: bool,
}

/// One persisted image set, with every version of it the data store holds.
#[derive(Debug, Clone)]
pub struct ImageSet {
    /// The tags `TagResource` / `ListTagsForResource` keep for this image set, which
    /// an ARN reaches through its optional `/imageset/<id>` suffix (see
    /// [`ARN_PATTERN`]).
    pub tags: HashMap<String, String>,
    pub image_set_id: String,
    pub image_set_arn: String,
    /// The newest version. Held apart from [`ImageSet::earlier_versions`] so that
    /// "an image set has at least one version" is a TYPE invariant: no reader has to
    /// handle an empty version list, and no handler can create an image set without
    /// one.
    pub latest_version: ImageSetVersion,
    /// Every superseded version, oldest first.
    pub earlier_versions: Vec<ImageSetVersion>,
    /// The pixel data this image set holds, keyed by `ImageFrameId` — what
    /// `GetImageFrame` serves. Empty for an image set no frame has been delivered to
    /// yet.
    pub image_frames: HashMap<String, ImageFrame>,
    /// Whether this image set is in the primary DICOM hierarchy (`isPrimary`).
    pub is_primary: bool,
    /// The DICOM attributes of the patient, study and series this image set holds —
    /// what `SearchImageSets` filters on and reports in each
    /// `ImageSetsMetadataSummary`. Every member is absent (and the two study counts
    /// zero) for an image set nothing has delivered DICOM content to yet, exactly as it
    /// holds no pixel data: an attribute is searchable only once something stored it.
    pub dicom_tags: DICOMTags,
    /// The attributes of this image set the model does NOT name, which a metadata update
    /// may write and `GetImageSetMetadata` reports from where they were written — the
    /// other half of [`ImageSetAttributes`], held beside the modelled attributes rather
    /// than inside them so every reader of `dicom_tags` keeps reading what it did.
    pub extra_attributes: ::serde_json::Map<String, ::serde_json::Value>,
    pub created_at: SystemTime,
    pub deleted_at: Option<SystemTime>,
    /// When the DICOM content this image set holds was last ACCESSED — what
    /// `lastAccessedAt` reports. An image set nothing has accessed since it came into
    /// existence carries the instant it was created; every access of its content —
    /// pixel data served to a caller, content delivered into it — moves it forward
    /// through [`ImageSet::record_access`]. Reporting an image set's PROPERTIES is not
    /// an access of the content it holds, so no reader of those moves it.
    pub last_accessed_at: SystemTime,
    /// What the clock owes the latest version, if anything.
    pub pending: Option<PendingImageSetTransition>,
}

impl ImageSet {
    /// A freshly minted image set at version [`FIRST_IMAGE_SET_VERSION_ID`], in the
    /// state and workflow status its creator puts it in.
    ///
    /// The ONE place an image set record is built, so every creator — the copy's
    /// destination today, a DICOM import later — agrees on the invariants.
    pub fn new(
        image_set_id: String,
        image_set_arn: String,
        image_set_state: ImageSetState,
        workflow_status: ImageSetWorkflowStatus,
        is_primary: bool,
        forced: bool,
        now: SystemTime,
    ) -> Self {
        Self {
            tags: HashMap::new(),
            image_set_id,
            image_set_arn,
            latest_version: ImageSetVersion {
                version_id: FIRST_IMAGE_SET_VERSION_ID,
                image_set_state,
                workflow_status,
                updated_at: now,
                forced,
                message: None,
                // The latest version's attributes are the image set's own; a snapshot is
                // recorded only when a later version supersedes this one.
                superseded_attributes: None,
            },
            earlier_versions: Vec::new(),
            // A freshly minted image set holds no pixel data: a frame is readable only
            // once something has delivered it (the copy that completes, an import).
            image_frames: HashMap::new(),
            is_primary,
            // No DICOM content either, for the same reason the frame map is empty.
            dicom_tags: DICOMTags::default(),
            // Nor any attribute outside the modelled ones: nothing has written one.
            extra_attributes: ::serde_json::Map::new(),
            created_at: now,
            deleted_at: None,
            // Nothing has accessed the content of an image set that has only just come
            // into existence, so the instant it appeared is the last access of it.
            last_accessed_at: now,
            pending: None,
        }
    }

    /// Every version this image set holds, oldest first — the order
    /// `ListImageSetVersions` reports them in.
    pub fn versions(&self) -> impl Iterator<Item = &ImageSetVersion> {
        self.earlier_versions
            .iter()
            .chain(::std::iter::once(&self.latest_version))
    }

    /// The version `wire_version_id` (`^\d+$`) names, or `None` when this image set
    /// holds no such version.
    pub fn version(&self, wire_version_id: &str) -> Option<&ImageSetVersion> {
        self.versions()
            .find(|version| version.wire_version_id() == wire_version_id)
    }

    /// Whether this image set has been deleted — the terminal state a delete lands on,
    /// after which the record is a tombstone: it still reports its properties and its
    /// `deletedAt`, but holds none of the content it held
    /// ([`ImageSet::release_content`]) and takes part in no further work.
    pub fn is_deleted(&self) -> bool {
        self.latest_version.image_set_state == ImageSetState::Deleted
    }

    /// Whether a DELETE of this image set is already in flight, i.e. the clock owes it
    /// the move to `DELETED`. What makes `DeleteImageSet` idempotent: a repeat call
    /// reports the same `LOCKED` / `DELETING` pair without moving the instant the
    /// deletion falls due.
    pub fn deletion_in_flight(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.retires_image_set)
    }

    /// Record that the DICOM content this image set holds was accessed at `at` — pixel
    /// data served from it, or content delivered into it.
    ///
    /// MONOTONIC: the reported instant never moves backwards, so an access recorded at
    /// the instant it fell due is never overwritten by an earlier one, and what
    /// `lastAccessedAt` reports is always the LAST access.
    pub fn record_access(&mut self, at: SystemTime) {
        self.last_accessed_at = self.last_accessed_at.max(at);
    }

    /// The image frame `image_frame_id` names, or `None` when this image set holds no
    /// pixel data under that identifier.
    pub fn image_frame(&self, image_frame_id: &str) -> Option<&ImageFrame> {
        self.image_frames.get(image_frame_id)
    }

    /// The latest version identifier, as the wire spells it.
    pub fn latest_version_id(&self) -> String {
        self.latest_version.wire_version_id()
    }

    /// Supersede the latest version with a new one, numbered one past it, and answer
    /// the new version's identifier. The old latest becomes the newest earlier version,
    /// so a reader naming it still gets what it reported before.
    ///
    /// The superseded version keeps the DICOM attributes the image set holds AT THIS
    /// INSTANT ([`ImageSetVersion::superseded_attributes`]): whatever content the new
    /// version goes on to carry, the version it replaced still reports the content it
    /// was serving, which is what a revert to that version restores.
    ///
    /// `image_set_state` is the state the IMAGE SET is in from this instant, so it reaches
    /// every version ([`ImageSet::set_state_on_every_version`]); `workflow_status` — what
    /// last happened — is recorded on the new version alone.
    pub fn push_version(
        &mut self,
        image_set_state: ImageSetState,
        workflow_status: ImageSetWorkflowStatus,
        forced: bool,
        now: SystemTime,
    ) -> u32 {
        let version_id = self.latest_version.version_id.saturating_add(1);
        let superseded_attributes = self.attributes();
        let mut superseded = ::std::mem::replace(
            &mut self.latest_version,
            ImageSetVersion {
                version_id,
                image_set_state,
                workflow_status,
                updated_at: now,
                forced,
                message: None,
                superseded_attributes: None,
            },
        );
        superseded.superseded_attributes = Some(superseded_attributes);
        self.earlier_versions.push(superseded);
        // The one writer of the member, so the version just built and the versions behind
        // it cannot disagree about the state the image set is in. (`ImageSetVersion` names
        // the member as required, so the literal above has to carry it too.)
        self.set_state_on_every_version(image_set_state);
        version_id
    }

    /// The DICOM attributes this image set holds right now — the latest version's.
    pub fn attributes(&self) -> ImageSetAttributes {
        ImageSetAttributes {
            dicom_tags: self.dicom_tags.clone(),
            extra_attributes: self.extra_attributes.clone(),
        }
    }

    /// Replace the DICOM attributes this image set holds, both halves together, so the
    /// modelled attributes a search filters on and the attributes a metadata document
    /// reports always describe the same content.
    ///
    /// For a caller that computed the WHOLE attribute state the image set is to hold. A
    /// DELIVERY of content into the image set carries only the attributes it describes, so
    /// it lands through [`ImageSet::merge_attributes`] instead.
    pub fn set_attributes(&mut self, attributes: ImageSetAttributes) {
        self.dicom_tags = attributes.dicom_tags;
        self.extra_attributes = attributes.extra_attributes;
    }

    /// DELIVER `delivered` into the DICOM attributes this image set holds
    /// ([`ImageSetAttributes::merge`]): every attribute the delivery carries lands here,
    /// taking the place of whatever sat at that attribute's own place, and every attribute
    /// the image set held that the delivery carries nothing for stays where it was.
    pub fn merge_attributes(&mut self, delivered: ImageSetAttributes) {
        let mut merged = self.attributes();
        merged.merge(delivered);
        self.set_attributes(merged);
    }

    /// The DICOM attributes the version `wire_version_id` (`^\d+$`) held, or `None` when
    /// this image set holds no such version. The latest version's are the ones the image
    /// set holds now; an earlier version's are the snapshot taken when it was superseded.
    pub fn attributes_of_version(&self, wire_version_id: &str) -> Option<ImageSetAttributes> {
        let version = self.version(wire_version_id)?;
        Some(
            version
                .superseded_attributes
                .clone()
                .unwrap_or_else(|| self.attributes()),
        )
    }

    /// Give up the DICOM content this image set holds: the pixel data it was serving,
    /// the DICOM attributes describing it — the modelled ones a search filters on and
    /// the ones held at the path they were written at alike — and the snapshot every
    /// superseded version kept of those attributes.
    ///
    /// What the landing of a deletion leaves behind is a record that reports an image
    /// set's PROPERTIES (its identifiers, its states, its timestamps and the tags kept
    /// on the record) and holds none of the content those properties described, so no
    /// reader can report content of an image set the service reports as deleted.
    pub fn release_content(&mut self) {
        self.image_frames.clear();
        self.set_attributes(ImageSetAttributes::default());
        // A superseded version's snapshot is the content a revert to that version would
        // put back, so it is released with the rest: once the content is given up there
        // is no version of it left to restore. The latest version keeps its `None`,
        // which is the attributes the image set holds — now none.
        for version in &mut self.earlier_versions {
            version.superseded_attributes = Some(ImageSetAttributes::default());
        }
    }

    /// Put EVERY version of this image set into `image_set_state`.
    ///
    /// `ImageSetState` is whether the image set can be read and written, which is a
    /// property of the image set rather than of one snapshot of it, so every change of it
    /// — the lock a write in flight takes, the release of that lock, a retirement — goes
    /// through here and no version is left reporting a state the image set no longer has.
    pub fn set_state_on_every_version(&mut self, image_set_state: ImageSetState) {
        self.latest_version.image_set_state = image_set_state;
        for version in &mut self.earlier_versions {
            version.image_set_state = image_set_state;
        }
    }

    /// Materialise the transition the clock owed this image set by `now`.
    fn advance_to(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        // The state the transition lands reaches EVERY version, by the rule every other
        // writer of it follows ([`ImageSet::set_state_on_every_version`]): it is the state
        // the image set is in, not one version's. The workflow status — what last happened
        // — stays on the version it happened to.
        self.set_state_on_every_version(pending.image_set_state);
        self.latest_version.workflow_status = pending.workflow_status;
        // The due instant, not `now`: `updatedAt` must not move between reads.
        self.latest_version.updated_at = pending.due_at;
        // Content landing IN this image set is an access of it, recorded at the instant
        // the transition that delivers it fell DUE rather than at `now`, so what
        // `lastAccessedAt` reports does not move between reads.
        let delivers_content =
            pending.delivered_image_frames.is_some() || pending.delivered_attributes.is_some();
        if delivers_content {
            self.record_access(pending.due_at);
        }
        // The pixel data the transition owed lands with it, so a frame becomes readable
        // exactly when the copy that delivers it completes. A frame already held under
        // the same identifier is replaced by the delivered one: an image frame id names
        // its pixel data, so the two cannot disagree.
        if let Some(delivered) = pending.delivered_image_frames {
            self.image_frames.extend(delivered);
        }
        // The DICOM attributes the transition owed land with it too, so what
        // `SearchImageSets` filters on and reports for this image set only ever
        // describes content the image set already holds. Delivered attribute by
        // attribute, by the rule the delivered frames follow: a delivery describes the
        // content it carries, so it takes the place of what it describes and leaves
        // what it does not ([`ImageSet::merge_attributes`]).
        if let Some(delivered) = pending.delivered_attributes {
            self.merge_attributes(delivered);
        }
        // A deletion the clock owed lands the same way, at the DUE instant rather than at
        // `now`, so `deletedAt` does not move between reads — and the content this image
        // set held goes with it ([`ImageSet::release_content`]), because a deleted image
        // set holds none.
        if pending.retires_image_set {
            self.deleted_at = Some(pending.due_at);
            self.release_content();
        }
        self.pending = None;
    }
}

/// One persisted data store, with the import jobs and image sets it owns.
#[derive(Debug, Clone)]
pub struct Datastore {
    pub datastore_id: String,
    /// The region this data store EXISTS in: the one the creating request was addressed
    /// to ([`request_region`]), and the one its [`Datastore::datastore_arn`] names.
    ///
    /// A data store is a regional resource, so this is what makes it reachable from
    /// that region and from no other — see
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::datastore_in`].
    pub region: String,
    /// `CreateDatastore` leaves `datastoreName` optional; `DatastoreProperties` and
    /// `DatastoreSummary` make it required, so [`Datastore::reported_name`] supplies
    /// the fallback.
    pub datastore_name: Option<String>,
    pub datastore_status: DatastoreStatus,
    pub datastore_arn: String,
    pub kms_key_arn: Option<String>,
    pub lambda_authorizer_arn: Option<String>,
    pub lossless_storage_format: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the next change this data store owes falls due, if any: the move from
    /// `CREATING` to `ACTIVE`, the move from `DELETING` to `DELETED`, or — once the
    /// deletion has landed — the reaping of the `DELETED` record
    /// ([`DATASTORE_DELETED_RETENTION`]).
    pub status_due_at: Option<SystemTime>,
    pub import_jobs: HashMap<String, ImportJob>,
    pub image_sets: HashMap<String, ImageSet>,
}

impl Datastore {
    /// The data store name `DatastoreProperties` / `DatastoreSummary` report. Both
    /// shapes make `datastoreName` `@required` while the create request leaves it
    /// optional, so a store created without one reports its own identifier — which
    /// satisfies the member's `^[A-Za-z0-9._/#-]+$` pattern.
    pub fn reported_name(&self) -> String {
        self.datastore_name
            .clone()
            .unwrap_or_else(|| self.datastore_id.clone())
    }

    /// Whether this data store has settled into the one status that serves work on it or
    /// on its contents: `ACTIVE`.
    ///
    /// A status change the clock still owes — `CREATING` on the way to `ACTIVE`,
    /// `DELETING` on the way to gone — is exactly what the modelled `ConflictException`
    /// (HTTP 409, "Updating or deleting a resource can cause an inconsistent state")
    /// reports: acting on a store mid-transition would leave the transition and the call
    /// disagreeing about the resource. The model states the same precondition in prose
    /// for `StartDICOMImportJob`: "Start importing bulk data into an ACTIVE data store"
    /// (`smithy_query(query="operation",operation="StartDICOMImportJob")`).
    ///
    /// A terminal status is NOT in flight: a `CREATE_FAILED` store never becomes usable,
    /// but it is still deletable — see [`Datastore::creation_in_flight`].
    pub fn is_active(&self) -> bool {
        self.datastore_status == DatastoreStatus::Active
    }

    /// Whether this data store's CREATION is still in flight (`CREATING`), i.e. the
    /// clock owes it the move to `ACTIVE`.
    ///
    /// What `DeleteDatastore` refuses: a delete admitted here would cancel a creation the
    /// service has already promised, so it is answered with `ConflictException`. Every
    /// other status is deletable — `ACTIVE` because it has settled, `CREATE_FAILED`
    /// because a failed creation is terminal and has to be cleanable, and `DELETING`
    /// because a repeat delete is idempotent.
    pub fn creation_in_flight(&self) -> bool {
        self.datastore_status == DatastoreStatus::Creating
    }

    /// Whether this data store's deletion has LANDED (`DELETED`), i.e. the record is a
    /// tombstone: it still reports its properties in that status until it is reaped
    /// ([`DATASTORE_DELETED_RETENTION`]), but it holds nothing a caller can reach and
    /// takes part in no further work.
    ///
    /// What separates the two accessor families in
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState`]: a tombstone is REPORTABLE
    /// and not reachable.
    pub fn is_deleted(&self) -> bool {
        self.datastore_status == DatastoreStatus::Deleted
    }

    /// Every image set this data store still HOLDS, in no particular order — what the
    /// per-data-store cap on image sets counts ([`MAX_IMAGE_SETS_PER_DATASTORE`]), and
    /// what work that reaches the image sets of a data store at large walks.
    ///
    /// A tombstone occupies no place: an image set whose deletion has landed has given up
    /// the content it held, so it neither takes a place among the image sets the store
    /// holds nor can be worked on ([`ImageSet::is_deleted`]) — the rule
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::datastores_in`] states for the
    /// data stores of a region. Use [`Datastore::image_sets`] directly where the records
    /// themselves are what the call is about: reporting one by its id, or bounding how
    /// many records this emulator holds ([`MAX_IMAGE_SET_RECORDS_PER_DATASTORE`]).
    pub fn held_image_sets(&self) -> impl Iterator<Item = &ImageSet> {
        self.image_sets
            .values()
            .filter(|image_set| !image_set.is_deleted())
    }

    /// Every DICOM import job of this data store that is still RUNNING — one the clock
    /// has not yet moved to a terminal `jobStatus` ([`JobStatus::is_terminal`]).
    ///
    /// What `DeleteDatastore` consults: an import writes image sets INTO the data store,
    /// so deleting the store under a job still in flight would leave the job and the store
    /// disagreeing about the resource, which is the inconsistent state the modelled
    /// `ConflictException` (HTTP 409) reports.
    pub fn running_import_jobs(&self) -> impl Iterator<Item = &ImportJob> {
        self.import_jobs
            .values()
            .filter(|job| !job.job_status.is_terminal())
    }

    /// Materialise every change the clock owed this data store by `now`.
    ///
    /// Returns `false` once the record's `DELETED` retention has run out, i.e. the
    /// record must be dropped from the store.
    fn advance_to(&mut self, now: SystemTime) -> bool {
        // A LOOP, not one step: when a request arrives long after a delete, every
        // change the clock owed in between is already due, so the record lands on
        // `DELETED` and is reaped within this one pass rather than lingering until the
        // next request happens to come along.
        while let Some(due) = self.status_due_at {
            if now < due {
                break;
            }
            match self.datastore_status {
                DatastoreStatus::Creating => {
                    self.datastore_status = DatastoreStatus::Active;
                    // The due instant, not `now`: `updatedAt` must be stable
                    // across reads.
                    self.updated_at = due;
                    self.status_due_at = None;
                }
                DatastoreStatus::Deleting => {
                    self.datastore_status = DatastoreStatus::Deleted;
                    self.updated_at = due;
                    // The finished deletion is reportable in `DELETED` from the instant
                    // it landed, and the reaping that ends that window is the next
                    // change the clock owes — dated from the DUE instant, so neither the
                    // window nor `updatedAt` moves with when somebody happened to read.
                    self.status_due_at = Some(due + DATASTORE_DELETED_RETENTION);
                }
                DatastoreStatus::Deleted => return false,
                _ => self.status_due_at = None,
            }
        }
        for job in self.import_jobs.values_mut() {
            job.advance_to(now);
        }
        for image_set in self.image_sets.values_mut() {
            image_set.advance_to(now);
        }
        true
    }
}

/// Per-service state for com-amazonaws-medicalimaging-ahigatewayservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Every live data store, keyed by its 32-character lowercase-alphanumeric id.
    ///
    /// One map holds every region's stores — an id is minted from one process-wide
    /// counter ([`Self::mint_id`]), so no two regions can name the same one — and a
    /// store is REACHED through [`Self::datastore_in`], which admits only the stores of
    /// the region the request addressed.
    pub datastores: HashMap<String, Datastore>,
    /// `@idempotencyToken` ledger: a scoped client token (see [`client_token_key`])
    /// mapped to the id it minted, so a repeat call returns the SAME resource. Bounded
    /// by [`MAX_CLIENT_TOKENS`]; write it only through
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::remember_client_token`].
    pub client_tokens: HashMap<String, String>,
    /// The ledger's keys in insertion order, which is what makes the eviction in
    /// [`ComAmazonawsMedicalimagingAhigatewayserviceState::remember_client_token`] FIFO.
    pub client_token_order: VecDeque<String>,
    /// Monotonic counter behind [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`].
    pub next_id: u64,
    /// How many import-job output deliveries this service has in flight
    /// ([`import_job_output::MAX_DELIVERIES_IN_FLIGHT`]). PRIVATE, and taken and given
    /// back around one delivery, so a burst of submissions cannot leave an unbounded
    /// number of writes outstanding.
    deliveries_in_flight: usize,
}

impl ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Mint a fresh 32-character lowercase-hex id, which satisfies both
    /// `DatastoreId` (`^[0-9a-z]{32}$`) and `JobId` (`^[0-9a-z]+$`, 1..32).
    ///
    /// Deterministic: the n-th id of a fresh store is always the same, because the
    /// bits come from SplitMix64 over a counter rather than from the system RNG.
    /// SplitMix64 is a bijection on `u64`, so two counter values never collide.
    pub fn mint_id(&mut self) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let high = splitmix64(self.next_id);
        let low = splitmix64(self.next_id ^ 0x5DEE_CE66_D1B5_7A4F);
        format!("{high:016x}{low:016x}")
    }

    /// Record that `key` minted `minted_id`, evicting the oldest token once the ledger
    /// is at [`MAX_CLIENT_TOKENS`], so repeated creates cannot grow it without limit.
    pub fn remember_client_token(&mut self, key: String, minted_id: String) {
        if self.client_tokens.insert(key.clone(), minted_id).is_none() {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                None => break,
            }
        }
    }

    /// The data store `datastore_id` names, as seen from `region` — `None` when this
    /// emulator holds no such data store THERE.
    ///
    /// A data store is a regional resource: it exists only in the region its creating
    /// request was addressed to, so a request addressed to any other region reaches
    /// nothing by its id and is answered as if the resource did not exist. Every
    /// id-keyed read and write goes through here (or through [`Self::datastore_in_mut`]),
    /// so one rule decides reachability for all of them rather than each handler
    /// deciding for itself.
    ///
    /// A data store whose deletion has LANDED is not reached here: its record lives on
    /// only to be REPORTED in `DELETED` ([`Datastore::is_deleted`]), so every call that
    /// would act on it, or on anything it held, is answered exactly as it is for an id
    /// nothing ever minted. Use [`Self::reported_datastore_in`] where the data store's
    /// status is itself what the call reports.
    pub fn datastore_in(&self, region: &str, datastore_id: &str) -> Option<&Datastore> {
        self.reported_datastore_in(region, datastore_id)
            .filter(|datastore| !datastore.is_deleted())
    }

    /// [`Self::datastore_in`], for a writer.
    pub fn datastore_in_mut(&mut self, region: &str, datastore_id: &str) -> Option<&mut Datastore> {
        self.datastores
            .get_mut(datastore_id)
            .filter(|datastore| datastore.region == region && !datastore.is_deleted())
    }

    /// The data store `datastore_id` names, as seen from `region`, INCLUDING one whose
    /// deletion has landed and whose record has not been reaped yet — `None` when this
    /// emulator holds no such record there.
    ///
    /// What an operation whose job is to REPORT a data store's status reads, so a
    /// finished deletion is observable in `DELETED` for as long as the record survives
    /// ([`DATASTORE_DELETED_RETENTION`]) rather than being a status no response can
    /// carry. Everything that ACTS on a data store goes through [`Self::datastore_in`].
    pub fn reported_datastore_in(&self, region: &str, datastore_id: &str) -> Option<&Datastore> {
        self.datastores
            .get(datastore_id)
            .filter(|datastore| datastore.region == region)
    }

    /// Every data store `region` holds that work can still reach, in no particular
    /// order — what the per-region quota counts.
    ///
    /// A tombstone occupies no place: a data store whose deletion has landed has given
    /// up everything it held, so it neither takes a quota slot nor can be worked on.
    pub fn datastores_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Datastore> {
        self.reported_datastores_in(region)
            .filter(|datastore| !datastore.is_deleted())
    }

    /// Every data store RECORD `region` holds, in no particular order, including the
    /// tombstones of finished deletions — the collection a listing enumerates, and the
    /// one its `datastoreStatus` filter selects from, so a filtered and an unfiltered
    /// listing answer about the same population.
    pub fn reported_datastores_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a Datastore> {
        self.datastores
            .values()
            .filter(move |datastore| datastore.region == region)
    }

    /// The tag map of the resource `arn` names — a data store, or one image set it
    /// owns — as seen from `region`, or which resource KIND is missing when this
    /// emulator holds no such resource there.
    ///
    /// What the REPORTING route reads: one lookup for both resource kinds, so the
    /// tagging operations agree on what exists — tags live ON the resource record rather
    /// than in a map beside it — and a record that is reportable still reports the tags
    /// it holds, a retired image set among them. A write goes through
    /// [`Self::writable_resource_tags_mut`], which admits the same records minus the
    /// tombstones.
    /// The ARN must be the resource's OWN: a data store is reached by its id in the
    /// region the request addressed, and then only when the ARN it was named by is the
    /// one the data store reports.
    ///
    /// The failure says WHICH kind is absent — the data store, or the image set inside
    /// one that is present — because that is what a refusal has to state
    /// ([`MissingResource::message`]).
    pub fn resource_tags(
        &self,
        region: &str,
        arn: &ResourceArn,
    ) -> Result<&HashMap<String, String>, MissingResource> {
        let datastore = self
            .datastore_in(region, &arn.datastore_id)
            .filter(|datastore| datastore.datastore_arn == arn.datastore_arn)
            .ok_or(MissingResource::Datastore)?;
        match &arn.image_set_id {
            None => Ok(&datastore.tags),
            Some(image_set_id) => datastore
                .image_sets
                .get(image_set_id)
                .map(|image_set| &image_set.tags)
                .ok_or(MissingResource::ImageSet),
        }
    }

    /// [`Self::resource_tags`], for a WRITER — the tag map of a resource record this
    /// emulator still holds, or which resource KIND it holds none of.
    ///
    /// A tag map is METADATA ON THE RECORD: writing it neither reads nor changes anything
    /// the resource serves, so it is admitted for as long as the record exists, whatever
    /// status change the clock still owes it. Existence is the one thing it answers to:
    ///
    /// * the data store has to be one work can still reach ([`Self::datastore_in_mut`]):
    ///   a record whose deletion has LANDED holds nothing a caller can reach, so it is
    ///   absent to a writer even while it is still reportable.
    /// * an image set the ARN names has to be one that is not retired
    ///   ([`ImageSet::is_deleted`]). A tombstone still REPORTS what it held, but it takes
    ///   part in no further work, so nothing more is written onto it.
    ///
    /// The refusal resolves BEFORE the map is handed out, so a refused write leaves the
    /// tag map exactly as it was.
    pub fn writable_resource_tags_mut(
        &mut self,
        region: &str,
        arn: &ResourceArn,
    ) -> Result<&mut HashMap<String, String>, MissingResource> {
        let datastore = self
            .datastore_in_mut(region, &arn.datastore_id)
            .filter(|datastore| datastore.datastore_arn == arn.datastore_arn)
            .ok_or(MissingResource::Datastore)?;
        match &arn.image_set_id {
            None => Ok(&mut datastore.tags),
            Some(image_set_id) => datastore
                .image_sets
                .get_mut(image_set_id)
                .filter(|image_set| !image_set.is_deleted())
                .map(|image_set| &mut image_set.tags)
                .ok_or(MissingResource::ImageSet),
        }
    }

    /// Materialise every status change the clock owed by `now`, dropping the data
    /// stores whose `DELETED` record the clock has reaped — and, with them, the ledger
    /// entries that named the data store or any of its import jobs, so a create/delete
    /// loop cannot leave entries behind forever. Every handler calls this first, so each
    /// request sees the state the clock already made true.
    pub fn advance_clock(&mut self, now: SystemTime) {
        let mut retired: HashSet<String> = HashSet::new();
        self.datastores.retain(|_, datastore| {
            if datastore.advance_to(now) {
                return true;
            }
            retired.insert(datastore.datastore_id.clone());
            retired.extend(datastore.import_jobs.keys().cloned());
            false
        });
        if retired.is_empty() {
            return;
        }
        self.client_tokens
            .retain(|_, minted_id| !retired.contains(minted_id));
        let live = &self.client_tokens;
        self.client_token_order.retain(|key| live.contains_key(key));
    }
}

/// The ledger key for one `@idempotencyToken`.
///
/// Scoped by REGION, and by operation (and, for an import job, by the data store it runs
/// against), so the same token reused on a different operation — or against a different
/// region, where the resource the first call minted does not exist — mints a different
/// resource rather than returning the first one.
pub fn client_token_key(region: &str, scope: &str, client_token: &str) -> String {
    format!("{region}\u{0}{scope}\u{0}{client_token}")
}

/// SplitMix64's finalizer — a bijection on `u64`, which is what makes
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`] collision-free.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The region this request is signed for, used to build ARNs.
///
/// Read from the SigV4 credential scope (`<akid>/<date>/<region>/<name>/aws4_request`)
/// through the one parser the core vends; an unsigned request falls back to
/// [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a data store in `region`:
/// `arn:aws:medical-imaging:<region>:<account>:datastore/<datastoreId>`.
pub fn datastore_arn(region: &str, datastore_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("datastore/{datastore_id}"))
}

/// The ARN of one image set inside a data store: that data store's OWN ARN with the
/// `/imageset/<imageSetId>` suffix [`ARN_PATTERN`] admits — which is what makes the
/// tagging operations reach the image set through [`ResourceArn`].
pub fn image_set_arn(datastore_arn: &str, image_set_id: &str) -> String {
    format!("{datastore_arn}/imageset/{image_set_id}")
}

/// What a `resourceArn` label names: a data store, or ONE image set inside it —
/// the two resources [`ARN_PATTERN`] can spell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceArn {
    /// The ARN of the data store, i.e. the label minus any `/imageset/<id>` suffix.
    ///
    /// Carried, not just the id, because an ARN names a resource only when it is
    /// THAT resource's ARN: one differing in partition, region or account names a
    /// resource of another account or region, which this emulator does not hold.
    pub datastore_arn: String,
    pub datastore_id: String,
    /// `Some` only when the ARN carries the optional `/imageset/<id>` suffix.
    pub image_set_id: Option<String>,
}

/// Which resource a [`ResourceArn`] lookup failed to find: the data store the ARN
/// names, or the image set inside a data store that IS held.
///
/// It exists so a refusal can state the KIND that is absent — rule 1 of the wording
/// rules above — rather than anything the caller supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingResource {
    Datastore,
    ImageSet,
}

impl MissingResource {
    /// The sentence this service answers a request naming this absent resource with —
    /// the same constant every other operation that resolves the kind uses, so one
    /// resource has one refusal text whichever route reached it.
    pub fn message(self) -> &'static str {
        match self {
            Self::Datastore => DATASTORE_NOT_FOUND_MESSAGE,
            Self::ImageSet => IMAGE_SET_NOT_FOUND_MESSAGE,
        }
    }
}

/// Parse a medical-imaging resource ARN, or `None` when the string is not one.
///
/// Accepts exactly what [`ARN_PATTERN`] does — partition, this service's
/// `arnNamespace`, a region, a 12-digit account id, and a `datastore/<id>` resource
/// with an optional `/imageset/<id>` suffix — so a value the boundary validator let
/// through always parses, and a caller that reaches the handler by another route is
/// still refused rather than mis-read.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceArn> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if !ARN_PARTITIONS.contains(&partition) {
        return None;
    }
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    // `[a-z0-9-]+`: the pattern constrains the region's alphabet, not its shape.
    let region = fields.next()?;
    if region.is_empty()
        || !region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    // `[0-9]{12}`: an account id.
    let account_id = fields.next()?;
    if account_id.len() != 12 || !account_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut segments = fields.next()?.split('/');
    if segments.next()? != "datastore" {
        return None;
    }
    let datastore_id = segments.next().filter(|id| is_resource_id(id))?.to_string();
    let image_set_id = match segments.next() {
        None => None,
        Some("imageset") => Some(segments.next().filter(|id| is_resource_id(id))?.to_string()),
        Some(_) => return None,
    };
    if segments.next().is_some() {
        return None;
    }
    Some(ResourceArn {
        datastore_arn: format!(
            "arn:{partition}:{ARN_SERVICE}:{region}:{account_id}:datastore/{datastore_id}"
        ),
        datastore_id,
        image_set_id,
    })
}

/// `[0-9a-z]{32}`: the id form [`ARN_PATTERN`] gives both `datastore/` and
/// `/imageset/` — what a request may NAME a resource by, which is wider than the form
/// this service mints one in ([`is_minted_id`]).
fn is_resource_id(value: &str) -> bool {
    value.len() == 32
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// `[0-9a-f]{32}`: the form of every identifier this service MINTS, which
/// [`ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`] writes as 32 lowercase
/// HEX characters — narrower than the `[0-9a-z]{32}` a request is allowed to name a
/// resource by ([`is_resource_id`]).
///
/// What a payload this service itself wrote is checked against: text outside this form is
/// something no call of this service ever produced, whatever a request may legally carry.
fn is_minted_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// A modelled timestamp for `instant`, kept to full nanosecond precision rather than
/// routed through an `f64` number of seconds.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(seconds, since_epoch.subsec_nanos())
}

// ---------------------------------------------------------------------------
// How this service PAGES a listing
//
// Every paginated listing this service serves walks a total order over the
// collection — the record's own instant first, its identifier to break a tie — and
// hands the caller an OPAQUE token naming the FULL sort key the next page starts at.
// Carrying the whole key is what keeps the walk correct when the record the token was
// minted at is gone by the next call: the resume can neither land before a page
// already served (repeating a record) nor past one never seen (skipping it).
//
// The codec lives here because more than one listing uses it, and a token format
// spelled twice drifts. Each collection passes its OWN marker, so a token minted for
// one collection is not a position in another and is refused rather than honoured.
//
// A marker names every choice the request makes about WHICH records the walk
// enumerates: the collection's scope, the order it is walked in where the caller picks
// one, and the FILTER it was narrowed by. A filter decides which records are in the walk
// at all, so a position computed under one is not a position under another.
// ---------------------------------------------------------------------------

/// How a collection marker spells the FILTER the request narrowed the walked records
/// with: the filter's own text, or [`UNFILTERED_PAGE_TOKEN_SEGMENT`] when the request
/// named none.
///
/// Passing it keeps a token a position in the collection the request actually defined: a
/// token minted while paging the records one filter selects is neither a position in the
/// records another selects nor one in the unfiltered whole, so it is refused rather than
/// honoured — which is what stops a resume under a changed filter from skipping records
/// the caller never saw or repeating ones already served.
pub fn page_token_filter_segment(filter: Option<&str>) -> &str {
    filter.unwrap_or(UNFILTERED_PAGE_TOKEN_SEGMENT)
}

/// How [`page_token_filter_segment`] spells "this request filtered nothing". Lower-case
/// letters outside the hexadecimal alphabet, so it is a text no modelled status value and
/// no [`page_token_filter_digest`] can spell, and the unfiltered collection's marker can
/// collide with no filtered one's.
pub const UNFILTERED_PAGE_TOKEN_SEGMENT: &str = "unfiltered";

/// How a collection marker spells a filter whose shape is a STRUCTURE rather than one
/// value: a fixed-width digest of the filter's canonical text.
///
/// A digest rather than the text itself, so the marker's length is the same whatever the
/// filter holds and a token stays well inside `NextToken`'s `1..8192`; equality is all a
/// marker needs of it, since the question a resume asks is only whether this request's
/// filter is the one the token was minted under.
pub fn page_token_filter_digest(canonical: &str) -> String {
    use ::sha2::Digest as _;
    let mut digest = ::sha2::Sha256::new();
    digest.update(canonical.as_bytes());
    ::hex::encode(digest.finalize())
}

/// How many records one page of a listing holds, from the `maxResults` the caller named.
///
/// Lives here, like the token codec above, because every paginated listing shares it and
/// a page-size rule spelled once per listing drifts: one clamp updated, the others left
/// behind.
///
/// A value outside [`MIN_LIST_PAGE_SIZE`]`..=`[`MAX_LIST_PAGE_SIZE`] is clamped here, not
/// merely at the boundary: one request must never be able to materialise an unbounded
/// page. A request naming no `maxResults` gets [`MAX_LIST_PAGE_SIZE`].
pub fn list_page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .map(|requested| requested.clamp(MIN_LIST_PAGE_SIZE, MAX_LIST_PAGE_SIZE))
        .unwrap_or(MAX_LIST_PAGE_SIZE)
}

/// The total order a listing walks: the record's instant first (seconds, then
/// nanoseconds), its identifier to break a tie — so a page boundary is stable no matter
/// how the underlying map iterates.
pub fn page_sort_key(instant: SystemTime, resource_id: &str) -> (u64, u32, &str) {
    let since_epoch = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (
        since_epoch.as_secs(),
        since_epoch.subsec_nanos(),
        resource_id,
    )
}

/// A decoded continuation token: the FULL sort key the next page starts at, so resuming
/// does not depend on the record that minted it still existing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    instant_secs: u64,
    instant_nanos: u32,
    resource_id: String,
}

impl PageCursor {
    /// This cursor as a sort key, comparable with [`page_sort_key`].
    pub fn as_key(&self) -> (u64, u32, &str) {
        (
            self.instant_secs,
            self.instant_nanos,
            self.resource_id.as_str(),
        )
    }
}

/// Encode the sort key the next page starts at as an opaque `nextToken`.
///
/// Hex keeps the token inside `NextToken`'s `^\p{ASCII}{0,8192}$` pattern and makes it
/// plainly not a resource identifier, so no caller can mistake it for one. `marker`
/// identifies the COLLECTION the position belongs to.
pub fn encode_page_token(marker: &str, instant: SystemTime, resource_id: &str) -> String {
    let (secs, nanos, id) = page_sort_key(instant, resource_id);
    ::hex::encode(format!("{marker}{secs}.{nanos}:{id}").as_bytes())
}

/// Read one NUMBER a continuation payload carries, spelled exactly as this service spells
/// one: ASCII digits only, so a signed, space-padded or empty text that `str::parse` would
/// otherwise accept is not honoured as part of a position, and no digit ahead of the first
/// significant one, since every number this service writes into a payload is written by
/// its plain decimal rendering — which pads nothing, so only zero itself is spelled with a
/// leading zero.
///
/// Shared by every codec below, because a payload half this service did not write is not a
/// position in any of their collections.
fn page_token_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

/// How many nanoseconds one second holds — the bound the sub-second half of an instant
/// stays under, since it is a remainder of a second ([`page_sort_key`] takes it from
/// [`::std::time::Duration::subsec_nanos`]).
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Read the INSTANT half of a continuation payload — seconds and nanoseconds, spelled
/// exactly as [`encode_page_token`] writes them ([`page_token_number`] each, the
/// sub-second half inside the second it is a remainder of) — or `None` when it is not a
/// text this service would have written.
pub fn parse_page_token_instant(instant: &str) -> Option<(u64, u32)> {
    let (secs, nanos) = instant.split_once('.')?;
    let secs = page_token_number(secs)?;
    let nanos = u32::try_from(page_token_number(nanos)?).ok()?;
    (u64::from(nanos) < NANOS_PER_SECOND).then_some((secs, nanos))
}

/// Decode a token [`encode_page_token`] produced for the collection `marker` names, or
/// `None` when it was not one — a string that is not hex, does not decode to UTF-8,
/// carries another collection's marker (or none), or holds a payload this service would
/// never mint.
pub fn decode_page_token(marker: &str, token: &str) -> Option<PageCursor> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let payload = decoded.strip_prefix(marker)?;
    // Every identifier a listing pages over is one this service minted, so neither `:`
    // nor `.` can appear in it and splitting on the first of each is unambiguous.
    let (instant, resource_id) = payload.split_once(':')?;
    // Both halves have to be spelled as this service spells them: the tie-breaker an
    // identifier it MINTS (what the sibling codec below checks too), the instant a pair of
    // plain numbers. A cursor carrying anything else is no position this service ever
    // handed out, and honouring it would resume the walk at a key no record holds — above
    // the record the real token named, which the walk would then never serve.
    if !is_minted_id(resource_id) {
        return None;
    }
    let (instant_secs, instant_nanos) = parse_page_token_instant(instant)?;
    Some(PageCursor {
        instant_secs,
        instant_nanos,
        resource_id: resource_id.to_string(),
    })
}

/// Encode the ORDINAL the next page starts at as an opaque `nextToken`, for a collection
/// whose records are totally ordered by a NUMBER rather than by an instant — an image
/// set's versions, which are numbered from [`FIRST_IMAGE_SET_VERSION_ID`] and never
/// renumbered or reused, so the number alone is the whole sort key.
///
/// A sibling of [`encode_page_token`] rather than a use of it: an ordinal compares
/// numerically (version 10 sorts after version 9), which the instant-plus-identifier key
/// that one carries cannot express. Both spell a token the same way — hex, so it stays
/// inside `NextToken`'s `^\p{ASCII}{0,8192}$` pattern and is plainly not a resource
/// identifier — and both take the COLLECTION's `marker`, so a token minted for one
/// collection is refused rather than honoured as a position in another.
pub fn encode_ordinal_page_token(marker: &str, ordinal: u32) -> String {
    ::hex::encode(format!("{marker}{ordinal}").as_bytes())
}

/// Decode a token [`encode_ordinal_page_token`] produced for the collection `marker`
/// names, or `None` when it was not one — a string that is not hex, does not decode to
/// UTF-8, carries another collection's marker (or none), or holds a payload that is not
/// an ordinal this service would mint.
pub fn decode_ordinal_page_token(marker: &str, token: &str) -> Option<u32> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let payload = decoded.strip_prefix(marker)?;
    // Spelled exactly as this service spells an ordinal ([`page_token_number`]): ASCII
    // digits only, so a signed or space-padded payload `str::parse` would otherwise accept
    // is not honoured as a position. Every ordinal this service mints is numbered from
    // one, so zero is not one either.
    let ordinal = u32::try_from(page_token_number(payload)?).ok()?;
    (ordinal > 0).then_some(ordinal)
}

/// Encode the sort key the next page starts at as an opaque `nextToken`, for a listing
/// whose order the CALLER chooses — a search, where the field and the direction the walk
/// follows come from the request, so the key's text is that field's own (an instant for
/// one field, a DICOM date and time for another) and only the caller's own operation
/// knows how to read it back.
///
/// A sibling of [`encode_page_token`] and [`encode_ordinal_page_token`] rather than a use
/// of either, for the same reason those two are siblings: the key is neither an instant
/// nor an ordinal. All three spell a token the same way — hex, so it stays inside
/// `NextToken`'s `^\p{ASCII}{0,8192}$` pattern and is plainly not a resource identifier —
/// and all three take the COLLECTION's `marker`, which for a caller-ordered listing names
/// the order too: a position in one order is not a position in another, so a token minted
/// under a different sort is refused rather than honoured.
pub fn encode_keyed_page_token(marker: &str, key: &str, resource_id: &str) -> String {
    ::hex::encode(format!("{marker}{key}:{resource_id}").as_bytes())
}

/// Decode a token [`encode_keyed_page_token`] produced for the collection and order
/// `marker` names, as the key's own text and the identifier that broke its tie, or `None`
/// when it was not one — a string that is not hex, does not decode to UTF-8, carries
/// another collection's or another order's marker (or none), or holds no identifier this
/// service would have minted.
///
/// Split at the LAST `:`, because every identifier this service pages over is minted hex
/// and so cannot contain one, while a key's text may.
pub fn decode_keyed_page_token(marker: &str, token: &str) -> Option<(String, String)> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let payload = decoded.strip_prefix(marker)?;
    let (key, resource_id) = payload.rsplit_once(':')?;
    // The tie-breaker has to be an identifier this service MINTS, exactly as the
    // instant-keyed codec above checks: a token is a position this service named, and no
    // id it ever named falls outside that form.
    if !is_minted_id(resource_id) {
        return None;
    }
    Some((key.to_string(), resource_id.to_string()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMedicalimagingAhigatewayserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMedicalimagingAhigatewayserviceState>)
}
