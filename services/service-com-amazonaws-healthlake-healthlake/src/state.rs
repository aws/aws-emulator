// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-healthlake-healthlake service.
//!
//! The FHIR data store is this service's primary resource: `CreateFHIRDatastore`
//! mints one here and `DescribeFHIRDatastore` / `ListFHIRDatastores` /
//! `TagResource` / `ListTagsForResource` / `UpdateFHIRDatastore` /
//! `DeleteFHIRDatastore` all read and write the SAME record, so a create is
//! observable through every one of them.
//!
//! `RestoreFHIRDatastore` mints one too, from the record a backup-enabled data
//! store already holds here, so the restored data store is observable through
//! exactly the same operations.
//!
//! The data transformation profile is the second: `CreateDataTransformationProfile`
//! mints one in DRAFT here, `UpdateDataTransformationProfile` rewrites that DRAFT's
//! content in place, `PublishDataTransformationProfile` snapshots a version of it
//! into [`DataTransformationProfile::published`], and
//! `GetDataTransformationProfile` reads the SAME record back, version by version.
//! `ListDataTransformationProfileVersions` reports those same versions — the
//! DRAFT and every published snapshot — newest first and paged, so a published
//! version is observable identically through either of them.
//! `ListDataTransformationProfiles` reports the SAME records once each instead,
//! at their latest version, filtered by source format and paged, so a created,
//! updated or published profile is observable through it too.
//!
//! The FHIR export job is the third: a job record belongs to the data store it
//! exports, `StartFHIRExportJob` MINTS one here, and `DescribeFHIRExportJob` /
//! `ListFHIRExportJobs` both read the SAME record back — one by `JobId`, the
//! other filtered and paged — so a started job is observable identically
//! through either of them, and a data store's jobs are dropped with the data
//! store itself.
//!
//! The FHIR import job is the fourth: a job record belongs to the data store it
//! imports into, is addressed by its own `JobId` WITHIN that data store, and
//! `DescribeFHIRImportJob` / `ListFHIRImportJobs` both report the records held
//! here — [`ComAmazonawsHealthlakeHealthlakeState::import_jobs`], one by `JobId`,
//! the other filtered and paged — so an import job is
//! dropped with the data store it belonged to exactly as an export job is.
//! `StartFHIRImportJob` is the operation that MINTS one, and its RUN —
//! [`import_run`] — reads the FHIR data the request's input location names out of
//! this emulator's own object store and DELIVERS each resource into the data
//! store's own [`Datastore::fhir_resources`], where the FHIR REST data plane
//! serves it exactly as it serves one a caller created.
//!
//! The data transformation job is the fifth: a job record is addressed by its
//! own `JobId`, `StartDataTransformationJob` MINTS one in
//! [`ComAmazonawsHealthlakeHealthlakeState::transformation_jobs`] and
//! `DescribeDataTransformationJob` / `ListDataTransformationJobs` both read the
//! SAME records back — one by `JobId`, the other filtered and paged — so a
//! started job is observable identically through either of them. A
//! transformation job reads and writes Amazon S3 rather than a data store, so
//! no record reaps one.
//!
//! The data transformation agent conversation is the sixth, and the only one no
//! operation mints a resource for: `UpdateProfileWithAgent` serves ONE TURN of a
//! chat over a profile, and the turn it serves is read from — and written back to
//! — the conversation record held in
//! [`ComAmazonawsHealthlakeHealthlakeState::agent_conversations`], so a
//! confirmation lands on the proposal the previous turn made. The turn that
//! confirms a proposal writes the profile's own DRAFT, which means
//! `GetDataTransformationProfile` and
//! `ListDataTransformationProfileVersions` read an agent-recorded change back
//! exactly as they read one `UpdateDataTransformationProfile` made. See
//! [`agent_conversation`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant, SystemTime};

mod agent_conversation;
pub use agent_conversation::*;

mod import_run;
pub use import_run::*;

mod sample_data;
pub use sample_data::*;

mod starter_profiles;
pub use starter_profiles::*;

use crate::ops::types::{
    AnalyticsConfiguration, BackupConfiguration, DatastoreBackupStatus, DatastoreProperties,
    DataTransformationProfileSummary, DataTransformationProfileVersionSummary, ErrorCause,
    ExportJobProperties,
    IdentityProviderConfiguration, ImportJobProperties, InputDataConfig, JobProgressReport,
    KmsEncryptionConfig, NlpConfiguration, OutputDataConfig, PreloadDataConfig,
    ProfileConfiguration, SseConfiguration, Tag, TransformationInputDataConfig,
    TransformationJobProgressReport, TransformationJobProperties, TransformationJobSummary,
    TransformationOutputDataConfig,
};

/// How long after the call that starts it a data store's pending lifecycle
/// transition (`CREATING` -> `ACTIVE`, `UPDATING` -> `ACTIVE`,
/// `DELETING` -> `DELETED`) falls due.
///
/// AWS documents no duration for any of these transitions, so this span is
/// CHOSEN: it is non-zero, so a caller that describes the store immediately
/// after creating, updating or deleting it still observes the transient status
/// (and the state errors that go with it), and short enough that nothing has to
/// wait long for the settled one. It is never slept on — the due instant is
/// stored on the record and materialised by the next call that reads it.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// How long a `DELETED` data store stays readable before it is reaped.
///
/// AWS documents no retention for a deleted data store, so this span is CHOSEN.
/// It is the TTL on a terminal record: long enough that a caller which polls
/// after its delete still sees `DELETED` rather than a bare 404, and short
/// enough that terminal records cannot accumulate. Like every other due instant
/// here it is stored, never slept on.
pub const DELETED_RETENTION: Duration = Duration::from_secs(60);

/// How long after the call that starts it a FHIR export job's pending status
/// transition falls due.
///
/// AWS documents no duration for an export run, so this span is CHOSEN, for the
/// reason [`LIFECYCLE_TRANSITION`] is: it is non-zero, so a caller that
/// describes the job immediately after starting it still observes the
/// `SUBMITTED` status it was answered with, and short enough that nothing waits
/// long for the settled one. It is never slept on — the due instant is stored
/// on the record and materialised by the next call that reads it.
pub const EXPORT_JOB_RUN: Duration = Duration::from_millis(500);

/// The most FHIR export jobs this emulator holds at once, across every data
/// store.
///
/// A ceiling, not a claim about any AWS quota: it exists so a
/// `StartFHIRExportJob` loop cannot grow the emulator's heap without bound, and
/// it also bounds the per-request
/// [`ComAmazonawsHealthlakeHealthlakeState::settle_export_jobs`] walk.
/// `StartFHIRExportJob` refuses past it with the `ThrottlingException` it models
/// — the only quota-shaped refusal that operation declares, the same one
/// `CreateFHIRDatastore` answers a full registry with.
pub const MAX_EXPORT_JOBS: usize = 200;

/// How long after the call that starts it a data transformation job's pending
/// status transition falls due.
///
/// AWS documents no duration for a transformation run, so this span is CHOSEN,
/// for the reason [`EXPORT_JOB_RUN`] is: it is non-zero, so a caller that
/// describes the job immediately after starting it still observes the
/// `SUBMITTED` status it was answered with, and short enough that nothing waits
/// long for the settled one. It is never slept on — the due instant is stored
/// on the record and materialised by the next call that reads it.
pub const TRANSFORMATION_JOB_RUN: Duration = Duration::from_millis(500);

/// The most data transformation jobs this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a
/// `StartDataTransformationJob` loop cannot grow the emulator's heap without
/// bound, and it also bounds the per-request
/// [`ComAmazonawsHealthlakeHealthlakeState::settle_transformation_jobs`] walk
/// and the idempotency walk
/// [`ComAmazonawsHealthlakeHealthlakeState::transformation_job_for_client_token`]
/// makes. `StartDataTransformationJob` refuses past it with the
/// `ThrottlingException` it models — the only quota-shaped refusal that
/// operation declares.
pub const MAX_TRANSFORMATION_JOBS: usize = 200;

/// The region stamped into an ARN and an endpoint when the request carries no
/// SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names a data store no record holds.
///
/// It is a FIXED service text: the same sentence whatever `DatastoreId` or ARN
/// was asked about, so a not-found answer never quotes the request back at the
/// caller. The model documents the error itself only as "The requested data
/// store was not found."; this is the wording the service answers with. Every
/// data-store not-found path in this crate states it through this one constant,
/// so none of them can drift into echoing the identifier.
pub const DATASTORE_NOT_FOUND_MESSAGE: &str = "Could not find specified datastore.";

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names an export job the data store it names holds no record of.
///
/// Fixed, for the reason [`DATASTORE_NOT_FOUND_MESSAGE`] is: the same sentence
/// whatever `JobId` was asked about, so a not-found answer never quotes the
/// request back at the caller. Every export-job not-found path in this crate
/// states it through this one constant.
pub const EXPORT_JOB_NOT_FOUND_MESSAGE: &str = "Could not find specified export job.";

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names an import job the data store it names holds no record of.
///
/// Fixed, for the reason [`DATASTORE_NOT_FOUND_MESSAGE`] is: the same sentence
/// whatever `JobId` was asked about, so a not-found answer never quotes the
/// request back at the caller. Every import-job not-found path in this crate
/// states it through this one constant.
pub const IMPORT_JOB_NOT_FOUND_MESSAGE: &str = "Could not find specified import job.";

/// Why a started export job ends in the modelled `FAILED` status: this emulator
/// cannot transmit the data store's FHIR resources to the destination Amazon S3
/// bucket the request named, so an export that has anything to transmit reports
/// the service's own failure rather than a transfer nothing ran.
///
/// `resources` is how many FHIR resources the data store held when the job was
/// started — the service's own state, not caller input, so the sentence states
/// it and quotes nothing from the request. The wording stays inside the modelled
/// `Message` alphabet (`^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`, len 1..2048), so the
/// text a job reports is one the model would admit.
pub fn export_not_transmitted_message(resources: usize) -> String {
    format!(
        "This emulator cannot transmit the data store FHIR resources to the destination Amazon \
         S3 bucket. Resources held: {resources}."
    )
}

/// The sentence this service refuses an operation with when the target data
/// store exists but its `DatastoreStatus` is a lifecycle TRANSITION state
/// (`CREATING`, `UPDATING` or `DELETING`).
///
/// It is the text beside the `ConflictException` (HTTP 409) the model documents
/// as "The data store is in a transition state and the user requested action
/// cannot be performed." for the operations that model one, and beside the
/// modelled `ValidationException` for an operation the model gives no
/// `ConflictException` (`StartFHIRExportJob`): the SENTENCE is the same whatever
/// shape carries it, so one data store's refusals cannot disagree about why the
/// call was refused.
///
/// `action` names the refused operation. The status is the SERVICE's own state,
/// so the sentence states it; like [`DATASTORE_NOT_FOUND_MESSAGE`] it quotes
/// nothing from the request. Every operation in this crate states its
/// resource-state conflict through this one function, so none of them can drift
/// into a different shape of refusal or into echoing the identifier.
pub fn datastore_in_transition_message(status: DatastoreStatus, action: DatastoreAction) -> String {
    format!(
        "Specified datastore is in status {} and cannot be {}.",
        status.as_wire(),
        action.as_participle()
    )
}

/// An operation on a data store record, as [`datastore_in_transition_message`]
/// names it when the record's state refuses it.
///
/// A typed value rather than a free-form word: the operations of this resource
/// cannot drift into different spellings of the same sentence, and a new one has
/// to add its variant here. `UpdateFHIRDatastore` and `StartFHIRImportJob` are
/// deliberately absent: neither classifies a non-`ACTIVE` target as a transition
/// conflict at all — they draw ONE line, active or not, and state
/// [`DATASTORE_NOT_ACTIVE_MESSAGE`] through [`Datastore::gate_active`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatastoreAction {
    Delete,
    /// Starting a FHIR export job on the data store, which reads it rather than
    /// changing it but still needs a settled, usable data store to export from.
    ExportFrom,
    /// Restoring the data store to a point in time, which mints a new data store
    /// from this one's backup and so needs this one settled and usable too.
    Restore,
}

impl DatastoreAction {
    /// The past participle the conflict sentence states this action with.
    pub fn as_participle(self) -> &'static str {
        match self {
            Self::Delete => "deleted",
            Self::ExportFrom => "exported from",
            Self::Restore => "restored",
        }
    }
}

/// Why a data store's own [`DatastoreStatus`] refuses a [`DatastoreAction`] on
/// it, with the sentence that refusal states.
///
/// Which statuses refuse an action, and as WHICH of these two kinds, is decided
/// once in [`Datastore::gate_action`]; what each operation keeps for itself is
/// only the modelled error SHAPE it carries the answer on, which the model does
/// not give every one of them alike (a `ConflictException` where the operation
/// declares one, the `ValidationException` it does declare where it does not).
/// So a status this enum grows an arm for, or an operation added to
/// [`DatastoreAction`], is classified in one place rather than in one `match`
/// per handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatastoreRefusal {
    /// The record exists but sits in a lifecycle TRANSITION state, so the
    /// requested action cannot be performed on it: the sentence is
    /// [`datastore_in_transition_message`].
    InTransition(String),
    /// The record has reached the terminal `DELETED` status. That is no
    /// transition — the data store is gone — so the refusal is this service's
    /// not-found answer, [`DATASTORE_NOT_FOUND_MESSAGE`].
    Gone(String),
}

/// The sentence this service refuses an operation with when the record exists
/// but its `DatastoreStatus` is anything other than `ACTIVE`.
///
/// Fixed, like the texts above: it quotes neither the requested `DatastoreId`
/// nor the status the record happens to be in, and it reads the same whichever
/// non-`ACTIVE` state that is — what the refusal is about is the record not
/// being settled and usable, not which stage of the lifecycle it sits at. It
/// stays inside the modelled `Message` alphabet
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`), so the text is one the model would
/// admit. [`Datastore::gate_active`] is the ONE gate that states it.
pub const DATASTORE_NOT_ACTIVE_MESSAGE: &str = "Specified datastore is not active.";

/// The sentence this service refuses a point-in-time restore with when the
/// source data store exists but holds no enabled backup to restore from.
///
/// Fixed, like the two texts above: it names neither the requested
/// `DatastoreId` nor the restore point, so a refusal never quotes the request
/// back at the caller.
pub const DATASTORE_BACKUP_NOT_ENABLED_MESSAGE: &str =
    "Specified datastore does not have backup enabled.";

/// Seconds in a day, for turning a modelled `BackupRetentionPeriodInDays`
/// (range 1..30) into the span it names.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names a data transformation profile no record holds.
///
/// Unlike the data-store texts above, the data transformation surface IDENTIFIES
/// the resource it could not find: the sentence is a template whose one variable
/// part is the `ProfileId` the request asked about, so a caller fanning out over
/// several profiles can tell which one was missing. The identifier is taken from
/// the request and is bounded by the modelled `ProfileIdString` (32 characters,
/// `^[a-f0-9]{32}$`), which the boundary validator has already enforced. Every
/// profile not-found path in this crate states it through this one function.
pub fn profile_not_found_message(profile_id: &str) -> String {
    format!("Profile '{profile_id}' not found")
}

/// The sentence this service's `ResourceNotFoundException` carries when the
/// profile exists but not at the version the call resolved — a version that was
/// never published, or the DRAFT of a profile no record holds.
///
/// The template of [`profile_not_found_message`] with a second variable part: the
/// version the call resolved, so a caller can tell a missing profile from a
/// missing version of one. Every profile-version not-found path in this crate
/// states it through this one function.
pub fn profile_version_not_found_message(profile_id: &str, version: i32) -> String {
    format!("Profile '{profile_id}' version {version} not found")
}

/// The sentence this service's `ResourceNotFoundException` carries when the
/// request names a data transformation job no record holds.
///
/// A template, for the reason [`profile_not_found_message`] is: its one variable
/// part is the `JobId` the request asked about, bounded by the modelled
/// `DataTransformationJobId` (1..32, `^[a-zA-Z0-9-]+$`) the boundary validator has
/// already enforced. Every transformation-job not-found path in this crate states
/// it through this one function.
pub fn transformation_job_not_found_message(job_id: &str) -> String {
    format!("Transformation job not found: {job_id}")
}

/// The `SourceFormat` value set this service's refusal of a value outside the enum
/// REPORTS, in the order it lists them.
///
/// Wider than the on-disk model snapshot's `SourceFormat` enum
/// (`smithy_query(query="shape",shape="SourceFormat")` -> `values: [CCDA, CSV]`),
/// which lags the service: the wire refusal the service answers an unsupported
/// format with names all three, so this is what a caller correcting its request
/// reads. The CONSTRAINT itself is still the model's — the boundary walk enforces the
/// snapshot's enum — so this set is used for the SENTENCE only, in the one place each
/// refusal is worded.
///
/// One consequence, stated plainly rather than left for a reader to find: a value
/// this set names but the snapshot's enum does not (`HL7V2`) is still REFUSED, since
/// what admits a value is the model the framework walks. Growing the snapshot is what
/// would accept it; `tests/all/source_format_enum_refusal.rs` keeps that value among
/// its refused rows so the emulator's answer for it is pinned, not quietly assumed.
pub const REPORTED_SOURCE_FORMATS: &[&str] = &["CSV", "HL7V2", "CCDA"];

/// The status a stored data transformation profile reports.
///
/// The service reports a profile's status in its create reply, and a profile is
/// usable from the instant that reply is written — its DRAFT exists then, so no
/// transition is owed and no other status is reachable. It is not a member of the
/// model snapshot's profile shapes, so the spelling is the one the wire carries.
/// Read through [`DataTransformationProfile::status`], never written out beside it.
pub const ACTIVE_PROFILE_STATUS: &str = "ACTIVE";

/// Why a started data transformation job ends in the modelled `FAILED` status:
/// the run has to READ the source files from the input Amazon S3 location and
/// WRITE the converted output to the destination bucket, and this emulator
/// opens no connection and reaches no bucket, so neither transfer happens.
///
/// The job is therefore reported as having failed, with this sentence saying
/// why, rather than as having converted files nothing read — and for the same
/// reason the record carries no `JobProgressReport`: no file was scanned, so
/// there are no counts to report. Fixed text, quoting nothing from the request,
/// and within the modelled `Message` length bound (1..2048).
pub const TRANSFORMATION_NOT_PERFORMED_MESSAGE: &str =
    "This emulator cannot read the source files from the input Amazon S3 location or write \
     converted output to the destination Amazon S3 bucket, so no file was transformed.";

/// The most data stores this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a `CreateFHIRDatastore`
/// loop cannot grow the emulator's heap without bound, and it also bounds the
/// per-request [`ComAmazonawsHealthlakeHealthlakeState::settle_lifecycle`] walk.
/// `CreateFHIRDatastore` refuses past it with the `ThrottlingException` it models
/// — the only quota-shaped refusal that operation declares.
pub const MAX_DATASTORES: usize = 100;

/// The most `ClientToken` entries kept for data-store idempotency, across both
/// operations that mint a data store (`CreateFHIRDatastore` and
/// `RestoreFHIRDatastore`).
///
/// At most one token is recorded per stored data store and a token is dropped
/// with the record it minted, so this cap is never the binding one in practice;
/// it is enforced anyway, by evicting the oldest token, so a stream of fresh
/// caller-chosen tokens cannot grow the map independently of the data stores
/// they name.
pub const MAX_CLIENT_TOKENS: usize = MAX_DATASTORES;

/// The most FHIR resources one data store holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a loop of FHIR
/// creates against the data store's REST endpoint cannot grow the emulator's
/// heap without bound. The data plane refuses past it rather than evicting, so
/// a resource a caller created stays readable for as long as its data store
/// lives.
pub const MAX_FHIR_RESOURCES_PER_DATASTORE: usize = 500;

/// The most versions of ONE FHIR resource the data plane retains at once,
/// counting the version a read answers.
///
/// The update interaction supersedes the version a read answers, and the
/// superseded version is RETAINED beside it, so a point-in-time restore can
/// still reproduce the resource as it stood before that write. A ceiling, not a
/// claim about any AWS quota: it exists so a loop of updates against one logical
/// id cannot grow the emulator's heap without bound, and the retained bytes are
/// charged against the same budgets a create is charged against
/// ([`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`] and
/// [`MAX_FHIR_RESOURCE_BYTES_TOTAL`]). Past it the OLDEST retained version is
/// dropped, so a restore point older than every version a data store still
/// holds for a resource reproduces none of them — the same answer as a restore
/// point before that resource was created.
pub const MAX_FHIR_RESOURCE_VERSIONS: usize = 10;

/// The largest single FHIR resource the data plane stores.
///
/// A count ceiling alone does not bound the retained HEAP: the bytes held would
/// be that count times whatever one request body the transport admits. So the
/// SIZE of one resource is bounded here, the bytes one data store retains by
/// [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`], and the bytes every data store
/// retains together by [`MAX_FHIR_RESOURCE_BYTES_TOTAL`] — so no product of
/// caller-chosen factors is the bound. Like the count, these are ceilings this
/// emulator chooses, not claims about any AWS quota.
pub const MAX_FHIR_RESOURCE_BYTES: usize = 1 << 20;

/// The most bytes of FHIR resources one data store retains.
pub const MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE: usize = 16 << 20;

/// The most bytes of FHIR resources EVERY data store retains together, so
/// [`MAX_DATASTORES`] data stores at their own budget cannot multiply into an
/// unbounded heap.
pub const MAX_FHIR_RESOURCE_BYTES_TOTAL: usize = 64 << 20;

/// Which of the three ceilings on retained FHIR resources a write reached.
///
/// The ceilings are decided once, in
/// [`ComAmazonawsHealthlakeHealthlakeState::admits_fhir_resource`]; what each
/// caller keeps for itself is only how its own surface STATES the refusal — the
/// FHIR `OperationOutcome` the REST data plane answers, or the `Message` an
/// import job's record carries — so a resource one of them would not retain is
/// not retained by the other either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FhirRetentionCeiling {
    /// [`MAX_FHIR_RESOURCES_PER_DATASTORE`]: the data store already holds as
    /// many resources as this emulator retains for one.
    ResourceCount,
    /// [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`]: the data store already retains
    /// as many resource bytes as this emulator holds for one.
    DatastoreBytes,
    /// [`MAX_FHIR_RESOURCE_BYTES_TOTAL`]: every data store together already
    /// retains as many resource bytes as this emulator holds.
    StoreBytes,
}

/// The most tags one data store carries.
///
/// The modelled `TagList` bound (0..200) caps ONE request; this caps the
/// accumulation across repeated `TagResource` calls with distinct keys. Both
/// `CreateFHIRDatastore` and `TagResource` refuse past it with the
/// `ValidationException` they model.
pub const MAX_TAGS_PER_DATASTORE: usize = 50;

/// The most tags one data transformation profile carries.
///
/// Taken from the model rather than chosen: a profile's tags are also written
/// by `CreateDataTransformationProfile`'s `Tags` (`TagMap`, len 0..50), so the
/// set a repeated `TagResource` accumulates is held to the same bound the
/// create path is held to. `TagResource` refuses past it with the
/// `ValidationException` it models.
pub const MAX_TAGS_PER_PROFILE: usize = 50;

/// How a refusal names a FHIR data store — the kind of resource, never the
/// identifier of one, which this service's error texts do not quote back.
pub const DATASTORE_NOUN: &str = "data store";

/// How a refusal names a data transformation profile, beside
/// [`DATASTORE_NOUN`].
pub const PROFILE_NOUN: &str = "data transformation profile";

/// The release-version segment a data store's FHIR endpoint publishes, and so
/// the third segment of every path on that endpoint
/// (`/datastore/{DatastoreId}/r4/...`).
///
/// The modelled `FHIRVersion` enum declares exactly one value (`R4`), which is
/// what [`Datastore::type_version`] holds; the endpoint spells it lowercase.
pub const FHIR_ENDPOINT_VERSION_SEGMENT: &str = "r4";

/// The literal first segment of a data store FHIR endpoint path.
const DATASTORE_PATH_SEGMENT: &str = "datastore";

/// The `datastore` resource prefix of a HealthLake data store ARN, as the
/// modelled `AmazonResourceName` pattern spells it
/// (`...:(datastore\/fhir\/.{32}|dataTransformationProfile\/[a-f0-9]{32})$`).
const DATASTORE_ARN_RESOURCE_PREFIX: &str = "datastore/fhir/";

/// The enabled value of the modelled `BackupStatus` enum
/// (`BACKUP_STATUS_VALUES` = `["DISABLED", "ENABLED"]`).
const BACKUP_ENABLED: &str = "ENABLED";

/// The service-owned-key value of the modelled `CmkType` enum
/// (`CMK_TYPE_VALUES` = `["AWS_OWNED_KMS_KEY", "CUSTOMER_MANAGED_KMS_KEY"]`).
///
/// `KmsEncryptionConfig` is documented as "The customer-managed-key (CMK) used when
/// creating a data store. If a customer-owned key is not specified, an Amazon Web
/// Services-owned key is used for encryption", and this is the value that case is
/// modelled by: it is what [`aws_owned_sse_configuration`] reports.
const AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// The `dataTransformationProfile` resource prefix of a HealthLake data
/// transformation profile ARN, as the same modelled `AmazonResourceName`
/// pattern spells it, and as the `DataTransformationProfile` resource's
/// `aws.api#arn` template does (`dataTransformationProfile/{ProfileId}`).
const PROFILE_ARN_RESOURCE_PREFIX: &str = "dataTransformationProfile/";

/// The version number that names a data transformation profile's DRAFT.
///
/// `CreateDataTransformationProfile` documents it: "The starting version is
/// always 0, which indicates the profile is in DRAFT state", and
/// `GetDataTransformationProfile` reads it back the same way ("Specify 0 to
/// retrieve the DRAFT version").
pub const DRAFT_PROFILE_VERSION: i32 = 0;

/// The highest version number a data transformation profile reaches, from the
/// modelled `ProfileVersion` range (0..99): the DRAFT is 0, so at most 99
/// published versions exist.
pub const MAX_PROFILE_VERSION: i32 = 99;

/// The most mapping entries one profile version holds, from the modelled
/// `ProfileMapping` map's own length bound (1..500) — the shape
/// `GetDataTransformationProfile` reports the content through, so content past
/// it could be stored but never read back.
pub const MAX_PROFILE_MAPPING_ENTRIES: usize = 500;

/// The fewest mapping entries one profile version holds, from the same
/// modelled `ProfileMapping` length bound (1..500): a profile version always
/// carries content.
pub const MIN_PROFILE_MAPPING_ENTRIES: usize = 1;

/// The longest mapping key, from the modelled `ProfileMappingKey` length bound
/// (1..500).
pub const MAX_PROFILE_MAPPING_KEY_LEN: usize = 500;

/// The longest mapping value, from the modelled `ProfileMappingValue` length
/// bound (0..102400).
pub const MAX_PROFILE_MAPPING_VALUE_LEN: usize = 102_400;

/// The most BYTES one profile version's mapping holds TOGETHER.
///
/// The modelled `ProfileMapping` bounds are all per entry (at most 500 entries, keys
/// 1..500 characters, values 0..102400), so a mapping that satisfies every one of
/// them can still carry ~51 MB — and a count ceiling alone would leave the retained
/// heap a product of caller-chosen factors (the same reason the FHIR data plane
/// budgets [`MAX_FHIR_RESOURCE_BYTES`]). This ceiling is this emulator's, not an AWS
/// quota: it admits ten modelled-maximum files, or all 500 small ones, which is more
/// than a profile needs. It bounds ONE version; a profile holds up to
/// `1 + MAX_PROFILE_VERSION` of them once `PublishDataTransformationProfile`
/// snapshots the DRAFT into published versions, so what one profile retains is
/// bounded by [`MAX_PROFILE_RETAINED_BYTES`] and what every profile retains
/// together by [`MAX_PROFILE_RETAINED_BYTES_TOTAL`] — the way the FHIR data plane
/// budgets [`MAX_FHIR_RESOURCE_BYTES`], [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`]
/// and [`MAX_FHIR_RESOURCE_BYTES_TOTAL`] — so no product of caller-chosen factors
/// is the bound.
///
/// Measured in BYTES (`str::len`) rather than characters, so multi-byte content
/// cannot pass the budget by a factor of four.
pub const MAX_PROFILE_MAPPING_BYTES: usize = 1 << 20;

/// The most BYTES of version content ONE data transformation profile retains,
/// across its DRAFT and every version published from it.
///
/// A count ceiling alone does not bound the retained heap: a profile may hold
/// `1 + MAX_PROFILE_VERSION` versions, each up to
/// [`MAX_PROFILE_MAPPING_BYTES`], so the per-version ceiling alone would leave what
/// one profile retains a product of caller-chosen factors. This is this emulator's
/// ceiling, not a claim about any AWS quota;
/// `PublishDataTransformationProfile` — the only operation that gives a profile a
/// second version — refuses past it with the `ServiceQuotaExceededException` it
/// models.
pub const MAX_PROFILE_RETAINED_BYTES: usize = 4 << 20;

/// The most BYTES of version content EVERY data transformation profile retains
/// together, so [`MAX_PROFILES`] profiles at their own budget cannot multiply into
/// an unbounded heap.
pub const MAX_PROFILE_RETAINED_BYTES_TOTAL: usize = 32 << 20;

/// The most data transformation profiles this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a
/// `CreateDataTransformationProfile` loop cannot grow the emulator's heap
/// without bound. `CreateDataTransformationProfile` refuses past it with the
/// `ServiceQuotaExceededException` it models.
pub const MAX_PROFILES: usize = 100;

/// The most `ClientToken` entries kept for `CreateDataTransformationProfile`
/// idempotency — one per profile that can be stored, for the same reason
/// [`MAX_CLIENT_TOKENS`] bounds the data store tokens.
pub const MAX_PROFILE_CLIENT_TOKENS: usize = MAX_PROFILES;

/// The states a data store moves through in this slice, as the modelled
/// `DatastoreStatus` enum names them on the wire. The states the enum also
/// declares (`CREATE_FAILED`, `UPDATE_FAILED`) describe a FAILURE of the
/// asynchronous work, and nothing in this slice fails, so nothing here produces
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatastoreStatus {
    Creating,
    Active,
    /// A configuration change is being applied to a data store that was
    /// `ACTIVE`: the transient status of `UpdateFHIRDatastore`, which settles
    /// back to [`Self::Active`] when the transition the clock owes falls due.
    Updating,
    Deleting,
    Deleted,
}

impl DatastoreStatus {
    /// The modelled enum value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }
}

/// One persisted FHIR data store: everything `CreateFHIRDatastore` was given,
/// plus the identifiers and the lifecycle state the service owns.
#[derive(Debug)]
pub struct Datastore {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub status: DatastoreStatus,
    /// The transition the clock owes this record, as `(due instant, next status)`.
    /// `None` once the status is settled.
    pub pending: Option<(Instant, DatastoreStatus)>,
    /// When this record is reaped, set once it reaches the terminal `DELETED`
    /// status. `None` while the data store is still live.
    pub reap_at: Option<Instant>,
    pub type_version: String,
    pub endpoint: String,
    pub created_at: ::aws_smithy_types::DateTime,
    /// How this data store's contents are encrypted at rest. NOT an `Option`: a
    /// data store is always encrypted, with the customer-managed key its create
    /// named or else with the service-owned key
    /// ([`aws_owned_sse_configuration`]), so every operation that mints a record
    /// has to settle which — and `DatastoreProperties.SseConfiguration` reports
    /// it on every read.
    pub sse_configuration: SseConfiguration,
    pub preload_data_config: Option<PreloadDataConfig>,
    pub identity_provider_configuration: Option<IdentityProviderConfiguration>,
    pub analytics_configuration: Option<AnalyticsConfiguration>,
    pub nlp_configuration: Option<NlpConfiguration>,
    pub profile_configuration: Option<ProfileConfiguration>,
    pub backup_configuration: Option<BackupConfiguration>,
    /// When this data store's backup configuration became `ENABLED`, which is
    /// what the modelled `DatastoreBackupStatus.BackupEnabledAt` reports ("the
    /// time backup was enabled on the data store"). `None` whenever backup is
    /// not enabled. Written only through [`Datastore::set_backup_configuration`].
    pub backup_enabled_at: Option<::aws_smithy_types::DateTime>,
    pub error_cause: Option<ErrorCause>,
    /// Tags by key, so `TagResource` merges by key and the reported order is
    /// stable across reads. Seeded from `CreateFHIRDatastore`'s `Tags`.
    pub tags: BTreeMap<String, String>,
    /// Every FHIR resource written through this data store's REST endpoint, by
    /// `(resourceType, logical id)`. A `BTreeMap` so the held order is stable
    /// across reads, and held HERE rather than in a map beside the registry so
    /// a data store's resources are dropped with the data store itself.
    /// Bounded by [`MAX_FHIR_RESOURCES_PER_DATASTORE`].
    pub fhir_resources: BTreeMap<(String, String), FhirResource>,
    /// How many BYTES the resources above hold together — the sum of their own
    /// [`FhirResource::retained_bytes`], so every version a resource retains is
    /// counted, maintained as they are stored so the data plane can refuse past
    /// [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`] without re-measuring every
    /// stored resource.
    pub fhir_resource_bytes: usize,
}

/// One FHIR resource a data store holds, as the FHIR REST data plane stored it:
/// the version a read answers, plus the versions earlier writes left behind.
///
/// The resource is kept as the JSON a read answers VERBATIM, so two reads of one
/// resource cannot differ and no read re-derives a server-owned value.
#[derive(Debug, Clone)]
pub struct FhirResource {
    /// The `resourceType` the resource carries, which the FHIR create
    /// interaction requires to equal the type segment of the URL it was posted
    /// to.
    pub resource_type: String,
    /// The logical id the server assigned.
    pub id: String,
    /// `meta.versionId`: 1 for a resource that has only been created.
    pub version: u64,
    /// `meta.lastUpdated`, the instant the stored version was written.
    pub last_updated: ::aws_smithy_types::DateTime,
    /// The stored resource JSON: the submitted content with the server-owned
    /// `id` and `meta` written onto it.
    pub content: ::serde_json::Value,
    /// How many bytes that JSON serializes to — what this record costs the
    /// store, so a later delete can give the budget back exactly what the
    /// create took.
    pub bytes: usize,
    /// The versions a later write SUPERSEDED, oldest first, each exactly as it
    /// was stored — never the version above, which is the one a read answers.
    ///
    /// Retained rather than discarded because a write only ends the span a
    /// version was current for: a reader asking what this resource was at an
    /// earlier instant (the point-in-time restore
    /// [`Datastore::fhir_resources_as_of`] performs) is answered from here.
    /// Bounded by [`MAX_FHIR_RESOURCE_VERSIONS`], and by the retention budgets
    /// the bytes held here are charged against, both of which drop the OLDEST
    /// versions first.
    pub superseded: Vec<FhirResourceVersion>,
}

/// One version of a FHIR resource a later write superseded: everything that
/// version of the resource was, kept so an earlier instant can be answered for.
///
/// Carries no `resourceType` or logical id of its own: those identify the
/// RESOURCE, which is [`FhirResource`], and every version of it shares them.
#[derive(Debug, Clone)]
pub struct FhirResourceVersion {
    /// `meta.versionId` of this version.
    pub version: u64,
    /// `meta.lastUpdated`: the instant this version was written, so it is the
    /// version a read as of any instant from here until the next write answers.
    pub last_updated: ::aws_smithy_types::DateTime,
    /// The stored resource JSON of this version, verbatim.
    pub content: ::serde_json::Value,
    /// How many bytes that JSON serializes to, so retaining it is charged — and
    /// dropping it refunded — exactly what it costs.
    pub bytes: usize,
}

impl FhirResource {
    /// The bytes this resource costs its data store: the version a read answers
    /// plus every superseded version retained beside it.
    pub fn retained_bytes(&self) -> usize {
        self.superseded
            .iter()
            .fold(self.bytes, |total, version| {
                total.saturating_add(version.bytes)
            })
    }

    /// How many of the versions this resource would retain after a further
    /// write must be DROPPED to keep [`MAX_FHIR_RESOURCE_VERSIONS`], counted
    /// from the oldest.
    ///
    /// The floor on what a write drops; the data plane drops more when its
    /// retention budgets cannot hold what is left.
    pub fn forced_drops_on_supersede(&self) -> usize {
        // One more version is about to be held: the current one moves into the
        // history and the new one takes its place, so the history would hold
        // `len() + 1` beside it.
        self.superseded
            .len()
            .saturating_add(2)
            .saturating_sub(MAX_FHIR_RESOURCE_VERSIONS)
    }

    /// The most versions a further write can drop: every version it would
    /// retain, which leaves the new version standing alone.
    pub fn max_drops_on_supersede(&self) -> usize {
        self.superseded.len().saturating_add(1)
    }

    /// How many bytes a further write RELEASES when it drops the `drop` oldest
    /// of the versions it would otherwise retain.
    ///
    /// Stated separately from [`Self::supersede_dropping`] so the data plane can
    /// charge its retention budgets BEFORE it writes, the way it already does
    /// for the bytes a write adds.
    pub fn release_dropping(&self, drop: usize) -> usize {
        // The versions this resource would retain, oldest first: its history,
        // and then the version this write supersedes.
        self.superseded
            .iter()
            .map(|version| version.bytes)
            .chain(::std::iter::once(self.bytes))
            .take(drop)
            .fold(0usize, usize::saturating_add)
    }

    /// Supersede the version a read answers with `next`, keeping the version it
    /// replaces in this resource's history and dropping the `drop` oldest
    /// versions retained there, and report the bytes those dropped versions
    /// released.
    pub fn supersede_dropping(&mut self, next: FhirResourceVersion, drop: usize) -> usize {
        self.superseded.push(FhirResourceVersion {
            version: self.version,
            last_updated: self.last_updated,
            content: ::std::mem::replace(&mut self.content, next.content),
            bytes: self.bytes,
        });
        self.version = next.version;
        self.last_updated = next.last_updated;
        self.bytes = next.bytes;
        self.superseded
            .drain(..drop.min(self.superseded.len()))
            .fold(0usize, |total, version| {
                total.saturating_add(version.bytes)
            })
    }

    /// This resource as it stood at `at`: the newest version written at or
    /// before that instant, carrying the versions written before it, or `None`
    /// when this data store holds no version of it from then.
    ///
    /// A version is current from the instant it was written until the next write
    /// supersedes it, so the newest version at or before `at` IS what a read at
    /// `at` would have answered. Nothing is reconstructed: a version is copied
    /// exactly as it was stored, or the resource is left out.
    pub fn as_of(&self, at: &::aws_smithy_types::DateTime) -> Option<Self> {
        let cutoff = epoch_nanos(at);
        // The version a read answers now is the newest of all, so when it is
        // itself at or before the cutoff every retained version is too.
        if epoch_nanos(&self.last_updated) <= cutoff {
            return Some(self.clone());
        }
        // Otherwise the newest RETAINED version at or before the cutoff, found
        // from the newest end because the history runs oldest first.
        let index = self
            .superseded
            .iter()
            .rposition(|version| epoch_nanos(&version.last_updated) <= cutoff)?;
        let held = &self.superseded[index];
        Some(Self {
            resource_type: self.resource_type.clone(),
            id: self.id.clone(),
            version: held.version,
            last_updated: held.last_updated,
            content: held.content.clone(),
            bytes: held.bytes,
            superseded: self.superseded[..index].to_vec(),
        })
    }
}

impl Datastore {
    /// This record as the modelled `DatastoreProperties`, the one shape
    /// `DescribeFHIRDatastore` and `ListFHIRDatastores` both report.
    ///
    /// `now` is the instant the reading call runs, which is what the reported
    /// restore window is derived against ([`Datastore::restore_window`]): a
    /// continuous backup's latest restorable point IS the present, so it is a
    /// parameter rather than a stored value. Taking it from the caller also
    /// makes one call's whole answer — every entry of a `ListFHIRDatastores`
    /// page included — consistent at a single instant.
    pub fn properties(&self, now: SystemTime) -> DatastoreProperties {
        let window = self.restore_window(now);
        DatastoreProperties {
            datastore_id: self.id.clone(),
            datastore_arn: self.arn.clone(),
            datastore_name: self.name.clone(),
            datastore_status: self.status.as_wire().to_string(),
            created_at: Some(self.created_at),
            datastore_type_version: self.type_version.clone(),
            datastore_endpoint: self.endpoint.clone(),
            // Always reported: this data store IS encrypted, so a client reading
            // its encryption posture is told which key encrypts it rather than
            // finding the member absent.
            sse_configuration: Some(self.sse_configuration.clone()),
            preload_data_config: self.preload_data_config.clone(),
            identity_provider_configuration: self.identity_provider_configuration.clone(),
            error_cause: self.error_cause.clone(),
            nlp_configuration: self.nlp_configuration.clone(),
            analytics_configuration: self.analytics_configuration.clone(),
            profile_configuration: self.profile_configuration.clone(),
            // The backup status reports the stored configuration, the instant
            // that configuration became enabled, and the two bounds of the
            // window this data store can be restored to — all derived from this
            // one record. The two restore points are exactly the window
            // [`Datastore::restore_window`] derives and
            // `RestoreFHIRDatastore` holds a requested restore point against,
            // so a caller can read the window and then name a point inside it.
            //
            // `ScheduledPermanentDeletionTime` is "the time the retained backup
            // data is scheduled for permanent deletion", and this service
            // retains no backup data of a deleted data store, so there is no
            // such instant to report.
            backup_status_info: self
                .backup_configuration
                .clone()
                .map(|configuration| DatastoreBackupStatus {
                    configuration: Some(configuration),
                    backup_enabled_at: self.backup_enabled_at,
                    earliest_restore_point: window.map(|window| window.earliest),
                    latest_restore_point: window.map(|window| window.latest),
                    scheduled_permanent_deletion_time: None,
                }),
        }
    }

    /// Whether `action` may proceed against this record as its lifecycle status
    /// stands, or the refusal that status earns it.
    ///
    /// The ONE status partition behind every data-store action gate in this
    /// crate: only a settled, usable (`ACTIVE`) record takes an action, a record
    /// in a lifecycle transition refuses it as a transition conflict worded by
    /// [`datastore_in_transition_message`], and a record that has reached the
    /// terminal `DELETED` status refuses it as gone
    /// ([`DATASTORE_NOT_FOUND_MESSAGE`]). The caller wraps the answer in the
    /// modelled error shape ITS operation declares — which is all that differs
    /// between them — so the statuses, the two classifications and the sentences
    /// cannot drift apart operation by operation.
    ///
    /// `UpdateFHIRDatastore` deliberately does NOT come through here: it draws a
    /// different line (anything other than `ACTIVE` is simply not active, stated
    /// with [`DATASTORE_NOT_ACTIVE_MESSAGE`]), which is why
    /// [`DatastoreAction`] has no variant for it.
    ///
    /// Reads the status only, and writes nothing: call
    /// [`ComAmazonawsHealthlakeHealthlakeState::settle_lifecycle`] first, so the
    /// status this gates on is the one the clock already owes.
    pub fn gate_action(&self, action: DatastoreAction) -> Result<(), DatastoreRefusal> {
        match self.status {
            DatastoreStatus::Active => Ok(()),
            DatastoreStatus::Creating | DatastoreStatus::Updating | DatastoreStatus::Deleting => {
                Err(DatastoreRefusal::InTransition(
                    datastore_in_transition_message(self.status, action),
                ))
            }
            DatastoreStatus::Deleted => Err(DatastoreRefusal::Gone(
                DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            )),
        }
    }

    /// `Ok` when this record is the settled, usable `ACTIVE` data store an
    /// operation that draws ONE line requires, else the sentence that refusal
    /// states.
    ///
    /// The companion of [`Self::gate_action`] for the operations of this
    /// resource that do NOT classify a non-`ACTIVE` target as a transition
    /// conflict: `UpdateFHIRDatastore` and `StartFHIRImportJob` both answer
    /// every status other than `ACTIVE` — the lifecycle transitions `CREATING`,
    /// `UPDATING` and `DELETING`, and the terminal `DELETED` — with the same
    /// refusal, worded [`DATASTORE_NOT_ACTIVE_MESSAGE`], so the answer does not
    /// depend on which stage of the lifecycle the record sits at. It is NOT a
    /// not-found: the record is still there and still reported by
    /// `DescribeFHIRDatastore`, so `ResourceNotFoundException` stays for a
    /// `DatastoreId` the registry holds no record of.
    ///
    /// Reads the status only, and writes nothing: call
    /// [`ComAmazonawsHealthlakeHealthlakeState::settle_lifecycle`] first, so the
    /// status this gates on is the one the clock already owes.
    pub fn gate_active(&self) -> Result<(), &'static str> {
        match self.status {
            DatastoreStatus::Active => Ok(()),
            DatastoreStatus::Creating
            | DatastoreStatus::Updating
            | DatastoreStatus::Deleting
            | DatastoreStatus::Deleted => Err(DATASTORE_NOT_ACTIVE_MESSAGE),
        }
    }

    /// The FHIR resource of `resource_type` with logical id `id`, or `None` when
    /// this data store holds no such resource.
    pub fn fhir_resource(&self, resource_type: &str, id: &str) -> Option<&FhirResource> {
        self.fhir_resources
            .get(&(resource_type.to_string(), id.to_string()))
    }

    /// Record `configuration` as this data store's backup configuration, taken
    /// at `at`.
    ///
    /// `DatastoreBackupStatus.BackupEnabledAt` is "the time backup was enabled
    /// on the data store", so it is stamped when the configuration becomes
    /// enabled, left alone while it stays enabled (a later configuration change
    /// does not re-enable anything), and cleared when it is not enabled.
    pub fn set_backup_configuration(
        &mut self,
        configuration: BackupConfiguration,
        at: ::aws_smithy_types::DateTime,
    ) {
        let enabled = configuration.status.as_deref() == Some(BACKUP_ENABLED);
        self.backup_configuration = Some(configuration);
        self.backup_enabled_at = match (enabled, self.backup_enabled_at) {
            (true, already_enabled_at) => already_enabled_at.or(Some(at)),
            (false, _) => None,
        };
    }

    /// The window a point-in-time restore of this data store may name, or `None`
    /// when it holds no enabled backup and so cannot be restored at all.
    ///
    /// Continuous backup is a CONTINUOUS window rather than a set of snapshots,
    /// so it runs from the instant backup became enabled on this data store —
    /// nothing before that was ever backed up — no earlier than the retention
    /// period the configuration names (`BackupConfiguration.RetentionPeriodInDays`,
    /// "the number of days backup data is retained"), up to `now`, the latest
    /// instant a continuous backup covers.
    ///
    /// Derived on every call rather than stored: the latest restorable point
    /// advances with the clock, so there is nothing here to keep up to date and
    /// reading it writes nothing.
    pub fn restore_window(&self, now: SystemTime) -> Option<RestoreWindow> {
        let enabled_at = self.backup_enabled_at?;
        let retention_floor = self
            .backup_configuration
            .as_ref()
            .and_then(|configuration| configuration.retention_period_in_days)
            .filter(|days| *days > 0)
            .and_then(|days| {
                now.checked_sub(Duration::from_secs(
                    u64::from(days.unsigned_abs()).saturating_mul(SECONDS_PER_DAY),
                ))
            })
            .map(::aws_smithy_types::DateTime::from);
        // Whichever bound is LATER is the earliest restorable point: backup
        // covers nothing before it was enabled, and nothing older than the
        // retention period is retained.
        let earliest = match retention_floor {
            Some(floor) if epoch_nanos(&floor) > epoch_nanos(&enabled_at) => floor,
            _ => enabled_at,
        };
        let at = ::aws_smithy_types::DateTime::from(now);
        // A window never ends before it starts, whatever the clock did between
        // the instant backup was enabled and this one.
        let latest = if epoch_nanos(&earliest) > epoch_nanos(&at) {
            earliest
        } else {
            at
        };
        Some(RestoreWindow { earliest, latest })
    }

    /// This data store's FHIR resources as they stood at `at`, with the bytes
    /// they hold together.
    ///
    /// A point-in-time restore reproduces the data store as of one instant, so
    /// each resource is taken at the newest version written at or before that
    /// instant — the version a read at that instant would have answered, which
    /// for a resource written once is that one version and for an updated one is
    /// the version the update superseded — and a resource this data store holds
    /// no version of from then (it was created later, or every version it still
    /// retains postdates the instant) is left out. Nothing is reconstructed: a
    /// version is copied exactly as it was stored, or not at all.
    pub fn fhir_resources_as_of(
        &self,
        at: &::aws_smithy_types::DateTime,
    ) -> (BTreeMap<(String, String), FhirResource>, usize) {
        let resources: BTreeMap<(String, String), FhirResource> = self
            .fhir_resources
            .iter()
            .filter_map(|(key, resource)| {
                resource.as_of(at).map(|held| (key.clone(), held))
            })
            .collect();
        let bytes = resources.values().fold(0usize, |total, resource| {
            total.saturating_add(resource.retained_bytes())
        });
        (resources, bytes)
    }

    /// The tags a restore of this data store inherits: its own tags when its
    /// backup configuration includes them, and none otherwise.
    ///
    /// `BackupConfiguration.BackupTagsEnabled` "specifies whether tags are
    /// included in backups", so a backup taken with it unset carries no tags for
    /// a restore to reproduce.
    pub fn backed_up_tags(&self) -> BTreeMap<String, String> {
        let included = self
            .backup_configuration
            .as_ref()
            .is_some_and(|configuration| configuration.backup_tags_enabled);
        if included {
            self.tags.clone()
        } else {
            BTreeMap::new()
        }
    }

    /// The stored tags as the modelled `TagList`, ordered by key.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// The server-side encryption configuration of a data store whose create named no
/// customer-provided key.
///
/// `KmsEncryptionConfig` is documented as "The customer-managed-key (CMK) used when
/// creating a data store. If a customer-owned key is not specified, an Amazon Web
/// Services-owned key is used for encryption", and the modelled `CmkType` enum
/// carries [`AWS_OWNED_KMS_KEY`] for exactly that case. So an absent
/// `SseConfiguration` does not mean an unencrypted data store, and it is not a value
/// to be kept absent: it SELECTS the service-owned key, which every operation that
/// mints a data store materialises here and every read reports.
///
/// No `KmsKeyId`: the member is "The Key Management Service (KMS) encryption key
/// id/alias used to encrypt the data store contents at rest", and a service-owned key
/// is not one a caller names or can name.
pub fn aws_owned_sse_configuration() -> SseConfiguration {
    SseConfiguration {
        kms_encryption_config: KmsEncryptionConfig {
            cmk_type: AWS_OWNED_KMS_KEY.to_string(),
            kms_key_id: None,
        },
    }
}

/// A stored tag map as the modelled `TagList`, ordered by key.
///
/// Every taggable resource in this service holds its tags the same way (a
/// `BTreeMap` keyed by tag key), so `ListTagsForResource` reports all of them
/// through this one function.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The window a point-in-time restore may name on a backup-enabled data store:
/// from the instant backup became enabled (no earlier than the retention period
/// the configuration names) up to now, which is the range
/// `RestoreFHIRDatastore` holds a requested `RestorePointTime` against. The
/// modelled `DatastoreBackupStatus.EarliestRestorePoint` / `LatestRestorePoint`
/// describe the same two instants, and [`Datastore::properties`] reports them
/// from here on every read.
#[derive(Debug, Clone, Copy)]
pub struct RestoreWindow {
    pub earliest: ::aws_smithy_types::DateTime,
    pub latest: ::aws_smithy_types::DateTime,
}

/// Take the transition the clock has ALREADY made due out of `pending`, as
/// `(the status it moves to, the message that status comes with, the WALL-CLOCK
/// instant it fell due)`; `None` while nothing is due yet, which leaves
/// `pending` untouched.
///
/// The wall-clock instant is why this is shared. A due instant is stored as a
/// monotonic [`Instant`] — the only clock a span can be measured on — while
/// every timestamp a response reports is wall-clock, so a record that reports
/// WHEN it changed has to convert the one into the other. `now` and `wall_now`
/// are the two clocks read at the same moment by the caller walking the
/// records: a transition that fell due `now - due` ago fell due that same span
/// before `wall_now`, which is the instant it happened whether or not anybody
/// called — never the instant of the call that materialises it. Every job record
/// here settles through this one function, so no two of them can disagree about
/// which instant that is.
fn take_due_transition<S>(
    pending: &mut Option<(Instant, S, Option<String>)>,
    now: Instant,
    wall_now: SystemTime,
) -> Option<(S, Option<String>, SystemTime)> {
    // Taken only when it is due, so a record whose transition is still ahead of
    // the clock keeps it.
    let (due, next, message) = pending.take_if(|(due, _, _)| now >= *due)?;
    // `due` is at or before `now`, so the span since it fell due is what
    // separates the answer from `wall_now`. A wall clock that cannot reach that
    // far back answers `wall_now` rather than underflowing.
    let due_at = wall_now
        .checked_sub(now.saturating_duration_since(due))
        .unwrap_or(wall_now);
    Some((next, message, due_at))
}

/// The states a FHIR job moves through, as the modelled `JobStatus` enum names
/// them on the wire (`JOB_STATUS_VALUES`).
///
/// A typed status rather than a stored string: a record can only ever hold a
/// value the modelled enum declares, so no read can report a status the model
/// would refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    Queued,
    InProgress,
    Completed,
    CompletedWithErrors,
    Failed,
    CancelSubmitted,
    CancelInProgress,
    CancelCompleted,
    CancelFailed,
}

impl JobStatus {
    /// The modelled enum value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::Queued => "QUEUED",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::CompletedWithErrors => "COMPLETED_WITH_ERRORS",
            Self::Failed => "FAILED",
            Self::CancelSubmitted => "CANCEL_SUBMITTED",
            Self::CancelInProgress => "CANCEL_IN_PROGRESS",
            Self::CancelCompleted => "CANCEL_COMPLETED",
            Self::CancelFailed => "CANCEL_FAILED",
        }
    }

    /// Whether this is a status a job never leaves.
    ///
    /// What makes it matter is the `EndTime` both FHIR job shapes carry
    /// (`ExportJobProperties$EndTime`, "the time the export job completed", and
    /// `ImportJobProperties$EndTime`, "the time the import job was completed")
    /// — which is owed exactly once, when the job reaches one of these and not
    /// while it is still moving.
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Completed
            | Self::CompletedWithErrors
            | Self::Failed
            | Self::CancelCompleted
            | Self::CancelFailed => true,
            Self::Submitted
            | Self::Queued
            | Self::InProgress
            | Self::CancelSubmitted
            | Self::CancelInProgress => false,
        }
    }
}

/// One persisted FHIR export job: what the request that started it asked for,
/// plus the identifier and the status the service owns.
///
/// Held in ONE map beside the data store registry rather than on the data store
/// record, because a job is addressed by its own `JobId`; the data store it
/// belongs to is a field, and the state's own `forget_export_jobs_for` drops a
/// data store's jobs with the data store, so a job never outlives it.
#[derive(Debug, Clone)]
pub struct ExportJob {
    /// The `JobId` the service minted for this job.
    pub id: String,
    /// The `DatastoreId` the job exports from.
    pub datastore_id: String,
    /// The `JobName` the request carried, if any.
    pub name: Option<String>,
    pub status: JobStatus,
    /// The next transition the clock owes this job, as `(due instant, next
    /// status, the message that status comes with)`. `None` when the job is not
    /// waiting on the clock. A job that moves through several statuses re-arms
    /// this as each one falls due, so one slot carries one step.
    ///
    /// The message travels WITH the status it explains, so a record cannot
    /// report a failure's text while it is still running: [`ExportJob::settle`]
    /// writes both in the same step.
    pub pending: Option<(Instant, JobStatus, Option<String>)>,
    /// `SubmitTime`: the instant the job was started.
    pub submitted_at: ::aws_smithy_types::DateTime,
    /// `EndTime`: the instant the job reached a terminal status, written ONCE
    /// by [`ExportJob::settle`] — from the instant that status fell due, not
    /// from the clock of the call that materialised it — so two reads of a
    /// finished job cannot differ.
    pub ended_at: Option<::aws_smithy_types::DateTime>,
    /// The `OutputDataConfig` the request supplied, reported back verbatim.
    pub output_data_config: OutputDataConfig,
    /// The `DataAccessRoleArn` the request supplied.
    pub data_access_role_arn: Option<String>,
    /// `Message`: why the job ended as it did, when there is anything to say.
    pub message: Option<String>,
    /// The `ClientToken` the request that started this job carried, if any.
    ///
    /// Held ON the job rather than in a second map beside the registry, so a
    /// token is dropped with the job it started (and so with the data store the
    /// job belonged to) and cannot resolve to a record that is gone.
    pub client_token: Option<String>,
    /// A fingerprint of the request that started this job, so a `ClientToken`
    /// reused with a DIFFERENT request is refused rather than silently answered
    /// with this job. Built by the operation that mints the record through
    /// [`request_fingerprint`].
    pub request_fingerprint: String,
}

impl ExportJob {
    /// This record as the modelled `ExportJobProperties`, the one shape
    /// `DescribeFHIRExportJob` and `ListFHIRExportJobs` both report.
    pub fn properties(&self) -> ExportJobProperties {
        ExportJobProperties {
            job_id: self.id.clone(),
            job_name: self.name.clone(),
            job_status: self.status.as_wire().to_string(),
            submit_time: self.submitted_at,
            end_time: self.ended_at,
            datastore_id: self.datastore_id.clone(),
            output_data_config: self.output_data_config.clone(),
            data_access_role_arn: self.data_access_role_arn.clone(),
            message: self.message.clone(),
        }
    }

    /// Materialise the transition the clock has ALREADY made due on this job.
    ///
    /// The transition was owed at `now` whether or not anybody called, so a
    /// reporting operation applying it is the record catching up with the clock
    /// rather than a read with a side effect.
    fn settle(&mut self, now: Instant, wall_now: SystemTime) {
        let Some((next, message, due_at)) =
            take_due_transition(&mut self.pending, now, wall_now)
        else {
            return;
        };
        self.status = next;
        // The `Message` is the one the owed status comes with — including none,
        // so a status that explains itself cannot inherit an earlier step's
        // text.
        self.message = message;
        // `EndTime` is owed once, by the step that reaches a status the job
        // never leaves, and is stamped with the instant that step fell DUE —
        // the instant the job ended — then stored, never re-derived per read.
        if next.is_terminal() && self.ended_at.is_none() {
            self.ended_at = Some(::aws_smithy_types::DateTime::from(due_at));
        }
    }
}

/// One persisted FHIR import job: what the request that started it asked for,
/// plus the identifier, the status and the progress the service owns.
///
/// Held in ONE map beside the data store registry, like an [`ExportJob`] and for
/// the same reason: a job is addressed by its own `JobId` within the data store
/// it belongs to, that data store is a field, and the state's own
/// `forget_import_jobs_for` drops a data store's import jobs with the data
/// store, so a job never outlives it.
#[derive(Debug, Clone)]
pub struct ImportJob {
    /// The `JobId` the service minted for this job.
    pub id: String,
    /// The `DatastoreId` the job imports into.
    pub datastore_id: String,
    /// The `JobName` the request carried, if any.
    pub name: Option<String>,
    pub status: JobStatus,
    /// The next transition the clock owes this job, as `(due instant, next
    /// status, the message that status comes with)`. `None` when the job is not
    /// waiting on the clock. A job that moves through several statuses re-arms
    /// this as each one falls due, so one slot carries one step.
    ///
    /// The message travels WITH the status it explains, so a record cannot
    /// report a failure's text while it is still running: [`ImportJob::settle`]
    /// writes both in the same step.
    pub pending: Option<(Instant, JobStatus, Option<String>)>,
    /// `SubmitTime`: the instant the job was submitted for processing.
    pub submitted_at: ::aws_smithy_types::DateTime,
    /// `EndTime`: the instant the job reached a terminal status, written ONCE
    /// by [`ImportJob::settle`] — from the instant that status fell due, not
    /// from the clock of the call that materialised it — so two reads of a
    /// finished job cannot differ.
    pub ended_at: Option<::aws_smithy_types::DateTime>,
    /// The `InputDataConfig` the request supplied, reported back verbatim.
    pub input_data_config: InputDataConfig,
    /// The `JobOutputDataConfig` the request supplied, reported back verbatim.
    pub output_data_config: Option<OutputDataConfig>,
    /// The `DataAccessRoleArn` the request supplied.
    pub data_access_role_arn: Option<String>,
    /// `Message`: why the job ended as it did, when there is anything to say.
    pub message: Option<String>,
    /// `JobProgressReport`: the counts the run itself produced. `None` until
    /// there is anything to report — nothing here is derived on read.
    pub progress_report: Option<JobProgressReport>,
    /// `ValidationLevel` as the request set it, absent when it set none.
    pub validation_level: Option<String>,
    /// The `ClientToken` the request that started this job carried, if any.
    ///
    /// Held ON the job rather than in a second map beside the registry, for the
    /// reason [`ExportJob::client_token`] is: a token is dropped with the job it
    /// started — and so with the data store that job belonged to — and cannot
    /// resolve to a record that is gone.
    pub client_token: Option<String>,
    /// A fingerprint of the request that started this job, so a `ClientToken`
    /// reused with a DIFFERENT request is refused rather than silently answered
    /// with this job. Built by the operation that mints the record through
    /// [`request_fingerprint`].
    pub request_fingerprint: String,
}

impl ImportJob {
    /// This record as the modelled `ImportJobProperties`, the one shape
    /// `DescribeFHIRImportJob` and `ListFHIRImportJobs` both report.
    pub fn properties(&self) -> ImportJobProperties {
        ImportJobProperties {
            job_id: self.id.clone(),
            job_name: self.name.clone(),
            job_status: self.status.as_wire().to_string(),
            submit_time: self.submitted_at,
            end_time: self.ended_at,
            datastore_id: self.datastore_id.clone(),
            input_data_config: self.input_data_config.clone(),
            job_output_data_config: self.output_data_config.clone(),
            job_progress_report: self.progress_report.clone(),
            data_access_role_arn: self.data_access_role_arn.clone(),
            message: self.message.clone(),
            validation_level: self.validation_level.clone(),
        }
    }

    /// Materialise the transition the clock has ALREADY made due on this job.
    ///
    /// The transition was owed at `now` whether or not anybody called, so a
    /// reporting operation applying it is the record catching up with the clock
    /// rather than a read with a side effect.
    fn settle(&mut self, now: Instant, wall_now: SystemTime) {
        let Some((next, message, due_at)) =
            take_due_transition(&mut self.pending, now, wall_now)
        else {
            return;
        };
        self.status = next;
        // The `Message` is the one the owed status comes with — including none,
        // so a status that explains itself cannot inherit an earlier step's
        // text.
        self.message = message;
        // `EndTime` is owed once, by the step that reaches a status the job
        // never leaves, and is stamped with the instant that step fell DUE —
        // the instant the job ended — then stored, never re-derived per read.
        if next.is_terminal() && self.ended_at.is_none() {
            self.ended_at = Some(::aws_smithy_types::DateTime::from(due_at));
        }
    }
}

/// The states a data transformation job moves through, as the modelled
/// `TransformationJobStatus` enum names them on the wire
/// (`TRANSFORMATION_JOB_STATUS_VALUES`).
///
/// Kept apart from [`JobStatus`] — the FHIR import/export job status — because
/// the two are DIFFERENT modelled enums: `TransformationJobStatus` declares no
/// `CANCEL_*` value (a transformation job models no cancel operation), so a
/// record of this kind can never report one. A typed status rather than a
/// stored string, so no read can report a status the model would refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformationJobStatus {
    Submitted,
    Queued,
    InProgress,
    Completed,
    CompletedWithErrors,
    Failed,
}

impl TransformationJobStatus {
    /// The modelled enum value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::Queued => "QUEUED",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::CompletedWithErrors => "COMPLETED_WITH_ERRORS",
            Self::Failed => "FAILED",
        }
    }

    /// Whether this is a status a transformation job never leaves.
    ///
    /// What makes it matter is `TransformationJobProperties$EndTime` — "the
    /// timestamp when the job completed or failed" — which is owed exactly
    /// once, when the job reaches one of these and not while it is still
    /// moving.
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Completed | Self::CompletedWithErrors | Self::Failed => true,
            Self::Submitted | Self::Queued | Self::InProgress => false,
        }
    }
}

/// One persisted data transformation job: what the request that started it
/// asked for, plus the identifier, the status and the progress the service
/// owns.
///
/// Held in ONE map keyed by `JobId`, the identifier
/// `DescribeDataTransformationJob` addresses a job by. Unlike an
/// [`ExportJob`] a transformation job belongs to no data store — it reads and
/// writes Amazon S3 — so there is no record it is dropped with; the profile it
/// names is recorded by id, NAME and VERSION as the start call resolved them,
/// so a read reports what the job ran with rather than re-deriving it from a
/// profile that may since have changed.
#[derive(Debug, Clone)]
pub struct TransformationJob {
    /// The `JobId` the service minted for this job.
    pub id: String,
    /// The `JobName` the request carried, if any.
    pub name: Option<String>,
    pub status: TransformationJobStatus,
    /// The next transition the clock owes this job, as `(due instant, next
    /// status, the message that status comes with)`. `None` when the job is not
    /// waiting on the clock. A job that moves through several statuses re-arms
    /// this as each one falls due, so one slot carries one step. The message
    /// travels WITH the status, like an [`ExportJob`]'s, so a record cannot
    /// report why it ended while it is still running.
    pub pending: Option<(Instant, TransformationJobStatus, Option<String>)>,
    /// `SubmitTime`: the instant the job was submitted.
    pub submitted_at: ::aws_smithy_types::DateTime,
    /// `EndTime`: the instant the job reached a terminal status, written ONCE
    /// by [`TransformationJob::settle`] — from the instant that status fell
    /// due, not from the clock of the call that materialised it — so two reads
    /// of a finished job cannot differ.
    pub ended_at: Option<::aws_smithy_types::DateTime>,
    /// The `InputDataConfig` the request supplied, reported back verbatim.
    pub input_data_config: TransformationInputDataConfig,
    /// The `OutputDataConfig` the request supplied, reported back verbatim.
    pub output_data_config: TransformationOutputDataConfig,
    /// The `DataAccessRoleArn` the request supplied.
    pub data_access_role_arn: String,
    /// The `ProfileId` the request named.
    pub profile_id: Option<String>,
    /// The profile's `ProfileName` as it stood when the job was submitted.
    pub profile_name: Option<String>,
    /// The profile's `ProfileVersion` the job runs against.
    pub profile_version: Option<i32>,
    /// `DriftDetectionEnabled` as the request set it, absent when it set none.
    pub drift_detection_enabled: Option<bool>,
    /// `ProvenanceEnabled` as the request set it, including the modelled
    /// default the boundary applies.
    pub provenance_enabled: Option<bool>,
    /// `Message`: why the job ended as it did, when there is anything to say.
    pub message: Option<String>,
    /// `JobProgressReport`: the counts the run itself produced. `None` until
    /// there is anything to report — nothing here is derived on read.
    pub progress_report: Option<TransformationJobProgressReport>,
    /// The `ClientToken` the start call carried, held ON the record so one
    /// token resolves to the one job it started. `None` on a record that was
    /// not started through the API.
    pub client_token: Option<String>,
}

impl TransformationJob {
    /// This record as the modelled `TransformationJobProperties`, the shape
    /// `DescribeDataTransformationJob` reports a job through.
    pub fn properties(&self) -> TransformationJobProperties {
        TransformationJobProperties {
            job_id: self.id.clone(),
            job_status: self.status.as_wire().to_string(),
            input_data_config: self.input_data_config.clone(),
            output_data_config: self.output_data_config.clone(),
            data_access_role_arn: self.data_access_role_arn.clone(),
            submit_time: self.submitted_at,
            job_name: self.name.clone(),
            profile_id: self.profile_id.clone(),
            profile_name: self.profile_name.clone(),
            profile_version: self.profile_version,
            end_time: self.ended_at,
            drift_detection_enabled: self.drift_detection_enabled,
            provenance_enabled: self.provenance_enabled,
            message: self.message.clone(),
            job_progress_report: self.progress_report.clone(),
        }
    }

    /// This record as the modelled `TransformationJobSummary`, the shape
    /// `ListDataTransformationJobs` reports a job through: the subset of
    /// [`Self::properties`] that shape declares, read off the SAME record, so
    /// the two operations cannot disagree about one job.
    pub fn summary(&self) -> TransformationJobSummary {
        TransformationJobSummary {
            job_id: self.id.clone(),
            job_status: self.status.as_wire().to_string(),
            submit_time: self.submitted_at,
            job_name: self.name.clone(),
            end_time: self.ended_at,
            // `TransformationJobSummary$SourceFormat` is "the source data
            // format for this job", which is the format the job's own
            // `InputDataConfig` names — reported from there rather than
            // stored twice.
            source_format: self.input_data_config.source_format.clone(),
        }
    }

    /// Materialise the transition the clock has ALREADY made due on this job.
    ///
    /// The transition was owed at `now` whether or not anybody called, so a
    /// reporting operation applying it is the record catching up with the clock
    /// rather than a read with a side effect.
    fn settle(&mut self, now: Instant, wall_now: SystemTime) {
        let Some((next, message, due_at)) =
            take_due_transition(&mut self.pending, now, wall_now)
        else {
            return;
        };
        self.status = next;
        // The `Message` is the one the owed status comes with — including none,
        // so a status that explains itself cannot inherit an earlier step's
        // text.
        self.message = message;
        // `EndTime` is owed once, by the step that reaches a status the job
        // never leaves, and is stamped with the instant that step fell DUE —
        // the instant the job ended — then stored, never re-derived per read.
        if next.is_terminal() && self.ended_at.is_none() {
            self.ended_at = Some(::aws_smithy_types::DateTime::from(due_at));
        }
    }
}

/// What one `CreateFHIRDatastore` `ClientToken` recorded: the data store it
/// minted, and a fingerprint of the request that minted it, so a token reused
/// with a DIFFERENT request is refused instead of silently answering with the
/// earlier data store.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub datastore_id: String,
    pub fingerprint: String,
}

/// What one `CreateDataTransformationProfile` `ClientToken` recorded: the
/// profile it minted.
///
/// Kept apart from [`ClientTokenRecord`] because the two operations mint
/// different resources: one caller's token for a data store must never resolve
/// to a profile, or the other way round. It holds no request fingerprint: the
/// model documents that operation's token as making the service IGNORE a
/// repeat, so what the repeat asked for is never compared with what the first
/// call asked for.
#[derive(Debug, Clone)]
pub struct ProfileClientTokenRecord {
    pub profile_id: String,
}

/// One version of a data transformation profile: its content, why it changed,
/// and when it last did.
///
/// The DRAFT (version [`DRAFT_PROFILE_VERSION`]) is the mutable one; a
/// published version is an immutable snapshot of the DRAFT taken when it was
/// published, which is why the two are the same record type.
#[derive(Debug, Clone)]
pub struct ProfileVersionRecord {
    pub version: i32,
    /// The profile content, keyed by file path. A `BTreeMap` so the content a
    /// caller reads back is ordered the same way on every read. Satisfies the
    /// modelled `ProfileMapping` bounds — see [`validate_profile_mapping`].
    pub mapping: BTreeMap<String, String>,
    pub change_description: Option<String>,
    pub last_updated_at: ::aws_smithy_types::DateTime,
}

impl ProfileVersionRecord {
    /// The content as the modelled `ProfileMapping`, the shape
    /// `GetDataTransformationProfile` reports it through.
    pub fn profile_mapping(&self) -> ::std::collections::HashMap<String, String> {
        self.mapping
            .iter()
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect()
    }

    /// How many BYTES of keys and values this version's content holds together —
    /// what [`MAX_PROFILE_MAPPING_BYTES`] bounds for one version and
    /// [`MAX_PROFILE_RETAINED_BYTES`] bounds across a profile's versions.
    pub fn retained_bytes(&self) -> usize {
        mapping_bytes(&self.mapping)
    }
}

/// How many BYTES of keys and values `mapping` holds together, in the one place
/// every profile byte budget measures it, so a stored version and a candidate one
/// are never measured two different ways.
pub fn mapping_bytes(mapping: &BTreeMap<String, String>) -> usize {
    mapping.iter().fold(0usize, |total, (path, content)| {
        total.saturating_add(path.len()).saturating_add(content.len())
    })
}

/// Whether a holder that RETAINS `retained` bytes stays inside `ceiling` once a
/// write adds `added` bytes of content and gives `released` of the bytes already
/// counted back.
///
/// The arithmetic EVERY byte ceiling in this service is charged with, in one place
/// so the per-holder and the store-wide ceilings — and each write that charges
/// them, whether it mints, replaces or snapshots content — can never measure the
/// same write two different ways. `saturating_*`, so no arithmetic edge can wrap
/// into an admission.
pub fn retained_bytes_admits(
    retained: usize,
    added: usize,
    released: usize,
    ceiling: usize,
) -> bool {
    retained.saturating_sub(released).saturating_add(added) <= ceiling
}

/// One persisted data transformation profile: the metadata
/// `CreateDataTransformationProfile` was given, plus its DRAFT and every
/// version published from it.
#[derive(Debug)]
pub struct DataTransformationProfile {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The format the profile converts FROM, as the modelled `SourceFormat`
    /// enum spells it on the wire.
    pub source_format: String,
    /// The format the profile converts TO. The modelled `TargetFormat` enum
    /// declares exactly one value, which `CreateDataTransformationProfile`
    /// documents as "Always FHIR_R4" — stored rather than re-derived so a
    /// later value of the enum does not silently change an existing profile.
    pub target_format: String,
    pub kms_key_id: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    /// Version [`DRAFT_PROFILE_VERSION`], the one `UpdateDataTransformationProfile`
    /// rewrites in place. A profile always has one: it is created in DRAFT state.
    pub draft: ProfileVersionRecord,
    /// The published versions, by version number (1..=[`MAX_PROFILE_VERSION`]).
    /// A `BTreeMap` so "the latest published version" is its last key and a
    /// listing is in a stable order.
    pub published: BTreeMap<i32, ProfileVersionRecord>,
    /// Tags by key, seeded from `CreateDataTransformationProfile`'s `Tags`, so
    /// the reported order is stable across reads.
    pub tags: BTreeMap<String, String>,
}

impl DataTransformationProfile {
    /// The status this service reports this profile in.
    ///
    /// Read off the record: the DRAFT exists from the instant the create that
    /// minted it returned, so a profile this service holds owes the clock no
    /// transition and nothing about it is pending — it is reported
    /// [`ACTIVE_PROFILE_STATUS`]. `ProfileStatus` is not a member of the model
    /// snapshot's profile shapes; the service reports it in the create reply, and
    /// this is the one place that answer is decided.
    pub fn status(&self) -> &'static str {
        ACTIVE_PROFILE_STATUS
    }

    /// The version `version` names: the DRAFT at [`DRAFT_PROFILE_VERSION`], a
    /// published snapshot above it, or `None` when this profile has no such
    /// version.
    pub fn version(&self, version: i32) -> Option<&ProfileVersionRecord> {
        if version == DRAFT_PROFILE_VERSION {
            Some(&self.draft)
        } else {
            self.published.get(&version)
        }
    }

    /// The version a read that names NO version resolves to: the latest published
    /// version, and the DRAFT while the profile has none.
    ///
    /// `GetDataTransformationProfile` documents the first half ("If you omit this
    /// parameter, the service returns the latest published version") and the
    /// recorded wire shows the second: a profile that has been created, and
    /// perhaps updated, but never published is READ BACK at its DRAFT rather than
    /// refused, so the authoring loop create -> get and create -> update -> get
    /// serves the content the caller just wrote.
    pub fn latest_readable(&self) -> &ProfileVersionRecord {
        self.latest_published().unwrap_or(&self.draft)
    }

    /// The stored tags as the modelled `TagList`, ordered by key — the same
    /// shape `ListTagsForResource` reports a data store's tags through.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// The highest published version, or `None` while the profile has only its
    /// DRAFT — which is what `GetDataTransformationProfile` resolves when the
    /// caller omits the version ("the service returns the latest published
    /// version").
    pub fn latest_published(&self) -> Option<&ProfileVersionRecord> {
        self.published.values().next_back()
    }

    /// Every version this profile holds — its DRAFT and each version published
    /// from it — in REVERSE CHRONOLOGICAL order, newest first, which is the
    /// order `ListDataTransformationProfileVersions` reports them in ("all
    /// versions of a specific data transformation profile (DRAFT and
    /// published), in reverse chronological order (newest first)").
    ///
    /// "Chronological" is measured on when a version CAME INTO BEING, which is
    /// the one thing about a version no later call can rewrite: the DRAFT is
    /// minted with the profile and each publish mints the version one past the
    /// latest published one, so the version numbers run in the order the
    /// versions were created and DESCENDING version number is exactly newest
    /// first — with the DRAFT, minted at create, last. Nothing renumbers or
    /// removes a version, so the order is a total one that no write can
    /// reshuffle, and a page resumed into it by version number lands where it
    /// was issued for.
    ///
    /// A version's `LastUpdatedAt` cannot order this listing: it is rewritten
    /// in place on the DRAFT, which would move an entry between two pages of
    /// one walk and serve another entry twice. The walk is bounded: at most
    /// `1 + MAX_PROFILE_VERSION` versions.
    pub fn versions_newest_first(&self) -> Vec<&ProfileVersionRecord> {
        let mut versions: Vec<&ProfileVersionRecord> = ::std::iter::once(&self.draft)
            .chain(self.published.values())
            .collect();
        versions.sort_by_key(|version| ::std::cmp::Reverse(version.version));
        versions
    }

    /// One of this profile's versions as the modelled
    /// `DataTransformationProfileVersionSummary`, the shape
    /// `ListDataTransformationProfileVersions` reports a version through.
    ///
    /// The metadata comes from the profile and the per-version members from the
    /// version, so a summary and the `GetDataTransformationProfile` of the same
    /// version can never disagree. The content itself is NOT summarised: the
    /// model sends a caller to `GetDataTransformationProfile` for it.
    pub fn version_summary(
        &self,
        version: &ProfileVersionRecord,
    ) -> DataTransformationProfileVersionSummary {
        DataTransformationProfileVersionSummary {
            profile_id: self.id.clone(),
            version: version.version,
            source_format: self.source_format.clone(),
            target_format: self.target_format.clone(),
            profile_name: Some(self.name.clone()),
            change_description: version.change_description.clone(),
            last_updated_at: Some(version.last_updated_at),
        }
    }

    /// The LATEST version of this profile: the highest version it holds, which is
    /// the highest published one, or its DRAFT while nothing has been published
    /// from it. A published version is numbered above the DRAFT's
    /// [`DRAFT_PROFILE_VERSION`], so "highest" and "latest" are the same version.
    pub fn latest_version(&self) -> &ProfileVersionRecord {
        self.latest_published().unwrap_or(&self.draft)
    }

    /// When this PROFILE last changed: the most recent instant any version of it
    /// was last updated, across its DRAFT and every version published from it.
    ///
    /// Measured over the versions rather than read off the latest one, so a DRAFT
    /// rewritten after the last publish still moves it — a write to any version is
    /// a write to the profile. The walk is bounded: at most
    /// `1 + MAX_PROFILE_VERSION` versions.
    pub fn last_updated_at(&self) -> ::aws_smithy_types::DateTime {
        ::std::iter::once(&self.draft)
            .chain(self.published.values())
            .map(|version| version.last_updated_at)
            .max_by_key(epoch_nanos)
            // A profile always holds its DRAFT, so the iterator is never empty;
            // the create instant is the floor if that ever stops being true.
            .unwrap_or(self.created_at)
    }

    /// This profile as the modelled `DataTransformationProfileSummary`, the shape
    /// `ListDataTransformationProfiles` reports a profile through — one entry per
    /// profile, carrying its latest version number and the instant the profile was
    /// last updated.
    ///
    /// The metadata comes from the profile itself, so a summary and the
    /// `GetDataTransformationProfile` of the same version can never disagree. The
    /// content is NOT summarised: the model sends a caller to
    /// `GetDataTransformationProfile` for it.
    pub fn summary(&self) -> DataTransformationProfileSummary {
        DataTransformationProfileSummary {
            profile_id: self.id.clone(),
            version: self.latest_version().version,
            source_format: self.source_format.clone(),
            target_format: self.target_format.clone(),
            profile_name: Some(self.name.clone()),
            profile_description: self.description.clone(),
            last_updated_at: Some(self.last_updated_at()),
        }
    }

    /// How many BYTES of content this profile retains across its DRAFT and every
    /// version published from it — what [`MAX_PROFILE_RETAINED_BYTES`] bounds.
    ///
    /// Summed over the versions rather than tracked as a second counter, so a
    /// version's bytes are given back by the version being dropped and the two can
    /// never disagree. The walk is bounded: at most `1 + MAX_PROFILE_VERSION`
    /// versions, each of at most [`MAX_PROFILE_MAPPING_ENTRIES`] entries.
    pub fn retained_bytes(&self) -> usize {
        self.published
            .values()
            .fold(self.draft.retained_bytes(), |total, version| {
                total.saturating_add(version.retained_bytes())
            })
    }

    /// Whether THIS profile can retain `added` more BYTES of version content once
    /// the `released` bytes a write replaces are given back — what
    /// [`MAX_PROFILE_RETAINED_BYTES`] bounds.
    ///
    /// The ONE place the per-profile byte refusal is decided, so every operation
    /// that grows what one profile retains charges it on the same terms: a publish
    /// adds a snapshot and releases nothing, an update replaces the DRAFT and so
    /// gives the DRAFT's bytes back.
    pub fn admits_retained_bytes(&self, added: usize, released: usize) -> bool {
        retained_bytes_admits(
            self.retained_bytes(),
            added,
            released,
            MAX_PROFILE_RETAINED_BYTES,
        )
    }
}

/// The characters a mapping file's name is built from: the name it is derived from
/// lowercased, with every run of anything else collapsed to one of these.
const CONTENT_NAME_SEPARATOR: char = '-';

/// `name` as the name of a file in a profile's mapping: lowercased, with every run
/// of anything that is not a letter or a digit collapsed to one
/// [`CONTENT_NAME_SEPARATOR`] and none left at either end.
///
/// The ONE place a mapping file is named after something the service read — a C-CDA
/// section of a starter profile ([`starter_profiles`]) or a column of a sample data
/// file ([`sample_data`]) — so both resolvers name their files the same way.
pub(crate) fn content_file_slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_alphanumeric() {
            slug.extend(character.to_lowercase());
        } else if !slug.ends_with(CONTENT_NAME_SEPARATOR) {
            slug.push(CONTENT_NAME_SEPARATOR);
        }
    }
    slug.trim_matches(CONTENT_NAME_SEPARATOR).to_string()
}

/// `name`'s letters and digits, lowercased: the form two spellings of one name are
/// compared in.
///
/// The ONE place a name a caller or a file supplies is matched against a name this
/// service publishes — a starter profile of the library, a FHIR R4 element a sample
/// column carries — so neither resolver decides a match on case or on how the name's
/// words are separated.
pub(crate) fn normalized_content_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The file every resolved profile mapping carries beside its per-entry files:
/// which source the content came from and what it converts between.
///
/// Named in one place because both resolvers that BUILD content — the starter
/// profile library ([`starter_profiles`]) and the sample data inference
/// ([`sample_data`]) — emit it, so the two cannot name it differently.
pub(crate) const PROFILE_CONTENT_MANIFEST_FILE: &str = "profile.json";

/// The format a profile's content converts TO, from the modelled `TargetFormat`
/// enum — which declares exactly one value, the one
/// `CreateDataTransformationProfile` documents as "Always FHIR_R4".
///
/// Read from the model in ONE place, so no resolver writes a target format out by
/// hand or reads a different member of the enum than another.
pub(crate) fn profile_target_format() -> &'static str {
    crate::ops::types::TARGET_FORMAT_VALUES
        .first()
        .copied()
        .unwrap_or_default()
}

/// Refuse profile content the modelled `ProfileMapping` could not carry, with
/// the member and the bound it violates.
///
/// The create/update SOURCE shapes take an unconstrained `StringMap`, but the
/// content is read back through `ProfileMapping` (1..500 entries, keys 1..500,
/// values 0..102400), so content past those bounds could be stored and never
/// read. `Ok(())` means every bound holds.
pub fn validate_profile_mapping(mapping: &BTreeMap<String, String>) -> Result<(), String> {
    if mapping.len() < MIN_PROFILE_MAPPING_ENTRIES {
        return Err(format!(
            "ProfileMapping must hold at least {MIN_PROFILE_MAPPING_ENTRIES} entry, got {}",
            mapping.len()
        ));
    }
    if mapping.len() > MAX_PROFILE_MAPPING_ENTRIES {
        return Err(format!(
            "ProfileMapping must hold at most {MAX_PROFILE_MAPPING_ENTRIES} entries, got {}",
            mapping.len()
        ));
    }
    for (path, content) in mapping {
        if path.is_empty() || path.chars().count() > MAX_PROFILE_MAPPING_KEY_LEN {
            return Err(format!(
                "every ProfileMapping key must be 1..{MAX_PROFILE_MAPPING_KEY_LEN} characters long, got one of {}",
                path.chars().count()
            ));
        }
        if content.chars().count() > MAX_PROFILE_MAPPING_VALUE_LEN {
            return Err(format!(
                "every ProfileMapping value must be at most {MAX_PROFILE_MAPPING_VALUE_LEN} characters long, got one of {}",
                content.chars().count()
            ));
        }
    }
    // Every entry can satisfy its own modelled bound and the mapping still be far
    // larger than this emulator retains, so the bytes they hold TOGETHER are bounded
    // too — see [`MAX_PROFILE_MAPPING_BYTES`].
    let bytes = mapping_bytes(mapping);
    if bytes > MAX_PROFILE_MAPPING_BYTES {
        return Err(format!(
            "ProfileMapping must hold at most {MAX_PROFILE_MAPPING_BYTES} bytes of keys and \
             values together, got {bytes}"
        ));
    }
    Ok(())
}

/// Per-service state for com-amazonaws-healthlake-healthlake. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsHealthlakeHealthlakeState {
    /// Every live data store, by `DatastoreId`. A `BTreeMap` so
    /// `ListFHIRDatastores` pages in a stable, resumable order. Bounded by
    /// [`MAX_DATASTORES`]; a `DELETED` record is reaped after
    /// [`DELETED_RETENTION`].
    pub datastores: BTreeMap<String, Datastore>,
    /// Data-store idempotency: `ClientToken` -> what the first call with that
    /// token minted. ONE map for both operations that mint a data store
    /// (`CreateFHIRDatastore` and `RestoreFHIRDatastore`), so one caller's token
    /// names one data store whichever of them minted it; each records a
    /// fingerprint of its own request, so a token reused with a different
    /// request — including the other operation's — is refused rather than
    /// answered with the earlier data store. Bounded by [`MAX_CLIENT_TOKENS`]; write it through
    /// [`Self::record_client_token`], never directly.
    pub client_tokens: BTreeMap<String, ClientTokenRecord>,
    /// The order [`Self::client_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    client_token_order: VecDeque<String>,
    /// How many data stores have been minted, mixed into the next identifier so
    /// two creates in the same clock tick still get distinct ones.
    pub datastores_created: u64,
    /// How many FHIR resources have been minted across every data store, mixed
    /// into the next logical id for the same reason.
    pub fhir_resources_created: u64,
    /// Every data transformation profile, by `ProfileId`. A `BTreeMap` so
    /// `ListDataTransformationProfiles` pages in a stable, resumable order.
    /// Bounded by [`MAX_PROFILES`].
    pub profiles: BTreeMap<String, DataTransformationProfile>,
    /// `CreateDataTransformationProfile` idempotency: `ClientToken` -> what the
    /// first call with that token minted. Bounded by
    /// [`MAX_PROFILE_CLIENT_TOKENS`]; write it through
    /// [`Self::record_profile_client_token`], never directly. Every entry names
    /// a profile this store still holds: a profile leaves through
    /// [`Self::remove_profile`], which drops the tokens that minted it, so a
    /// token cannot resolve to a record that is gone.
    pub profile_client_tokens: BTreeMap<String, ProfileClientTokenRecord>,
    /// The order [`Self::profile_client_tokens`] was written in, so the cap
    /// evicts the oldest token rather than an arbitrary one.
    profile_client_token_order: VecDeque<String>,
    /// How many profiles have been minted, mixed into the next identifier so two
    /// creates in the same clock tick still get distinct ones.
    pub profiles_created: u64,
    /// Every FHIR export job, by `JobId`. A `BTreeMap` so
    /// `ListFHIRExportJobs` pages in a stable, resumable order. A job's record
    /// is dropped with the data store it belongs to, by this state's own
    /// `drop_datastore`. Bounded by [`MAX_EXPORT_JOBS`], which
    /// `StartFHIRExportJob` — the only operation that mints one — refuses past.
    pub export_jobs: BTreeMap<String, ExportJob>,
    /// How many FHIR export jobs have been started, mixed into the next `JobId`
    /// so two starts in the same clock tick still get distinct ones.
    pub export_jobs_started: u64,
    /// Every FHIR import job, by `JobId`. A `BTreeMap` so
    /// `ListFHIRImportJobs` pages in a stable, resumable order. A job's record
    /// is dropped with the data store it belongs to, by
    /// this state's own `drop_datastore`. Bounded by [`MAX_IMPORT_JOBS`], which
    /// `StartFHIRImportJob` — the only operation that mints one — refuses past.
    pub import_jobs: BTreeMap<String, ImportJob>,
    /// How many FHIR import jobs have been started, mixed into the next `JobId`
    /// so two starts in the same clock tick still get distinct ones.
    pub import_jobs_started: u64,
    /// Every data transformation job, by `JobId` — the identifier
    /// `DescribeDataTransformationJob` addresses one by. A `BTreeMap` so
    /// `ListDataTransformationJobs` pages in a stable, resumable order. A
    /// transformation job belongs to no data store, so no
    /// record reaps one. Bounded by [`MAX_TRANSFORMATION_JOBS`], which
    /// `StartDataTransformationJob` — the only operation that mints one —
    /// refuses past.
    pub transformation_jobs: BTreeMap<String, TransformationJob>,
    /// How many data transformation jobs have been started, mixed into the next
    /// `JobId` so two starts in the same clock tick still get distinct ones.
    pub transformation_jobs_started: u64,
    /// Every live data transformation agent conversation, by `ConversationId` —
    /// the multi-turn chat `UpdateProfileWithAgent` serves one turn of. A
    /// `BTreeMap` so an eviction walk is in a stable order. Bounded by
    /// [`MAX_AGENT_CONVERSATIONS`]; an entry is swept once
    /// [`AGENT_CONVERSATION_RETENTION`] has run out
    /// ([`Self::sweep_agent_conversations`]).
    pub agent_conversations: BTreeMap<String, AgentConversation>,
    /// How many conversations have been minted, mixed into the next
    /// `ConversationId` so two first turns in the same clock tick still get
    /// distinct ones.
    pub agent_conversations_started: u64,
}

impl ComAmazonawsHealthlakeHealthlakeState {
    /// How many BYTES of FHIR resources every data store retains together.
    ///
    /// Summed over the data stores' own counters rather than tracked as a second
    /// total, so a reaped data store gives its bytes back by being dropped and
    /// the two can never disagree. The walk is bounded by [`MAX_DATASTORES`].
    pub fn fhir_resource_bytes_total(&self) -> usize {
        self.datastores
            .values()
            .fold(0usize, |total, datastore| {
                total.saturating_add(datastore.fhir_resource_bytes)
            })
    }

    /// How many BYTES of profile content every data transformation profile
    /// retains together — what [`MAX_PROFILE_RETAINED_BYTES_TOTAL`] bounds.
    ///
    /// Summed over the profiles' own versions, for the reason
    /// [`Self::fhir_resource_bytes_total`] is summed over the data stores: a
    /// dropped profile gives its bytes back by being dropped. The walk is bounded
    /// by [`MAX_PROFILES`] times what [`DataTransformationProfile::retained_bytes`]
    /// walks.
    pub fn profile_retained_bytes_total(&self) -> usize {
        self.profiles.values().fold(0usize, |total, profile| {
            total.saturating_add(profile.retained_bytes())
        })
    }

    /// Whether every data transformation profile TOGETHER can retain `added` more
    /// BYTES of version content once the `released` bytes a write replaces are
    /// given back — what [`MAX_PROFILE_RETAINED_BYTES_TOTAL`] bounds.
    ///
    /// The ONE place the store-wide profile byte refusal is decided, for the reason
    /// [`Self::admits_fhir_resource_bytes`] is the one place the FHIR one is: every
    /// operation that adds retained profile content charges the ceiling before it
    /// writes, so none of them can push the store past a ceiling another one is
    /// then refused for. Nothing is swept first: a profile's bytes come back
    /// exactly by the record being dropped ([`Self::profile_retained_bytes_total`]
    /// is summed over the records), and a profile leaves only when a caller deletes
    /// it, so there is no terminal profile record lingering past its own
    /// retention.
    pub fn admits_profile_retained_bytes(&self, added: usize, released: usize) -> bool {
        retained_bytes_admits(
            self.profile_retained_bytes_total(),
            added,
            released,
            MAX_PROFILE_RETAINED_BYTES_TOTAL,
        )
    }

    /// Materialise every lifecycle transition the clock has ALREADY made due.
    ///
    /// Called by the operations that report a data store's status as well as by
    /// the ones that change it: the transition was owed at this instant whether
    /// or not anybody called, so applying it on read is not a read with a side
    /// effect — it is the record catching up with the clock.
    /// The walk is bounded by [`MAX_DATASTORES`], so it costs the same whatever
    /// a caller does.
    pub fn settle_lifecycle(&mut self) {
        let now = Instant::now();
        for datastore in self.datastores.values_mut() {
            if let Some((due, next)) = datastore.pending {
                if now >= due {
                    datastore.status = next;
                    datastore.pending = None;
                    // A record that has reached its terminal status starts its
                    // retention clock; everything else stays live. The
                    // retention runs from the instant the record BECAME
                    // terminal — the instant the transition fell due — not from
                    // the call that materialised it, so a store a caller polls
                    // late is not kept past its retention.
                    if next == DatastoreStatus::Deleted {
                        datastore.reap_at = Some(due + DELETED_RETENTION);
                    }
                }
            }
        }
        self.reap_expired_records(now);
    }

    /// Drop every terminal record whose retention has run out, and the
    /// idempotency tokens that named it, so a deleted data store does not
    /// outlive its own retention.
    fn reap_expired_records(&mut self, now: Instant) {
        self.reap_datastores_where(|datastore| datastore.reap_at.is_some_and(|at| now >= at));
    }

    /// Drop every held data store `select` picks out, each with everything this
    /// state holds on its behalf.
    ///
    /// The body the reapers share: they differ only in WHICH records they
    /// select, so the selection is the parameter here and the removal is
    /// [`Self::drop_datastore`]. Collected first, because the walk that selects
    /// borrows the registry the removal writes to. Bounded by
    /// [`MAX_DATASTORES`].
    fn reap_datastores_where(&mut self, select: impl Fn(&Datastore) -> bool) {
        let reaped: Vec<String> = self
            .datastores
            .iter()
            .filter(|(_, datastore)| select(datastore))
            .map(|(id, _)| id.clone())
            .collect();
        for id in reaped {
            self.drop_datastore(&id);
        }
    }

    /// Drop the data store `datastore_id` and everything held for it: the
    /// registry entry, the idempotency tokens that named it, its export jobs
    /// and its import jobs.
    ///
    /// The ONE place that says what dropping a data store MEANS — the shape
    /// [`Self::remove_profile`] takes for a profile — so a per-data-store
    /// collection this module gains later is forgotten by every path that drops
    /// a record rather than by whichever path its author remembered.
    fn drop_datastore(&mut self, datastore_id: &str) {
        self.datastores.remove(datastore_id);
        self.forget_client_tokens_for(datastore_id);
        self.forget_export_jobs_for(datastore_id);
        self.forget_import_jobs_for(datastore_id);
    }

    /// The tag map a tagging MUTATION may write to for `arn`, or the fixed
    /// not-found sentence of the resource that ARN names.
    ///
    /// A tag write CHANGES the resource its `ResourceARN` names, so it reaches
    /// only a resource this service still holds. Two things decide that, and
    /// both are decided here rather than in each tagging handler, so `TagResource`
    /// and `UntagResource` cannot disagree about which records take a write:
    ///
    /// * the stored record's own ARN has to equal the one the caller handed in,
    ///   so an ARN naming another partition, region or account resolves to
    ///   nothing even when its id segment happens to match a local one; and
    /// * a data store that has reached the terminal [`DatastoreStatus::Deleted`]
    ///   status is not held any more in the sense a mutation needs: the record
    ///   lingers for [`DELETED_RETENTION`] only so a reader can observe the
    ///   deletion, and every operation that would CHANGE such a data store
    ///   answers for it as gone. A data transformation profile carries no
    ///   lifecycle status, so the ARN match is all there is to decide for it.
    ///
    /// Call [`Self::settle_lifecycle`] first, so the status this reads is the one
    /// the clock already owes.
    pub fn tags_for_mutation(&mut self, arn: &str) -> Result<MutableTags<'_>, String> {
        match tagged_resource_from_arn(arn) {
            Some(TaggedResource::Datastore(id)) => self
                .datastores
                .get_mut(id)
                .filter(|datastore| datastore.arn == arn)
                .filter(|datastore| datastore.status != DatastoreStatus::Deleted)
                .map(|datastore| MutableTags {
                    tags: &mut datastore.tags,
                    max_tags: MAX_TAGS_PER_DATASTORE,
                    noun: DATASTORE_NOUN,
                })
                .ok_or_else(|| DATASTORE_NOT_FOUND_MESSAGE.to_string()),
            // The profile texts name the profile, so the id the ARN carries is
            // what the refusal states — the same template every other profile
            // not-found path in this crate states.
            Some(TaggedResource::Profile(id)) => {
                let message = profile_not_found_message(id);
                self.profiles
                    .get_mut(id)
                    .filter(|profile| profile.arn == arn)
                    .map(|profile| MutableTags {
                        tags: &mut profile.tags,
                        max_tags: MAX_TAGS_PER_PROFILE,
                        noun: PROFILE_NOUN,
                    })
                    .ok_or(message)
            }
            // Unreachable through the wire — the modelled `AmazonResourceName`
            // pattern is enforced before a handler runs — but an ARN of neither
            // resource form names no resource, so it is answered the same way a
            // missing one is.
            None => Err(DATASTORE_NOT_FOUND_MESSAGE.to_string()),
        }
    }

    /// The tags a READER reports for the resource `arn` names, or the sentence
    /// that states which resource was not found.
    ///
    /// The reporting counterpart of [`Self::tags_for_mutation`], and the ONE
    /// place a tag REPORT resolves its `ResourceArn`, so every wire that reports
    /// a resource's tags — the awsJson `ListTagsForResource` and the
    /// data-transformation endpoint's REST `GET /tags/{ResourceArn}` — resolves
    /// and refuses on identical terms. Whichever resource the ARN names, the
    /// stored record's own ARN must equal the one the caller handed in, so an ARN
    /// naming another partition, region or account resolves to nothing even when
    /// its id segment happens to match a local one.
    ///
    /// Unlike a mutation, a report does NOT exclude a data store that has reached
    /// the terminal [`DatastoreStatus::Deleted`] status: the record lingers for
    /// [`DELETED_RETENTION`] precisely so a reader can still observe it. Call
    /// [`Self::settle_lifecycle`] first, so this reader agrees with
    /// Describe/List/Delete about which records still exist.
    pub fn tags_for_report(&self, arn: &str) -> Result<Vec<Tag>, String> {
        match tagged_resource_from_arn(arn) {
            Some(TaggedResource::Datastore(id)) => self
                .datastores
                .get(id)
                .filter(|datastore| datastore.arn == arn)
                .map(Datastore::tag_list)
                .ok_or_else(|| DATASTORE_NOT_FOUND_MESSAGE.to_string()),
            Some(TaggedResource::Profile(id)) => self
                .profiles
                .get(id)
                .filter(|profile| profile.arn == arn)
                .map(DataTransformationProfile::tag_list)
                .ok_or_else(|| profile_not_found_message(id)),
            // Unreachable through the awsJson wire — the modelled
            // `AmazonResourceName` pattern is enforced before a handler runs —
            // but an ARN of neither resource form names no resource, so it is
            // answered the same way a missing one is.
            None => Err(DATASTORE_NOT_FOUND_MESSAGE.to_string()),
        }
    }

    /// Reap every terminal record NOW, whatever its retention: the answer to a
    /// full store, taken before a create is refused, so a caller that deletes
    /// what it created can always create again.
    pub fn reap_terminal_records(&mut self) {
        self.reap_datastores_where(|datastore| datastore.status == DatastoreStatus::Deleted);
    }

    /// Whether this service can hold ONE more data store record — what
    /// [`MAX_DATASTORES`] bounds.
    ///
    /// The ONE place a store-wide registry refusal is decided, so every
    /// operation that mints a data store refuses on the same terms: a record
    /// that has reached the terminal [`DatastoreStatus::Deleted`] status is
    /// lingering only for [`DELETED_RETENTION`] and gives its slot back by being
    /// dropped, so when the registry as it stands is full the terminal records
    /// are reaped ([`Self::reap_terminal_records`]) and the count taken again.
    /// Only a genuinely full store is refused.
    pub fn admits_another_datastore(&mut self) -> bool {
        if self.datastores.len() < MAX_DATASTORES {
            return true;
        }
        self.reap_terminal_records();
        self.datastores.len() < MAX_DATASTORES
    }

    /// Whether every data store together can RETAIN `added` more BYTES of FHIR
    /// resources once the `released` bytes the same write gives back are
    /// returned — what [`MAX_FHIR_RESOURCE_BYTES_TOTAL`] bounds.
    ///
    /// The ONE place a store-wide byte refusal is decided, for the reason
    /// [`Self::admits_another_datastore`] is the one place the count refusal is:
    /// the ceiling bounds what this service HOLDS, and a terminal record's bytes
    /// come back exactly by dropping the record
    /// ([`Self::fhir_resource_bytes_total`] is summed over the records), so when
    /// the total as it stands does not admit the write the terminal records are
    /// reaped and the total is taken again. A caller that deletes the data
    /// stores it filled can therefore always write again, and only a genuinely
    /// full service is refused.
    pub fn admits_fhir_resource_bytes(&mut self, added: usize, released: usize) -> bool {
        if self.fhir_resource_bytes_total_admits(added, released) {
            return true;
        }
        self.reap_terminal_records();
        self.fhir_resource_bytes_total_admits(added, released)
    }

    /// Whether the data store `datastore_id` can RETAIN a resource of `bytes`
    /// that releases `released` bytes of versions the same write drops, and
    /// whether that write adds a resource (`retains_one_more`) or writes a
    /// further version of one the data store already holds.
    ///
    /// The three ceilings this emulator holds its retained FHIR resources to, in
    /// ONE place so every writer charges them identically: how many resources
    /// one data store holds, how many bytes one data store retains, and how many
    /// bytes every data store retains together. Each is checked BEFORE anything
    /// is written, and with `saturating_*`, so no product of caller-chosen
    /// factors can grow the heap and an arithmetic edge cannot wrap into an
    /// admission.
    ///
    /// The store-wide total goes through [`Self::admits_fhir_resource_bytes`],
    /// which is why `self` is taken by `&mut`: that ceiling bounds what this
    /// service HOLDS, so the terminal records whose bytes come back by being
    /// dropped are reaped before a write is refused for them. The two
    /// per-data-store ceilings are about the data store being written, which is
    /// serving and so never terminal, and are unaffected.
    pub fn admits_fhir_resource(
        &mut self,
        datastore_id: &str,
        bytes: usize,
        released: usize,
        retains_one_more: bool,
    ) -> Result<(), FhirRetentionCeiling> {
        let held = self
            .datastores
            .get(datastore_id)
            .map_or(0, |datastore| datastore.fhir_resources.len());
        if retains_one_more && held >= MAX_FHIR_RESOURCES_PER_DATASTORE {
            return Err(FhirRetentionCeiling::ResourceCount);
        }
        let held_bytes = self
            .datastores
            .get(datastore_id)
            .map_or(0, |datastore| datastore.fhir_resource_bytes)
            .saturating_sub(released);
        if held_bytes.saturating_add(bytes) > MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE {
            return Err(FhirRetentionCeiling::DatastoreBytes);
        }
        if !self.admits_fhir_resource_bytes(bytes, released) {
            return Err(FhirRetentionCeiling::StoreBytes);
        }
        Ok(())
    }

    /// Whether the retained total AS IT STANDS admits `added` more bytes once
    /// `released` of the bytes it counts come back.
    fn fhir_resource_bytes_total_admits(&self, added: usize, released: usize) -> bool {
        retained_bytes_admits(
            self.fhir_resource_bytes_total(),
            added,
            released,
            MAX_FHIR_RESOURCE_BYTES_TOTAL,
        )
    }

    /// Materialise every export job transition the clock has ALREADY made due.
    ///
    /// The companion of [`Self::settle_lifecycle`] for the job records: called
    /// by the operations that REPORT a job's status as well as by any that
    /// change it, because the transition was owed at this instant whether or
    /// not anybody called. The walk is over the job records the store holds,
    /// which [`MAX_EXPORT_JOBS`] bounds — refused by the operation that MINTS
    /// them (`StartFHIRExportJob`), the only place a job count can be refused —
    /// so it costs the same whatever a caller does.
    pub fn settle_export_jobs(&mut self) {
        let (now, wall_now) = (Instant::now(), SystemTime::now());
        for job in self.export_jobs.values_mut() {
            job.settle(now, wall_now);
        }
    }

    /// Mint an unused `JobId`: 32 lowercase hex characters, which satisfies the
    /// modelled `JobId` bounds (len 1..32, and an alphabet that admits hex) and
    /// is the form every other identifier this service mints takes.
    pub fn next_export_job_id(&mut self, started_at: SystemTime) -> String {
        loop {
            self.export_jobs_started += 1;
            let id = mint_hex_id(EXPORT_JOB_MINT_DOMAIN, self.export_jobs_started, started_at);
            if !self.export_jobs.contains_key(&id) {
                return id;
            }
        }
    }

    /// The export job one `ClientToken` started, or `None` when no held job was
    /// started with that token.
    ///
    /// The token is held ON the job record, so this is a walk over the jobs
    /// [`MAX_EXPORT_JOBS`] bounds rather than a second map to keep in step with
    /// the first — and a token whose job is gone (with the data store it
    /// exported from) resolves to nothing, as it must.
    pub fn export_job_for_client_token(&self, token: &str) -> Option<&ExportJob> {
        self.export_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(token))
    }

    /// Materialise every import job transition the clock has ALREADY made due.
    ///
    /// The companion of [`Self::settle_export_jobs`] for the import job
    /// records, and called for the same reason: the transition was owed at this
    /// instant whether or not anybody called. The walk is over the job records
    /// the store holds, which [`MAX_IMPORT_JOBS`] bounds — refused by the
    /// operation that MINTS them (`StartFHIRImportJob`), the only place a job
    /// count can be refused — so it costs the same whatever a caller does.
    pub fn settle_import_jobs(&mut self) {
        let (now, wall_now) = (Instant::now(), SystemTime::now());
        for job in self.import_jobs.values_mut() {
            job.settle(now, wall_now);
        }
    }

    /// Mint an unused import `JobId`: 32 lowercase hex characters, which
    /// satisfies the modelled `JobId` bounds (len 1..32, and an alphabet that
    /// admits hex) and is the form every other identifier this service mints
    /// takes.
    pub fn next_import_job_id(&mut self, started_at: SystemTime) -> String {
        loop {
            self.import_jobs_started += 1;
            let id = mint_hex_id(IMPORT_JOB_MINT_DOMAIN, self.import_jobs_started, started_at);
            if !self.import_jobs.contains_key(&id) {
                return id;
            }
        }
    }

    /// The import job one `ClientToken` started, or `None` when no held job was
    /// started with that token.
    ///
    /// The token is held ON the job record, so this is a walk over the jobs
    /// [`MAX_IMPORT_JOBS`] bounds rather than a second map to keep in step with
    /// the first — the same shape [`Self::export_job_for_client_token`] takes.
    pub fn import_job_for_client_token(&self, token: &str) -> Option<&ImportJob> {
        self.import_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(token))
    }

    /// Materialise every transformation job transition the clock has ALREADY
    /// made due.
    ///
    /// The companion of [`Self::settle_export_jobs`] for the transformation job
    /// records, and called for the same reason: the transition was owed at this
    /// instant whether or not anybody called, so a reporting operation applying
    /// it is the record catching up with the clock. The walk is over the job
    /// records the store holds, which [`MAX_TRANSFORMATION_JOBS`] bounds —
    /// refused by the operation that MINTS them
    /// (`StartDataTransformationJob`), the only place a job count can be
    /// refused — so it costs the same whatever a caller does.
    pub fn settle_transformation_jobs(&mut self) {
        let (now, wall_now) = (Instant::now(), SystemTime::now());
        for job in self.transformation_jobs.values_mut() {
            job.settle(now, wall_now);
        }
    }

    /// Mint an unused data transformation `JobId`: 32 lowercase hex characters,
    /// which satisfies the modelled `DataTransformationJobId` bounds (len
    /// 1..32, pattern `^[a-zA-Z0-9-]+$`, which admits hex) and is the form
    /// every other identifier this service mints takes.
    pub fn next_transformation_job_id(&mut self, started_at: SystemTime) -> String {
        loop {
            self.transformation_jobs_started += 1;
            let id = mint_hex_id(
                TRANSFORMATION_JOB_MINT_DOMAIN,
                self.transformation_jobs_started,
                started_at,
            );
            if !self.transformation_jobs.contains_key(&id) {
                return id;
            }
        }
    }

    /// The data transformation job one `ClientToken` started, or `None` when no
    /// held job was started with that token.
    ///
    /// The token is held ON the job record, so this is a walk over the jobs
    /// [`MAX_TRANSFORMATION_JOBS`] bounds rather than a second map to keep in
    /// step with the first — the same shape
    /// [`Self::export_job_for_client_token`] takes.
    pub fn transformation_job_for_client_token(&self, token: &str) -> Option<&TransformationJob> {
        self.transformation_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(token))
    }

    /// Drop every export job of `datastore_id`, so a job never outlives the
    /// data store it exported from.
    fn forget_export_jobs_for(&mut self, datastore_id: &str) {
        self.export_jobs
            .retain(|_, job| job.datastore_id != datastore_id);
    }

    /// Drop every import job of `datastore_id`, so a job never outlives the
    /// data store it imported into.
    fn forget_import_jobs_for(&mut self, datastore_id: &str) {
        self.import_jobs
            .retain(|_, job| job.datastore_id != datastore_id);
    }

    /// Record `token` as having minted `datastore_id` from a request with
    /// `fingerprint`, evicting the oldest token first if that would push the map
    /// past [`MAX_CLIENT_TOKENS`].
    pub fn record_client_token(&mut self, token: String, datastore_id: &str, fingerprint: String) {
        while self.client_tokens.len() >= MAX_CLIENT_TOKENS {
            match self.client_token_order.pop_front() {
                Some(oldest) => {
                    self.client_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the
                // cap an invariant even if they ever drift.
                None => {
                    self.client_tokens.clear();
                    break;
                }
            }
        }
        if self
            .client_tokens
            .insert(
                token.clone(),
                ClientTokenRecord {
                    datastore_id: datastore_id.to_string(),
                    fingerprint,
                },
            )
            .is_none()
        {
            self.client_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that minted `datastore_id`, so a token never
    /// outlives the data store it names.
    fn forget_client_tokens_for(&mut self, datastore_id: &str) {
        self.client_tokens
            .retain(|_, record| record.datastore_id != datastore_id);
        let live = &self.client_tokens;
        self.client_token_order
            .retain(|token| live.contains_key(token));
    }

    /// Mint an unused `DatastoreId`: 32 lowercase hex characters, which is what
    /// the modelled data store ARN requires (`datastore/fhir/.{32}`) and what
    /// `DatastoreId`'s own length bound (1..32) allows.
    pub fn next_datastore_id(&mut self, created_at: SystemTime) -> String {
        loop {
            self.datastores_created += 1;
            let id = mint_hex_id(
                DATASTORE_ARN_RESOURCE_PREFIX,
                self.datastores_created,
                created_at,
            );
            if !self.datastores.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint the next FHIR logical id a create interaction assigns: a
    /// UUID-shaped, 36-character identifier, which is the form HealthLake's FHIR
    /// endpoint assigns and which satisfies the FHIR `id` type's own alphabet
    /// (`[A-Za-z0-9\-\.]{1,64}`).
    ///
    /// Server-assigned, never derived from the submitted resource: two creates
    /// of identical content get two resources, as FHIR requires. The mint
    /// counter is ADVANCED on every call and mixed in by [`mint_hex_id`] beside
    /// the clock — under this resource kind's own domain, so a FHIR resource and
    /// a control-plane resource cannot collide on one hash — so two creates in
    /// the same tick, and two successive calls with the same instant, still get
    /// different ids. The CALLER checks the minted id against the data store it
    /// is writing to, so a collision cannot overwrite a stored resource.
    pub fn next_fhir_resource_id(&mut self, created_at: SystemTime) -> String {
        self.fhir_resources_created += 1;
        uuid_form(&mint_hex_id(
            FHIR_RESOURCE_MINT_DOMAIN,
            self.fhir_resources_created,
            created_at,
        ))
    }

    /// Record `token` as having minted `profile_id`, evicting the oldest token
    /// first if that would push the map past [`MAX_PROFILE_CLIENT_TOKENS`].
    pub fn record_profile_client_token(&mut self, token: String, profile_id: &str) {
        while self.profile_client_tokens.len() >= MAX_PROFILE_CLIENT_TOKENS {
            match self.profile_client_token_order.pop_front() {
                Some(oldest) => {
                    self.profile_client_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the
                // cap an invariant even if they ever drift.
                None => {
                    self.profile_client_tokens.clear();
                    break;
                }
            }
        }
        if self
            .profile_client_tokens
            .insert(
                token.clone(),
                ProfileClientTokenRecord {
                    profile_id: profile_id.to_string(),
                },
            )
            .is_none()
        {
            self.profile_client_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that minted `profile_id`, so a token never
    /// outlives the profile it names — the inverse of
    /// [`Self::record_profile_client_token`], and the profile-side counterpart of
    /// [`Self::forget_client_tokens_for`]. The order queue is pruned with the
    /// map, so a dropped token stops consuming one of the
    /// [`MAX_PROFILE_CLIENT_TOKENS`] slots as well.
    fn forget_profile_client_tokens_for(&mut self, profile_id: &str) {
        self.profile_client_tokens
            .retain(|_, record| record.profile_id != profile_id);
        let live = &self.profile_client_tokens;
        self.profile_client_token_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove the profile `profile_id` and everything held for it, handing back
    /// the record that went (or `None` when no record holds that id).
    ///
    /// The ONE way a profile leaves the store, so a removal cannot drop the
    /// record and forget its idempotency tokens — the shape
    /// [`Self::drop_datastore`] takes for a data store, which both reapers
    /// route their removals through.
    pub fn remove_profile(&mut self, profile_id: &str) -> Option<DataTransformationProfile> {
        let profile = self.profiles.remove(profile_id)?;
        self.forget_profile_client_tokens_for(profile_id);
        Some(profile)
    }

    /// Mint an unused `ProfileId`: 32 lowercase hex characters, which is
    /// exactly what the modelled `ProfileIdString` pattern (`^[a-f0-9]{32}$`)
    /// and the profile ARN template both require.
    pub fn next_profile_id(&mut self, created_at: SystemTime) -> String {
        loop {
            self.profiles_created += 1;
            let id = mint_hex_id(PROFILE_ARN_RESOURCE_PREFIX, self.profiles_created, created_at);
            if !self.profiles.contains_key(&id) {
                return id;
            }
        }
    }
}

/// The mint domain a FHIR export job's `JobId` is derived under, so it cannot
/// collide with a control-plane resource's id for the same counter and instant.
/// Like a FHIR resource, and unlike the data store and the profile, an export
/// job has no ARN, so the domain is named here rather than taken from an ARN
/// prefix.
const EXPORT_JOB_MINT_DOMAIN: &str = "export-job/";

/// The mint domain a FHIR import job's `JobId` is derived under, so it cannot
/// collide with an export job's — or any other resource's — id for the same
/// counter and instant. Like an export job, an import job has no ARN, so the
/// domain is named here rather than taken from an ARN prefix.
const IMPORT_JOB_MINT_DOMAIN: &str = "import-job/";

/// The mint domain a data transformation job's `JobId` is derived under, so it
/// cannot collide with another resource's id for the same counter and instant.
/// Like an export job, a transformation job has no ARN, so the domain is named
/// here rather than taken from an ARN prefix.
const TRANSFORMATION_JOB_MINT_DOMAIN: &str = "data-transformation-job/";

/// The mint domain a FHIR resource's logical id is derived under, so it cannot
/// collide with a control-plane resource's id for the same counter and instant.
/// Unlike the two ARN prefixes [`mint_hex_id`] is otherwise called with, a FHIR
/// resource has no ARN — it is addressed by its data store's endpoint — so the
/// domain is named here.
const FHIR_RESOURCE_MINT_DOMAIN: &str = "fhir-resource/";

/// One 32-lowercase-hex identifier, the form this service's resource ids and ARNs
/// take, minted from the resource's own `domain` prefix, the mint counter and the
/// creation instant — so an identifier leaks neither verbatim, two creates in the
/// same clock tick still differ, and the two resource kinds cannot collide on one
/// hash. Shared by every mint here; the caller re-mints while the id is taken.
fn mint_hex_id(domain: &str, counter: u64, created_at: SystemTime) -> String {
    use sha2::Digest;

    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = sha2::Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hex::encode(&hasher.finalize()[..16])
}

/// One [`mint_hex_id`] output written in the 8-4-4-4-12 grouping a UUID is written
/// in: 36 characters.
///
/// The ONE place that grouping is applied, so every identifier this service mints in
/// the UUID form — a FHIR resource's logical id and an agent `ConversationId` — is
/// grouped the same way. `mint_hex_id` returns exactly 32 lowercase hex characters,
/// so every slice below is in range for every input.
fn uuid_form(mixed: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &mixed[..8],
        &mixed[8..12],
        &mixed[12..16],
        &mixed[16..20],
        &mixed[20..32]
    )
}

/// What a request path says about a data store's FHIR endpoint: the data store
/// it addresses, and the FHIR interaction path on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FhirEndpointPath {
    /// The `DatastoreId` the path names. Whether a data store with that id
    /// exists — and whether it is serving — is the handler's question, not this
    /// reader's.
    pub datastore_id: String,
    /// The interaction path relative to the endpoint: the non-empty segments
    /// after the release-version segment (`["Patient"]` for a type-level
    /// interaction, `["Patient", "<id>"]` for an instance-level one).
    pub interaction: Vec<String>,
}

/// Read the data store FHIR endpoint path a request addresses, or `None` when
/// the request is not on such a path.
///
/// The path space is the one [`datastore_endpoint`] publishes —
/// `/datastore/{DatastoreId}/r4/` — so the two are written from the same
/// constants and cannot drift. Empty segments are skipped, so a client that
/// joins an interaction onto an endpoint already ending in `/` (yielding `//`)
/// addresses the same resource as one that does not. Segments are read RAW:
/// every value this path space carries — a `DatastoreId`, a FHIR resource type,
/// a FHIR logical id — is drawn from an alphabet no client percent-encodes.
pub fn parse_datastore_fhir_path(path: &str) -> Option<FhirEndpointPath> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    if segments.next()? != DATASTORE_PATH_SEGMENT {
        return None;
    }
    let datastore_id = segments.next()?;
    if !segments
        .next()?
        .eq_ignore_ascii_case(FHIR_ENDPOINT_VERSION_SEGMENT)
    {
        return None;
    }
    Some(FhirEndpointPath {
        datastore_id: datastore_id.to_string(),
        interaction: segments.map(str::to_string).collect(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsHealthlakeHealthlakeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsHealthlakeHealthlakeState>)
}

/// The region to stamp into a new resource's ARN and endpoint: the region of the
/// request's SigV4 credential scope, else [`DEFAULT_REGION`].
///
/// Lowercased and filtered to the characters the modelled `AmazonResourceName`
/// pattern allows in that segment (`[a-z0-9-]+`), so a scope carrying anything
/// else falls back rather than producing an ARN the model would refuse.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region.to_ascii_lowercase())
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the data store `id` in `region`, in the form the modelled
/// `DatastoreArn` and `AmazonResourceName` patterns both require. The account
/// segment comes from the emulator's canonical account id.
pub fn datastore_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "healthlake",
        region,
        &format!("{DATASTORE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The FHIR endpoint of the data store `id`, as a client must address it: THIS
/// emulator's own base URL ([`smithy_server_runtime::OperationContext::endpoint`])
/// followed by the data store's own path.
///
/// The path space is written from the same two constants
/// [`parse_datastore_fhir_path`] reads, so the endpoint this service advertises is
/// exactly the path space its FHIR REST data plane serves — and the host is the one
/// serving it, because a caller SENDS FHIR interactions to this value: a URL built
/// from the real service's host would take every one of them off the emulator, where
/// nothing answers. The Region is therefore not in the host (one emulator serves
/// every Region); it stays on the data store's ARN, which is what names the Region a
/// resource belongs to.
pub fn datastore_endpoint(endpoint_base: &str, id: &str) -> String {
    format!(
        "{}/{DATASTORE_PATH_SEGMENT}/{id}/{FHIR_ENDPOINT_VERSION_SEGMENT}/",
        endpoint_base.trim_end_matches('/')
    )
}

/// The ARN of the data transformation profile `id` in `region`, in the form the
/// modelled `AmazonResourceName` pattern and the `DataTransformationProfile`
/// resource's `aws.api#arn` template both require.
pub fn profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "healthlake",
        region,
        &format!("{PROFILE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// A stable fingerprint of the fields that define one request, for `ClientToken`
/// idempotency: a token resolves to the resource it minted only for a repeat of
/// the SAME request, and a token reused with a different request is refused.
///
/// Fields are hashed LENGTH-PREFIXED, so two different splits of the same bytes
/// cannot fingerprint alike. Every field a caller passes must have a CANONICAL
/// form: order a map (a `BTreeMap`, never the `HashMap` a modelled map
/// deserializes into — its iteration order differs per instance) and render
/// presence explicitly, or two identical requests fingerprint differently and
/// idempotency breaks.
pub fn request_fingerprint<I: IntoIterator<Item = String>>(fields: I) -> String {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for field in fields {
        hasher.update(field.len().to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// One optional member as a fingerprint field, so an absent member and an empty
/// one are different requests rather than being collapsed by a default.
pub fn optional_field(member: &str, value: Option<&str>) -> String {
    match value {
        Some(value) => format!("{member}=present:{value}"),
        None => format!("{member}=absent"),
    }
}

/// Whether `id` has the shape of a `ProfileId`, i.e. satisfies the modelled
/// `ProfileIdString` pattern `^[a-f0-9]{32}$`.
///
/// The members that carry a `ProfileId` INSIDE a source union target a plain
/// `String`, so the boundary validator does not hold them to that pattern; a
/// handler reading one must.
pub fn is_profile_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The `DatastoreId` named by a HealthLake data store ARN, or `None` when the
/// ARN is not a HealthLake data store ARN (a malformed one, another service's,
/// or a `dataTransformationProfile` ARN, which names a different resource).
///
/// This reads the ARN's SHAPE only. It says nothing about whether a data store
/// with that id exists, nor about the ARN's partition, region or account — the
/// caller must still require the stored record's own ARN to equal the one it
/// was handed, so an ARN naming another region or account resolves to nothing.
pub fn datastore_id_from_arn(arn: &str) -> Option<&str> {
    healthlake_arn_resource(arn)?.strip_prefix(DATASTORE_ARN_RESOURCE_PREFIX)
}

/// The `ProfileId` named by a HealthLake data transformation profile ARN, or
/// `None` when the ARN is not one (a malformed one, another service's, or a
/// `datastore/fhir` ARN, which names a different resource).
///
/// Like [`datastore_id_from_arn`] this reads the ARN's SHAPE only: the id has
/// to satisfy the modelled `ProfileIdString` alphabet the `AmazonResourceName`
/// pattern spells for this form (`dataTransformationProfile\/[a-f0-9]{32}`),
/// but nothing here says a profile with that id exists, and the caller must
/// still require the stored record's own ARN to equal the one it was handed so
/// an ARN naming another partition, region or account resolves to nothing.
pub fn profile_id_from_arn(arn: &str) -> Option<&str> {
    let id = healthlake_arn_resource(arn)?.strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?;
    is_profile_id(id).then_some(id)
}

/// Which HealthLake resource an `AmazonResourceName` names, as the modelled
/// pattern's two resource forms spell them.
///
/// The tagging operations take that one shape for EITHER resource, so they
/// resolve it through [`tagged_resource_from_arn`] rather than each knowing one
/// form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// A FHIR data store, by `DatastoreId`.
    Datastore(&'a str),
    /// A data transformation profile, by `ProfileId`.
    Profile(&'a str),
}

/// What a tagging MUTATION writes through: the tag map of the resource an
/// `AmazonResourceName` named, the cap that bounds the resulting set, and the
/// noun a refusal names that kind of resource by.
///
/// Handed out by [`ComAmazonawsHealthlakeHealthlakeState::tags_for_mutation`],
/// which is where a resource that takes no mutation is refused instead.
#[derive(Debug)]
pub struct MutableTags<'a> {
    pub tags: &'a mut BTreeMap<String, String>,
    pub max_tags: usize,
    pub noun: &'static str,
}

impl MutableTags<'_> {
    /// Merge `requested`, by key, onto the resource's stored tags, or refuse
    /// with the sentence that states the cap when the result would carry more
    /// than [`Self::max_tags`] of them.
    ///
    /// Merging is all-or-nothing: the resulting tag set is bounded, so a stream
    /// of calls with fresh keys cannot grow a record without limit. The modelled
    /// tag collection's own bound caps ONE request; this caps the RESOURCE, and
    /// [`Self::noun`] names the kind of resource it caps in the refusal.
    ///
    /// The ONE place a tag merge is written, so every wire that merges tags onto
    /// a resource of this service — the awsJson `TagResource` and the
    /// data-transformation endpoint's REST `POST /tags/{ResourceArn}` — merges
    /// and refuses on identical terms.
    pub fn merge<I: IntoIterator<Item = (String, String)>>(
        &mut self,
        requested: I,
    ) -> Result<(), String> {
        let requested: Vec<(String, String)> = requested.into_iter().collect();
        let merged = self
            .tags
            .keys()
            .map(String::as_str)
            .chain(requested.iter().map(|(key, _value)| key.as_str()))
            .collect::<BTreeSet<&str>>()
            .len();
        if merged > self.max_tags {
            return Err(format!(
                "A {} carries at most {} tags; this request would leave {merged}",
                self.noun, self.max_tags
            ));
        }

        for (key, value) in requested {
            self.tags.insert(key, value);
        }
        Ok(())
    }

    /// Remove the tags `keys` names from the resource's stored tags.
    ///
    /// Removal is BY KEY and idempotent: only the keys named go, a key the
    /// resource does not carry leaves the stored set as it was, and nothing else
    /// about the record changes — so repeating a request answers the same way
    /// and reports the same set. Shared by every wire that untags a resource of
    /// this service, for the reason [`Self::merge`] is.
    pub fn remove_keys<K: AsRef<str>, I: IntoIterator<Item = K>>(&mut self, keys: I) {
        for key in keys {
            self.tags.remove(key.as_ref());
        }
    }
}

/// The resource a HealthLake `AmazonResourceName` names, or `None` when the ARN
/// is of neither modelled resource form.
///
/// The modelled pattern admits exactly two
/// (`datastore\/fhir\/.{32}` and `dataTransformationProfile\/[a-f0-9]{32}`),
/// and both name a resource this service holds and tags, so every tagging
/// operation resolves its `ResourceARN` here. SHAPE only, as
/// [`datastore_id_from_arn`] and [`profile_id_from_arn`] are.
pub fn tagged_resource_from_arn(arn: &str) -> Option<TaggedResource<'_>> {
    if let Some(id) = datastore_id_from_arn(arn) {
        return Some(TaggedResource::Datastore(id));
    }
    profile_id_from_arn(arn).map(TaggedResource::Profile)
}

/// The RESOURCE field of a HealthLake ARN (everything after the account
/// segment), or `None` when `arn` is not an ARN of this service in an `aws`
/// partition.
///
/// The one place the five leading ARN fields are parsed, so every resource form
/// this service resolves agrees about what an ARN of its own looks like.
fn healthlake_arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("healthlake"),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if !partition.starts_with("aws") {
        return None;
    }
    Some(resource)
}

/// Epoch nanoseconds for a modelled timestamp, so two `HealthLakeTimestamp`
/// values compare on one scale whatever precision each carries.
pub fn epoch_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// Whether `at` satisfies a listing's pair of timestamp cutoff members read as
/// STRICT bounds — the reading a member documented as selecting records
/// "before" / "after" the date it names carries, under which a record at
/// exactly the named instant is neither before nor after it and so is reported
/// by neither cutoff.
///
/// `before` is the later-bounding member and `after` the earlier-bounding one;
/// a member the request left unset bounds nothing.
pub fn within_strict_cutoffs(
    at: &::aws_smithy_types::DateTime,
    before: Option<&::aws_smithy_types::DateTime>,
    after: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let at = epoch_nanos(at);
    if let Some(before) = before {
        if at >= epoch_nanos(before) {
            return false;
        }
    }
    if let Some(after) = after {
        if at <= epoch_nanos(after) {
            return false;
        }
    }
    true
}

/// Whether `at` satisfies a listing's pair of timestamp cutoff members read as
/// INCLUSIVE bounds — the reading a member documented as selecting records "at
/// or before" / "at or after" the instant it names carries, under which a
/// record at exactly that instant is reported by both cutoffs.
///
/// `before` is the later-bounding member and `after` the earlier-bounding one;
/// a member the request left unset bounds nothing.
pub fn within_inclusive_cutoffs(
    at: &::aws_smithy_types::DateTime,
    before: Option<&::aws_smithy_types::DateTime>,
    after: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let at = epoch_nanos(at);
    if let Some(before) = before {
        if at > epoch_nanos(before) {
            return false;
        }
    }
    if let Some(after) = after {
        if at < epoch_nanos(after) {
            return false;
        }
    }
    true
}
