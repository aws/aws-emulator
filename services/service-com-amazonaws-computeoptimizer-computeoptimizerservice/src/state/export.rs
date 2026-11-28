// SPDX-License-Identifier: CC0-1.0

//! The FILES a recommendations export job writes, and the write itself.
//!
//! Every `Export*Recommendations` operation of this service queues one asynchronous export:
//! "Recommendations are exported in a comma-separated values (.csv) file, and its metadata
//! in a JavaScript Object Notation (JSON) (.json) file, to an existing Amazon Simple Storage
//! Service (Amazon S3) bucket that you specify." The job record and its lifecycle live in
//! [`super`]; this module is the other half — WHAT the two objects hold, and the write that
//! puts them in the destination bucket once the job completes.
//!
//! The one object store in this emulator is the Amazon S3 service's own, so the export
//! objects are written there and are then read back with `GetObject` / `ListObjectsV2` like
//! any other object. That is also why a destination this emulator does not hold is REFUSED
//! at the export call ([`destination_bucket_rejection`]): "You must create the destination
//! Amazon S3 bucket for your recommendations export before you create the export job.
//! Compute Optimizer does not create the S3 bucket for you."
//!
//! What the files hold:
//!
//! * the export file — one HEADER row of the `Exportable*Field` names the request asked for
//!   ("The recommendations data to include in the export file"), then one row per
//!   recommendation the account's analysis holds for the job's resource type, and one per
//!   recommendation OPTION where the fields name an option's values (each
//!   `RecommendationOptions*` field names ONE option). Each cell is that field's value for
//!   that recommendation, computed from the stored analysis the same way the recommendation
//!   READER computes it, so an exported row and a `Get*Recommendations` response cannot
//!   disagree. A field the stored analysis records no value for is an empty cell, and so is one
//!   that names no member of the surface's recommendation at all. Where a surface's
//!   recommendation carries members the shared findings-catalog row does not — the Amazon EC2
//!   instance surface, whose options carry an instance type, the Aurora and RDS database
//!   surface, whose catalog row records the database's own configuration, the Amazon EBS volume
//!   surface, whose recommendation carries the volume's own configuration, the utilization
//!   observed on it and the configuration its option would move it to, the AWS Lambda function
//!   surface, whose recommendation carries the version analysed, the function's own
//!   configuration, the utilization observed on it and the memory size its option would move
//!   it to, and the Amazon ECS
//!   service surface, whose recommendation is computed from the Amazon ECS inventory and the
//!   utilization published for it rather than recorded in that catalog at all — the row is built
//!   from the observation the reader reports the recommendation from rather than from that
//!   catalog row, so the one rule serves both.
//! * the metadata file — what the export job itself was: its id, the resource type and
//!   account(s) it covers, the recommendation preferences it was asked for, the file format and
//!   columns of the export file, how many rows it holds and where both objects were written.
//!
//! AWS publishes no column layout, row order or metadata schema for either file, so both are
//! the emulator's own; every value in them is nevertheless derived from the stored analysis
//! and from the request that asked for the export.
//!
//! ## The clock
//!
//! The objects appear when the job REACHES `Complete` ([`ExportJob::status_at`]), not when
//! the export call returns: a client that reads the destination while the job is still
//! `Queued` or `InProgress` finds nothing there, as it would against the real service. The
//! files are computed at the export call (so the export is of the analysis as it stood when
//! the job was created) and held with the job until the clock makes the completion due;
//! [`write_due_exports`] then performs exactly the writes the stored due instant has already
//! made due, and no operation ever sleeps for it. A client that polls
//! `DescribeRecommendationExportJobs` to `Complete` and then downloads the file is the
//! documented workflow, and [`drive_export_clock`] starts one short in-process task per job
//! so the objects also appear for a client that never calls this API again.
//!
//! ## Lock order
//!
//! This service's store is locked, the due objects are TAKEN out of it, and the lock is
//! released before the object store is locked: the two locks are never held at once, and the
//! Amazon S3 service never reaches into this one, so the two cannot deadlock.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use ::service_com_amazonaws_s3_amazons3 as s3;

use super::{
    external_metric_status, look_back_period_in_days, parse_resource_arn,
    percentage_of, round_hundredths, stated_architectures, AnalysedResource, AnalyzedAutoScalingGroup,
    ComAmazonawsComputeoptimizerComputeoptimizerserviceState, EffectivePreferences, ExportJob,
    GroupConfiguration, IdleResourceAnalysis, InstanceConfiguration, ObservedFunction,
    ObservedInstance, ObservedLicensedInstance, ObservedVolume, CURRENCY_USD, IDLE_FALSE,
    IDLE_TRUE, LAMBDA_FINDING_UNAVAILABLE, RESOURCE_TYPE_AUTO_SCALING_GROUP,
    RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_ECS_SERVICE, RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION,
    RESOURCE_TYPE_LICENSE, RESOURCE_TYPE_RDS_DB_INSTANCE, SAVINGS_ESTIMATION_SOURCE,
};
use crate::ops::types::{
    DBStorageConfiguration, EBSUtilizationMetric, ECSServiceRecommendation,
    LambdaFunctionRecommendation, MemorySizeConfiguration, RDSDBRecommendation,
    RDSDBUtilizationMetric, SavingsOpportunity, Tag, VolumeConfiguration, VolumeRecommendation,
    EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES, EXPORTABLE_IDLE_FIELD_VALUES,
    EXPORTABLE_INSTANCE_FIELD_VALUES, EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES,
    EXPORTABLE_R_D_S_D_B_FIELD_VALUES, EXPORTABLE_VOLUME_FIELD_VALUES,
    EXPORTABLE_E_C_S_SERVICE_FIELD_VALUES,
};

/// The one `FileFormat` the model declares: "The only export file format currently supported
/// is Csv" (`crate::ops::types::FILE_FORMAT_VALUES`).
pub const FILE_FORMAT_CSV: &str = "Csv";

/// How a cell holding several values separates them, so one exportable field is still one
/// column. AWS publishes no layout for the export file, so this is the emulator's own.
const LIST_SEPARATOR: &str = "|";

/// One row of an export file: the value of each exportable field the recommendation carries,
/// keyed by the field's modelled name (an `Exportable*Field` value). A field the analysis
/// records no value for has NO entry, and the cell under its column is empty.
pub type ExportRow = BTreeMap<String, String>;

/// The column of an export row that names the account whose recommendation it is: the
/// `AccountId` exportable field every resource type's `Exportable*Field` value set declares
/// (`crate::ops::types::EXPORTABLE_INSTANCE_FIELD_VALUES` and its siblings). It is the
/// account a row is SELECTED by when a request names the accounts to export, whether or not
/// the request asked for the column itself.
pub const ACCOUNT_ID_COLUMN: &str = "AccountId";

/// One filter of any of this service's filter vocabularies.
///
/// Each `Export*Recommendations` operation carries its own filter shape and every one of them
/// is a name with the values it selects, so this is what lets one [`ExportPlan`] take the
/// filters of any of those surfaces ([`filter_pairs`]) instead of each handler restating the
/// same mapping.
pub trait ExportFilter {
    /// The `*FilterName` the filter carries, or `None` for one that names nothing.
    fn filter_name(&self) -> Option<&str>;
    /// The values it selects on; empty constrains nothing.
    fn filter_values(&self) -> &[String];
}

/// Implement [`ExportFilter`] for the modelled filter shapes, which all declare the same two
/// members.
macro_rules! export_filters {
    ($($shape:ty),+ $(,)?) => {
        $(impl ExportFilter for $shape {
            fn filter_name(&self) -> Option<&str> {
                self.name.as_deref()
            }

            fn filter_values(&self) -> &[String] {
                self.values.as_deref().unwrap_or_default()
            }
        })+
    };
}

export_filters!(
    crate::ops::types::Filter,
    crate::ops::types::EBSFilter,
    crate::ops::types::ECSServiceRecommendationFilter,
    crate::ops::types::IdleRecommendationFilter,
    crate::ops::types::LambdaFunctionRecommendationFilter,
    crate::ops::types::LicenseRecommendationFilter,
    crate::ops::types::RDSDBRecommendationFilter,
);

/// A request's filters as the (name, values) pairs an [`ExportPlan`] carries.
pub fn filter_pairs<F: ExportFilter>(filters: &[F]) -> Vec<(String, Vec<String>)> {
    filters
        .iter()
        .map(|filter| {
            (
                filter.filter_name().unwrap_or_default().to_string(),
                filter.filter_values().to_vec(),
            )
        })
        .collect()
}

/// Everything one export call settles about the job it creates: where the two objects go, and
/// what they hold. The job record and both files are derived from this, so the destination the
/// call answers, the destination the job reports and the objects that actually appear cannot
/// name different places.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    /// The `JobId` minted for the job.
    pub job_id: String,
    /// The Region whose endpoint created it.
    pub region: String,
    /// The `ResourceType` whose recommendations are exported.
    pub resource_type: String,
    /// The destination bucket the request named, and the two object keys the export file and
    /// its metadata file are written under ([`super::export_destination_keys`]).
    pub bucket: String,
    /// See [`ExportPlan::bucket`].
    pub key: String,
    /// See [`ExportPlan::bucket`].
    pub metadata_key: String,
    /// The columns of the export file ([`export_columns`]).
    pub columns: Vec<String>,
    /// The `FileFormat` the export file is written in.
    pub file_format: String,
    /// The `accountIds` the request named, and whether it asked for the organization's member
    /// accounts — what the export covers, which the metadata file states.
    pub account_ids: Vec<String>,
    /// See [`ExportPlan::account_ids`].
    pub include_member_accounts: bool,
    /// The request's filters as (name, values) pairs — "an array of objects to specify a
    /// filter that exports a more specific set of ... recommendations" — which select which
    /// recommendations the export file holds ([`row_matches_filters`]).
    pub filters: Vec<(String, Vec<String>)>,
    /// The `cpuVendorArchitectures` of the `recommendationPreferences` the request named: the
    /// architectures the exported recommendations are generated under, which select which
    /// recommendation options the export file holds and which architectures its rows state.
    /// Empty for a request that asked for none — and for a surface whose model declares no
    /// such member — which restricts nothing.
    pub cpu_vendor_architectures: Vec<String>,
    /// The Amazon Aurora and RDS databases the account held an analysis for when the export
    /// was created ([`super::analysed_databases`]): the DB instances of its own Amazon RDS
    /// inventory, beside the database rows of the findings catalog. They are resolved with the
    /// plan — "the export is of the recommendations the account held when it asked for the
    /// export" — so an exported file holds exactly the databases
    /// `GetRDSDatabaseRecommendations` reported at that moment. Empty for every other resource
    /// type's export, which no database row belongs to.
    pub analysed_databases: Vec<AnalysedResource>,
    /// The Amazon ECS service recommendations the account held when the export was created
    /// ([`super::analysed_ecs_services`]): the ACTIVE services of its own Amazon ECS inventory
    /// the analysis has inputs for. They are resolved with the plan — "the export is of the
    /// recommendations the account held when it asked for the export" — so an exported file
    /// holds exactly the services `GetECSServiceRecommendations` reported at that moment, each
    /// row carrying the members that reader reports for it. Empty for every other resource
    /// type's export, which no Amazon ECS service row belongs to.
    pub analysed_ecs_services: Vec<ECSServiceRecommendation>,
    /// The Amazon EBS volumes the account held an analysis for when the export was created
    /// ([`super::analysed_volumes`]): the volumes of its own Amazon EC2 inventory, each with the
    /// configuration and the utilization samples a recommendation for it is computed from. They
    /// are resolved with the plan, for the same reason the databases are, so an exported file
    /// holds exactly the volumes `GetEBSVolumeRecommendations` reported at that moment. The
    /// OBSERVATION is held rather than the findings-catalog projection of it, because a volume's
    /// recommendation carries members that catalog row does not
    /// ([`ObservedVolume::to_recommendation`]). Empty for every other resource type's export,
    /// which no volume row belongs to.
    pub analysed_volumes: Vec<ObservedVolume>,
    /// The Lambda function versions the account held an analysis for when the export was
    /// created ([`super::analysed_functions`]): the versions of its own AWS Lambda inventory,
    /// each with the memory it is configured with and the observations a recommendation for it
    /// is computed from. They are resolved with the plan, for the same reason the databases
    /// are, so an exported file holds exactly the functions
    /// `GetLambdaFunctionRecommendations` reported at that moment. The OBSERVATION is held
    /// rather than the findings-catalog projection of it, because a function's recommendation
    /// carries members that catalog row does not ([`ObservedFunction::to_recommendation`]).
    /// Empty for every other resource type's export, which no function row belongs to.
    pub analysed_functions: Vec<ObservedFunction>,
}

/// The two objects an export job has computed but not yet written: the export file and its
/// metadata file, held with the job until the clock makes the job's completion due.
#[derive(Debug, Clone)]
pub struct PendingExport {
    /// The export file's bytes.
    pub export_file: String,
    /// The metadata file's bytes.
    pub metadata_file: String,
}

/// One job's objects, taken out of the store because the clock has made the job's completion
/// due, with where they go — what [`write_due_exports`] then writes.
#[derive(Debug, Clone)]
pub struct DueExport {
    /// The job the objects belong to, which is what a failed write is recorded against.
    pub job_id: String,
    /// The destination bucket, and the keys the two objects are written under.
    pub bucket: String,
    /// See [`DueExport::bucket`].
    pub key: String,
    /// See [`DueExport::bucket`].
    pub metadata_key: String,
    /// The objects' bytes.
    pub pending: PendingExport,
}

/// Why a request's destination is not one this export can be written to, or `None` when it
/// is.
///
/// "You must create the destination Amazon S3 bucket for your recommendations export before
/// you create the export job. Compute Optimizer does not create the S3 bucket for you" — so a
/// request naming a bucket the object store does not hold is refused rather than accepted
/// into a job whose file could never appear.
///
/// The refusal is the service's own constant wording for the condition
/// ([`MISSING_DESTINATION_MESSAGE`]): it states that the destination does not exist and what to
/// do about it, and names neither the bucket nor the member — so no export operation words
/// this refusal differently.
pub fn destination_bucket_rejection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
) -> Option<String> {
    let held = s3::state::service_state(engine)
        .is_some_and(|store| store.lock().bucket(bucket).is_some());
    (!held).then(|| MISSING_DESTINATION_MESSAGE.to_string())
}

/// What every export operation answers when the destination it was given is not a bucket this
/// emulator's object store holds. ONE spelling for the whole crate, so the eight exports
/// cannot drift apart on the same refusal.
pub const MISSING_DESTINATION_MESSAGE: &str =
    "The Amazon S3 destination provided does not exist. Confirm your S3 destination and try \
     again.";

/// What every export operation answers a request made while one of its own resource type is
/// still in flight: the service's own constant sentence for the condition. It states the limit
/// and nothing else — neither the resource type, nor the Region, nor the job that still holds
/// the slot — so the refusal is the same text for every export surface and every run, and ONE
/// spelling serves the whole crate.
pub const EXPORT_JOB_LIMIT_MESSAGE: &str = "Export job limit exceeded. You can only have one \
     active job per resource type at a time.";

/// The columns of an export file: the fields the request asked for, in the order it asked for
/// them, or — for a request that names none — every field the resource type's
/// `Exportable*Field` enum declares.
///
/// "The recommendations data to include in the export file": the request chooses which of its
/// resource type's fields the file holds, and a request that chooses none gets the whole set
/// rather than a file of no columns.
///
/// Only the fields that resource type's enum DECLARES, each once: the column count is then
/// bounded by the size of that enum, so neither the file this builds nor the work of building
/// it can be grown by a request that repeats a name or carries a long list of them. (The
/// request boundary already refuses a value the enum does not declare, so a name dropped here
/// is one that could not have arrived.)
pub fn export_columns(requested: Option<&[String]>, modelled: &[&str]) -> Vec<String> {
    let requested = requested.unwrap_or_default();
    if requested.is_empty() {
        return modelled.iter().map(|field| (*field).to_string()).collect();
    }
    let declared: HashSet<&str> = modelled.iter().copied().collect();
    let mut taken: HashSet<&str> = HashSet::with_capacity(modelled.len());
    let mut columns: Vec<String> = Vec::new();
    for field in requested {
        if let Some(field) = declared.get(field.as_str()) {
            if taken.insert(field) {
                columns.push((*field).to_string());
            }
        }
    }
    columns
}

// ---------------------------------------------------------------------------
// Admitting one export call and creating its job
//
// Every `Export*Recommendations` operation of this service admits a request by the SAME
// rules — the mutually exclusive `accountIds`/`includeMemberAccounts` pair, the account ids
// themselves, the organization an `includeMemberAccounts` asks to cover, the surface's own
// filter vocabulary, a destination bucket that is named and that the object store holds, the
// enrollment gate, the one-job-in-progress-per-Region limit and how many jobs this store may
// hold — and then records the same job against the same derived keys. So that whole rule lives HERE once, and each export operation supplies only
// what its own surface settles: the resource type it exports, its filter vocabulary, its
// `Exportable*Field` set and the sentence it documents the one-job limit with.
// ---------------------------------------------------------------------------

/// What one `Export*Recommendations` request asks for, in the terms the shared admission rule
/// needs: the members every export surface declares, plus the few things that differ between
/// surfaces.
pub struct ExportRequest<'a, F: ExportFilter> {
    /// The `ResourceType` whose recommendations this surface exports, which the job carries
    /// and which the one-job-in-progress limit is counted per.
    pub resource_type: &'a str,
    /// The request's `accountIds` and whether it asked for the organization's member accounts
    /// — "The parameters are mutually exclusive." They select WHOSE recommendations go into
    /// the file, not where it is written, so an id naming an account this service cannot
    /// answer about
    /// ([`super::ComAmazonawsComputeoptimizerComputeoptimizerserviceState::answers_for_account`])
    /// is refused rather than exported under, and the file holds the rows of the accounts the
    /// request named and of no others.
    pub account_ids: Option<Vec<String>>,
    /// See [`ExportRequest::account_ids`]. Asking for the member accounts is asking for the
    /// resources of "all member accounts of the organization", which only a management account
    /// or delegated administrator has, so a caller that manages none is refused
    /// ([`super::ComAmazonawsComputeoptimizerComputeoptimizerserviceState::include_member_accounts_rejection`]).
    pub include_member_accounts: bool,
    /// The request's filters, in this surface's own filter shape: "an array of objects to
    /// specify a filter that exports a more specific set of ... recommendations".
    pub filters: Option<Vec<F>>,
    /// This surface's filter vocabulary check — the one its recommendation READER applies, so
    /// an export and a read never admit different filter sets.
    pub filter_rejection: fn(&[F]) -> Option<String>,
    /// The `s3DestinationConfig` the request named: the destination bucket and the key prefix
    /// the two object keys are derived from.
    pub destination: &'a crate::ops::types::S3DestinationConfig,
    /// "The recommendations data to include in the export file", with this surface's
    /// `Exportable*Field` value set to resolve it against ([`export_columns`]).
    pub fields_to_export: Option<&'a [String]>,
    /// See [`ExportRequest::fields_to_export`].
    pub exportable_fields: &'a [&'a str],
    /// The `fileFormat` the request asked for, or `None` for the one format the model
    /// declares ([`FILE_FORMAT_CSV`]).
    pub file_format: Option<String>,
    /// The `recommendationPreferences` the request carries, for the export surfaces whose
    /// model declares that member, and `None` for the surfaces that declare none. It
    /// "describes the recommendation preferences to return in the response of a
    /// GetAutoScalingGroupRecommendations, GetEC2InstanceRecommendations … and
    /// GetRDSDatabaseRecommendations request", so an EXPORT of those same recommendations is
    /// of the recommendations the preferences settle — resolved here by the rule the surface's
    /// own READER resolves it by, so the file and the response cannot disagree.
    pub recommendation_preferences: Option<crate::ops::types::RecommendationPreferences>,
}

/// Why one export call cannot create its job, as the modelled error the calling operation
/// answers it with: the shared rule names the refusal and each operation lifts it into its own
/// error channel.
#[derive(Debug, Clone)]
pub enum ExportRefusal {
    /// This service's store is not reachable from the engine — an `InternalServerException`.
    StoreUnavailable,
    /// The request is not one the service accepts — an `InvalidParameterValueException`.
    InvalidParameter(String),
    /// The account is not enrolled — an `OptInRequiredException`.
    OptInRequired,
    /// The call would exceed a limit of the service — a `LimitExceededException`.
    LimitExceeded(String),
}

impl ExportRefusal {
    /// The message the refusal carries, stated here so no two export operations word one
    /// refusal differently.
    pub fn message(&self) -> String {
        match self {
            Self::StoreUnavailable => {
                "The Compute Optimizer recommendation store is unavailable.".to_string()
            }
            Self::OptInRequired => {
                "The account is not opted in to Compute Optimizer.".to_string()
            }
            Self::InvalidParameter(message) | Self::LimitExceeded(message) => message.clone(),
        }
    }
}

/// What an admitted export call answers: the job's id, and the destination the job itself
/// holds — so the call's answer, the job's reported destination and the objects that appear
/// cannot name different places.
#[derive(Debug, Clone)]
pub struct CreatedExport {
    /// The `JobId` minted for the created job.
    pub job_id: String,
    /// The `S3Destination` of the created job.
    pub s3_destination: crate::ops::types::S3Destination,
}

/// Admit one `Export*Recommendations` request and create its job: the whole rule every export
/// operation of this service applies.
///
/// A malformed request is refused BEFORE the store is read, so an invalid parameter takes
/// precedence over the enrollment gate, as it does in every other operation of this service.
/// An `accountIds` naming an account outside the set this service answers for is such a
/// request: it asks for an export of an account this service answers nothing about, and is
/// refused with the crate's one cross-account refusal ([`super::CROSS_ACCOUNT_MESSAGE`])
/// exactly as a reader refuses it. So
/// is an `includeMemberAccounts` asking to cover the organization's member accounts from a
/// caller that manages no organization, which is refused before the enrollment gate and
/// before any job is recorded.
/// Creating a job is a write, so this is also where the jobs the seven-day reporting window
/// has dropped are let go of — a reader never writes. The export job belongs to the Region
/// whose endpoint created it, and so does the one-job-in-progress limit each export operation
/// documents: "You can have only one ... export job in progress per Amazon Web Services
/// Region", which a job that has reached its terminal status no longer holds, and which a job
/// of another resource type never holds against this one. That refusal carries the service's
/// one constant sentence for the condition ([`EXPORT_JOB_LIMIT_MESSAGE`]), whatever the
/// resource type and whichever job holds the slot.
pub fn create_export_job<Eng: Send + Sync + 'static, F: ExportFilter>(
    engine: &Arc<Eng>,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    request: ExportRequest<'_, F>,
) -> Result<CreatedExport, ExportRefusal> {
    let Some(state) = super::service_state(&**engine) else {
        return Err(ExportRefusal::StoreUnavailable);
    };

    // `accountIds`: "You can specify multiple account IDs per request", each an Amazon Web
    // Services account ID. "This parameter cannot be specified together with the include
    // member accounts parameter. The parameters are mutually exclusive."
    let account_ids = request.account_ids.unwrap_or_default();
    if !account_ids.is_empty() && request.include_member_accounts {
        return Err(ExportRefusal::InvalidParameter(
            "The value supplied for parameter accountIds is not valid: accountIds and \
             includeMemberAccounts are mutually exclusive."
                .to_string(),
        ));
    }
    if let Some(value) = account_ids
        .iter()
        .find(|value| !super::is_account_id(value))
    {
        return Err(ExportRefusal::InvalidParameter(format!(
            "The value supplied for parameter accountIds is not valid: {value} is not a \
             12-digit Amazon Web Services account ID."
        )));
    }
    // `accountIds` is the MANAGEMENT-account mechanism for naming a member account of the
    // organization the caller manages, so the accounts an export may name are the caller's own
    // and the member accounts this service records
    // ([`super::ComAmazonawsComputeoptimizerComputeoptimizerserviceState::answers_for_account`],
    // the one set every surface of this service answers for). An id naming any account outside
    // it asks for an export of an account whose analysis this service does not hold, and is
    // refused with the refusal the whole crate shares ([`super::CROSS_ACCOUNT_MESSAGE`]) — the
    // same answer a `Get*Recommendations` reader gives the same id. A named account's rows are
    // its OWN ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::export_rows`]), so
    // no file ever holds THIS account's rows under metadata naming another account.
    let names_an_unanswerable_account = {
        let guard = state.lock();
        account_ids
            .iter()
            .any(|value| !guard.answers_for_account(value))
    };
    if names_an_unanswerable_account {
        return Err(ExportRefusal::InvalidParameter(
            super::CROSS_ACCOUNT_MESSAGE.to_string(),
        ));
    }

    // The filters are the ones the surface's recommendation reader selects with, so a name or
    // value outside that vocabulary is refused here too rather than silently exporting a
    // different set.
    let filters = request.filters.unwrap_or_default();
    if let Some(reason) = (request.filter_rejection)(&filters) {
        return Err(ExportRefusal::InvalidParameter(reason));
    }

    // "You must create the destination Amazon S3 bucket for your recommendations export
    // before you create the export job. Compute Optimizer does not create the S3 bucket for
    // you." There is no export without a bucket to write it to, so a request that names none
    // is refused — and so is one naming a bucket this emulator's object store does not hold,
    // rather than queueing a job whose export file could never appear.
    let bucket = request
        .destination
        .bucket
        .as_deref()
        .map(str::trim)
        .filter(|bucket| !bucket.is_empty());
    let Some(bucket) = bucket else {
        return Err(ExportRefusal::InvalidParameter(
            "The value supplied for parameter s3DestinationConfig is not valid: a \
             destination bucket must be supplied."
                .to_string(),
        ));
    };
    if let Some(reason) = destination_bucket_rejection(&**engine, bucket) {
        return Err(ExportRefusal::InvalidParameter(reason));
    }

    let region = super::request_region(ctx);
    let now = super::now_secs();

    let mut guard = state.lock();
    // `includeMemberAccounts` asks for "recommendations for resources in all member accounts
    // of the organization", which only an organization's management account or delegated
    // administrator has, so a caller that manages none is refused rather than handed a job
    // claiming to cover an organization this service records nothing about
    // ([`super::include_member_accounts_rejection`]). Refused BEFORE the enrollment gate: it
    // is about what the request asks for, not about what the store holds for the caller, as
    // every other invalid parameter of this rule is.
    if let Some(message) = guard.include_member_accounts_rejection(request.include_member_accounts)
    {
        return Err(ExportRefusal::InvalidParameter(message.to_string()));
    }
    if !guard.enrollment.is_opted_in() {
        return Err(ExportRefusal::OptInRequired);
    }

    guard.export_jobs.prune_expired(now);

    if guard
        .export_jobs
        .in_progress_in(&region, request.resource_type, now)
        .is_some()
    {
        return Err(ExportRefusal::LimitExceeded(
            EXPORT_JOB_LIMIT_MESSAGE.to_string(),
        ));
    }
    if !guard.export_jobs.can_hold() {
        return Err(ExportRefusal::LimitExceeded(format!(
            "The request exceeds a limit of the service: this emulator holds at most {} \
             recommendation export jobs.",
            super::MAX_EXPORT_JOBS
        )));
    }

    // `recommendationPreferences$cpuVendorArchitectures` is a list of the modelled
    // `CpuVendorArchitecture` enum, so the request boundary has already refused any value
    // outside it and what reaches here is a list of architectures — the ones the exported
    // recommendations are generated under.
    let cpu_vendor_architectures = request
        .recommendation_preferences
        .and_then(|preferences| preferences.cpu_vendor_architectures)
        .unwrap_or_default();

    let job_id = guard.export_jobs.mint_job_id();
    let (key, metadata_key) = super::export_destination_keys(
        request.destination.key_prefix.as_deref(),
        ::engine::emulator_account_id(),
        request.resource_type,
        &job_id,
    );
    let plan = ExportPlan {
        job_id: job_id.clone(),
        region: region.clone(),
        resource_type: request.resource_type.to_string(),
        bucket: bucket.to_string(),
        key,
        metadata_key,
        // "The recommendations data to include in the export file", or every field the model
        // declares for this resource type when the request selects none.
        columns: export_columns(request.fields_to_export, request.exportable_fields),
        file_format: request
            .file_format
            .unwrap_or_else(|| FILE_FORMAT_CSV.to_string()),
        account_ids,
        include_member_accounts: request.include_member_accounts,
        filters: filter_pairs(&filters),
        cpu_vendor_architectures,
        // The databases the account holds an analysis for, resolved through the ONE rule every
        // database surface of this service reads them by, so an export and a read of the same
        // moment cannot differ on which databases the account has.
        analysed_databases: if request.resource_type == RESOURCE_TYPE_RDS_DB_INSTANCE {
            super::analysed_databases(
                &**engine,
                &guard,
                &region,
                ::engine::emulator_account_id(),
                now,
            )
        } else {
            Vec::new()
        },
        // The Amazon ECS services the account holds a recommendation for, resolved through the
        // ONE sweep every Amazon ECS service surface of this service reads them by, so an
        // export and a read of the same moment cannot differ on which services have a
        // recommendation or on any member of one.
        analysed_ecs_services: if request.resource_type == RESOURCE_TYPE_ECS_SERVICE {
            super::analysed_ecs_services(
                &**engine,
                &guard,
                &region,
                ::engine::emulator_account_id(),
                now,
            )
        } else {
            Vec::new()
        },
        // The volumes the account holds an analysis for, resolved through the ONE rule every
        // EBS volume surface of this service reads them by — so an export and a read of the
        // same moment cannot differ on which volumes the account has or on what each one's
        // analysis found. Each volume's recommendation is derived from the observation when the
        // rows are built ([`ObservedVolume::to_recommendation`]), under the look-back period in
        // force, exactly as the reader derives it.
        analysed_volumes: if request.resource_type == RESOURCE_TYPE_EBS_VOLUME {
            super::analysed_volumes(
                &**engine,
                &guard,
                &region,
                ::engine::emulator_account_id(),
                now,
            )
        } else {
            Vec::new()
        },
        // The Lambda function versions the account holds an analysis for, resolved through the
        // ONE rule every Lambda function surface of this service reads them by — so an export
        // and a read of the same moment cannot differ on which functions the account has or on
        // what each one's analysis found. Each function's recommendation is derived from the
        // observation when the rows are built ([`ObservedFunction::to_recommendation`]), under
        // the look-back period in force, exactly as the reader derives it.
        analysed_functions: if request.resource_type == RESOURCE_TYPE_LAMBDA_FUNCTION {
            super::analysed_functions(
                &**engine,
                &guard,
                &region,
                ::engine::emulator_account_id(),
                now,
            )
        } else {
            Vec::new()
        },
    };
    // The destination this call answers is the job's own, so it names exactly the objects
    // `DescribeRecommendationExportJobs` reports for the job and exactly the objects the
    // completed job writes.
    let s3_destination = guard.record_export_job(&plan, now).s3_destination();
    drop(guard);

    // An export this call did not create may have come due meanwhile, and this one comes due
    // two lifecycle steps from now — the task is what makes the objects appear for a client
    // that makes no further call to this service.
    deliver_and_schedule(engine, now);

    Ok(CreatedExport {
        job_id,
        s3_destination,
    })
}

/// The export file: the column header row, then one row per recommendation.
///
/// Comma-separated values as the `csv` crate writes them — quoting a cell that needs it, with
/// the CRLF record terminator RFC 4180 states — so the quoting rules are the codec's and not
/// re-derived here. A row holds no byte the codec can refuse, so the writer cannot fail; an
/// unexpected failure yields an EMPTY file rather than a truncated one a client could not tell
/// from a complete export, and is logged.
pub fn export_csv(columns: &[String], rows: &[ExportRow]) -> String {
    let mut writer = ::csv::WriterBuilder::new()
        .terminator(::csv::Terminator::CRLF)
        .from_writer(Vec::new());
    let mut write = || -> Result<(), ::csv::Error> {
        writer.write_record(columns)?;
        for row in rows {
            writer.write_record(
                columns
                    .iter()
                    .map(|column| row.get(column).map_or("", String::as_str)),
            )?;
        }
        writer.flush().map_err(::csv::Error::from)
    };
    if let Err(error) = write() {
        ::tracing::warn!(%error, "the recommendations export file could not be written");
        return String::new();
    }
    writer
        .into_inner()
        .map(|file| String::from_utf8_lossy(&file).into_owned())
        .unwrap_or_default()
}

/// The metadata file of the export `plan` describes, holding `rows` recommendation rows and
/// created at `created_at`: what the export job was, and where its two objects were written.
fn export_metadata(plan: &ExportPlan, rows: usize, created_at: f64) -> String {
    let mut metadata = ::serde_json::json!({
        "jobId": plan.job_id,
        "region": plan.region,
        "resourceType": plan.resource_type,
        "accountIds": if plan.account_ids.is_empty() {
            vec![::engine::emulator_account_id().to_string()]
        } else {
            plan.account_ids.clone()
        },
        "includeMemberAccounts": plan.include_member_accounts,
        "creationTimestamp": timestamp(created_at),
        "exportFile": {
            "fileFormat": plan.file_format,
            "columns": plan.columns,
            "rowCount": rows,
        },
        "destination": {
            "bucket": plan.bucket,
            "key": plan.key,
            "metadataKey": plan.metadata_key,
        },
    });
    // The recommendation preferences the export was asked for, stated only where the request
    // named any: a surface whose model declares no such member, and a request that asked for
    // no architecture, states no preference rather than an empty one.
    if !plan.cpu_vendor_architectures.is_empty() {
        metadata["recommendationPreferences"] = ::serde_json::json!({
            "cpuVendorArchitectures": plan.cpu_vendor_architectures,
        });
    }
    ::serde_json::to_string_pretty(&metadata).unwrap_or_else(|_| metadata.to_string())
}

/// The export COLUMN one filter name selects on, for the surface that exports
/// `resource_type` — the member the recommendation reader of that surface resolves the same
/// name to.
///
/// Each `Export*Recommendations` operation carries its own filter vocabulary, and a name is
/// spelled for the member it selects on rather than for the exportable field, so the two are
/// mapped here explicitly: the ECS, Lambda and License surfaces name one reason code
/// (`FindingReasonCode`) while the field is the list `FindingReasonCodes`, the License surface
/// selects on the license's name, and the database surface selects on the instance and storage
/// findings, their reason codes and whether the database is idle. `None` is a name the surface
/// resolves to no member of its own recommendation, which therefore selects nothing — the rule
/// the readers apply (`FindingReasonCodes` on the Auto Scaling group surface, say, whose
/// recommendation declares no such member).
fn filter_column_of(resource_type: &str, name: &str) -> Option<&'static str> {
    match (resource_type, name) {
        // The `Filters` vocabulary, shared by the Auto Scaling group and instance surfaces.
        (_, "Finding") => Some("Finding"),
        (_, "InferredWorkloadTypes") => Some("InferredWorkloadTypes"),
        // `FindingReasonCodes` selects nothing on the Auto Scaling group surface: an
        // `AutoScalingGroupRecommendation` declares no finding reason codes.
        (RESOURCE_TYPE_AUTO_SCALING_GROUP, "FindingReasonCodes") => None,
        (_, "FindingReasonCodes" | "FindingReasonCode") => Some("FindingReasonCodes"),
        // The idle resource surface.
        (RESOURCE_TYPE_IDLE, "ResourceType") => Some("ResourceType"),
        // The commercial software license surface.
        (RESOURCE_TYPE_LICENSE, "LicenseName") => {
            Some("CurrentLicenseConfigurationLicenseName")
        }
        // The database surface, which reports a finding per side of the analysis.
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "InstanceFinding") => Some("InstanceFinding"),
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "StorageFinding") => Some("StorageFinding"),
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "InstanceFindingReasonCode") => {
            Some("InstanceFindingReasonCodes")
        }
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "StorageFindingReasonCode") => {
            Some("StorageFindingReasonCodes")
        }
        (RESOURCE_TYPE_RDS_DB_INSTANCE, "Idle") => Some("Idle"),
        _ => None,
    }
}

/// Whether `row` satisfies every filter of the request that asked for the export.
///
/// A filter selects on the member its name names ([`filter_column_of`]) and the row matches
/// when that column holds any of the filter's values; `RecommendationSourceType` selects on
/// the resource type the job exports, which is the same for every row. Several filters are
/// ANDed and a filter carrying no values constrains nothing. A filter naming a member the
/// surface's own recommendation does not declare selects nothing, which is what the
/// recommendation readers of this service answer for the same filter sets.
fn row_matches_filters(
    row: &ExportRow,
    resource_type: &str,
    filters: &[(String, Vec<String>)],
) -> bool {
    filters.iter().all(|(name, values)| {
        if values.is_empty() {
            return true;
        }
        let held = if name == "RecommendationSourceType" {
            resource_type
        } else {
            match filter_column_of(resource_type, name) {
                Some(column) => row.get(column).map_or("", String::as_str),
                None => return false,
            }
        };
        held.split(LIST_SEPARATOR)
            .any(|item| !item.is_empty() && values.iter().any(|value| value == item))
    })
}

/// An instant as the export files state it: RFC 3339 in UTC, the form every timestamp this
/// service reports on the wire takes.
///
/// Stated as the READERS state it. A recommendation reader carries this same instant in the
/// epoch-seconds number the modelled timestamp form is, so the file states the instant that
/// number denotes: ONE conversion of that number, the conversion a client reading the number
/// performs, because converting it again through the number a second time lands on a
/// neighbouring tick and the file would then sit a tick away from the answer the API gave for
/// the same record.
fn timestamp(at: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// [`timestamp`] for an instant a reader ALREADY carries as the modelled timestamp: the
/// number that reader puts on the wire is this value's own epoch seconds, so the file states
/// what THAT number denotes — the same ONE conversion, reached from the reader's own value
/// rather than from a number a row converted for itself.
fn timestamp_of(at: &::aws_smithy_types::DateTime) -> String {
    timestamp(at.as_secs_f64())
}

/// Record `value` for `field` in `row`, unless it is empty: a field the analysis records no
/// value for has no entry at all, so its cell is empty whatever column order a request asks
/// for.
fn put(row: &mut ExportRow, field: &str, value: impl Into<String>) {
    let value: String = value.into();
    if !value.is_empty() {
        row.insert(field.to_string(), value);
    }
}

/// [`put`] for a derived figure, at the two decimal places every figure this service reports
/// is rounded to.
fn put_number(row: &mut ExportRow, field: &str, value: f64) {
    put(row, field, round_hundredths(value).to_string());
}

/// [`put`] for a list-valued member, as one cell ([`LIST_SEPARATOR`]).
fn put_list<T: AsRef<str>>(row: &mut ExportRow, field: &str, values: &[T]) {
    let joined: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    put(row, field, joined.join(LIST_SEPARATOR));
}

/// The columns every resource type's export file shares, from the preferences the
/// recommendation was generated under: `EffectiveRecommendationPreferences*` and the
/// look-back period in days, which is the length of the period in force.
fn put_preferences(row: &mut ExportRow, preferences: &EffectivePreferences) {
    put_number(
        row,
        "LookbackPeriodInDays",
        look_back_period_in_days(&preferences.look_back_period),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesLookBackPeriod",
        preferences.look_back_period.clone(),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesInferredWorkloadTypes",
        preferences.inferred_workload_types.clone(),
    );
    put(
        row,
        "EffectiveRecommendationPreferencesEnhancedInfrastructureMetrics",
        preferences.enhanced_infrastructure_metrics.clone(),
    );
    // `savingsEstimationMode` is an `InstanceSavingsEstimationMode`, whose one member is the
    // `source` the money figures came from — which is what the reader reports for it.
    put(
        row,
        "EffectiveRecommendationPreferencesSavingsEstimationMode",
        SAVINGS_ESTIMATION_SOURCE,
    );
    let preferred: Vec<String> = preferences
        .preferred_resources
        .iter()
        .flat_map(|resource| resource.include_list.clone().unwrap_or_default())
        .collect();
    put_list(
        row,
        "EffectiveRecommendationPreferencesPreferredResources",
        &preferred,
    );
}

/// The exportable field one utilization metric lands under —
/// `<prefix><MetricName><Statistic>` — as the resource type's own `Exportable*Field` enum
/// (`fields`) declares it.
///
/// The field sets spell every metric name in UpperCamelCase, which is how the metric-name
/// enums spell most of their own values — but not all of them: the `IdleMetricName` value is
/// `CPU` while the field is `UtilizationMetricsCpuMaximum`, and `MetricName` names the series
/// an instance type does not size in SCREAMING_SNAKE_CASE (`NETWORK_IN_BYTES_PER_SECOND`) while
/// the field is `UtilizationMetricsNetworkInBytesPerSecondMaximum`. So a name is converted to
/// that casing before the field is looked up, and a name already in it is left as it is.
///
/// The STATISTIC a field's name carries is the one the export file reports that metric under,
/// which need not be the statistic a recommendation reports it with: this API "returns
/// utilization metrics using only the `Maximum` statistic" while a field set names, say, a
/// cache hit count `UtilizationMetricsCacheHitsSum`. So the field named for the OBSERVED
/// statistic is used wherever the enum declares it — which is what picks between the two
/// fields a metric carrying one field per statistic has — and otherwise the single field that
/// enum declares for the metric, so an observed metric lands in the column that names it
/// rather than in no column at all. A metric the enum declares no unambiguous field for keeps
/// the statistic-named field, which no column of that resource type reads, so nothing is
/// reported under a name the model does not give it.
fn utilization_field(fields: &[&str], prefix: &str, name: &str, statistic: &str) -> String {
    let name = if name == "CPU" {
        "Cpu".to_string()
    } else if name.contains('_') {
        name.split('_')
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut letters = word.chars();
                letters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>()
                        + &letters.as_str().to_lowercase()
                })
            })
            .collect()
    } else {
        name.to_string()
    };
    let stem = format!("{prefix}{name}");
    let named = format!("{stem}{statistic}");
    if fields.contains(&named.as_str()) {
        return named;
    }
    let mut declared = fields.iter().filter(|field| {
        field
            .strip_prefix(stem.as_str())
            .is_some_and(|statistic| statistic.starts_with(char::is_uppercase))
    });
    match (declared.next(), declared.next()) {
        (Some(only), None) => (*only).to_string(),
        _ => named,
    }
}

/// The `Exportable*Field` an export file names a resource's own ARN under: each resource
/// type's enum declares the member its recommendation reports the ARN in
/// (`InstanceArn`, `VolumeArn`, `FunctionArn`, `ServiceArn`, `AutoScalingGroupArn`), and the
/// types whose recommendation reports it as `resourceArn` name it `ResourceArn`.
fn arn_field_of(resource_type: &str) -> &'static str {
    match resource_type {
        RESOURCE_TYPE_AUTO_SCALING_GROUP => "AutoScalingGroupArn",
        RESOURCE_TYPE_EC2_INSTANCE => "InstanceArn",
        RESOURCE_TYPE_EBS_VOLUME => "VolumeArn",
        RESOURCE_TYPE_LAMBDA_FUNCTION => "FunctionArn",
        RESOURCE_TYPE_ECS_SERVICE => "ServiceArn",
        _ => "ResourceArn",
    }
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// Create the export job `plan` describes, with the two objects it writes when the clock
    /// makes it complete, and answer the record as stored.
    ///
    /// The files are computed HERE, from the analysis as it stands at `now`, so the export is
    /// of the recommendations the account held when it asked for the export rather than of
    /// whatever the analysis says when the job happens to complete.
    pub fn record_export_job(&mut self, plan: &ExportPlan, now: f64) -> ExportJob {
        let rows = self.export_rows(plan);
        let job = ExportJob {
            job_id: plan.job_id.clone(),
            region: plan.region.clone(),
            resource_type: plan.resource_type.clone(),
            bucket: plan.bucket.clone(),
            key: plan.key.clone(),
            metadata_key: plan.metadata_key.clone(),
            created_at: now,
            pending: Some(PendingExport {
                export_file: export_csv(&plan.columns, &rows),
                metadata_file: export_metadata(plan, rows.len(), now),
            }),
            failure_reason: None,
        };
        self.export_jobs.insert(job.clone());
        job
    }

    /// The recommendation rows one export file holds: those of the job's resource type that
    /// the analysis of the accounts the request named holds and the request's filters select,
    /// in analysis order.
    ///
    /// `accountIds` selects WHOSE recommendations go into the file, so a row of an account the
    /// request did not name is not part of the export ([`ACCOUNT_ID_COLUMN`]). This emulator
    /// records an analysis for the ONE account it presents
    /// ([`engine::emulator_account_id`]), so an export that names a member account of the
    /// organization — an account an admitted request may name
    /// ([`create_export_job`]) — holds the header row alone, exactly as that account's
    /// recommendation reader reports no rows, rather than this account's rows under a file
    /// whose metadata names another account. A request naming no account exports the rows the
    /// caller can read, which is what `includeMemberAccounts` asks for over an organization
    /// whose member accounts hold no analysis here; both are stated in the metadata file as
    /// what the export was asked for.
    ///
    /// Compute Optimizer reports an account's STORED analysis to an ENROLLED account only, and
    /// only once the analysis its enrollment starts is due, so an export of that analysis
    /// created before then holds the header row alone — exactly what the recommendation
    /// readers of it report at that moment. A surface whose recommendations are computed from
    /// another service's inventory and the utilization published for it carries its own
    /// reader's gate instead ([`Self::rows_of_resource_type`]), so the file and that reader
    /// agree at every instant.
    fn export_rows(&self, plan: &ExportPlan) -> Vec<ExportRow> {
        // When the analysis these rows are of was generated: the instant every reader of this
        // service reports for it ([`Enrollment::last_analysis_refresh`]), so a file states the
        // same instant the reader reports.
        let last_refresh = self
            .enrollment
            .last_analysis_refresh()
            .map(|generated| generated.as_secs_f64());
        let mut rows = self.rows_of_resource_type(plan, last_refresh);
        if !plan.account_ids.is_empty() {
            let named: BTreeSet<&str> = plan.account_ids.iter().map(String::as_str).collect();
            rows.retain(|row| {
                row.get(ACCOUNT_ID_COLUMN)
                    .is_some_and(|account_id| named.contains(account_id.as_str()))
            });
        }
        rows.retain(|row| row_matches_filters(row, &plan.resource_type, &plan.filters));
        rows
    }

    /// The rows of the job's resource type, before the request's filters select among them:
    /// each surface's own, built from the SAME analysis its recommendation reader reports from
    /// and gated by the gate that reader applies.
    fn rows_of_resource_type(
        &self,
        plan: &ExportPlan,
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        // The Amazon ECS service recommendations the plan resolved. That analysis is computed
        // from the Amazon ECS inventory and the utilization published for it rather than
        // recorded with this account's enrollment, so its rows come from the sweep the plan
        // resolved \u2014 under the same visibility gate asked just below, which that sweep applied.
        if plan.resource_type == RESOURCE_TYPE_ECS_SERVICE {
            return plan
                .analysed_ecs_services
                .iter()
                .map(|recommendation| ecs_service_row(recommendation, last_refresh))
                .collect();
        }
        if !self.enrollment.analysis_visible() {
            return Vec::new();
        }
        match plan.resource_type.as_str() {
            RESOURCE_TYPE_AUTO_SCALING_GROUP => self.auto_scaling_group_export_rows(
                &plan.region,
                &plan.cpu_vendor_architectures,
                last_refresh,
            ),
            RESOURCE_TYPE_IDLE => self.idle_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_LICENSE => self.license_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_EC2_INSTANCE => self.instance_export_rows(
                &plan.region,
                &plan.cpu_vendor_architectures,
                last_refresh,
            ),
            RESOURCE_TYPE_RDS_DB_INSTANCE => self.rds_database_export_rows(
                &plan.analysed_databases,
                &plan.region,
                &plan.cpu_vendor_architectures,
                last_refresh,
            ),
            // The account's analysed volumes, which the plan resolved: a volume's recommendation
            // carries its own configuration, the utilization observed on it and the
            // configuration its option would move it to — none of which is a column of the
            // findings-catalog row — so the rows are built from that recommendation.
            RESOURCE_TYPE_EBS_VOLUME => {
                self.volume_export_rows(&plan.analysed_volumes, &plan.region, last_refresh)
            }
            // The account's analysed function versions, which the plan resolved: a function's
            // recommendation carries the version analysed, the memory and timeout it is
            // configured with, the utilization observed on it and the memory size its option
            // would move it to — none of which is a column of the findings-catalog row — so the
            // rows are built from that recommendation.
            RESOURCE_TYPE_LAMBDA_FUNCTION => self.lambda_function_export_rows(
                &plan.analysed_functions,
                &plan.region,
                &plan.filters,
                last_refresh,
            ),
            resource_type => {
                self.analysed_resource_export_rows(resource_type, &plan.region, last_refresh)
            }
        }
    }

    /// The export rows of the Auto Scaling group analysis: one per recommendation option of
    /// every analysed group that the request's CPU vendor architecture preference admits, since
    /// each `RecommendationOptions*` field names ONE option's value — and one row carrying the
    /// group's own columns for a group the preference leaves no candidate of.
    ///
    /// A row names each group by the ARN the endpoint the export request addressed reports it
    /// under ([`AnalyzedAutoScalingGroup::arn_in`]), the same ARN the recommendation reader
    /// answers that region with.
    fn auto_scaling_group_export_rows(
        &self,
        region: &str,
        requested_architectures: &[String],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let mut rows: Vec<ExportRow> = Vec::new();
        for group in self.visible_auto_scaling_group_analyses() {
            let arn = group.arn_in(region);
            let Some(parsed) = parse_resource_arn(&arn) else {
                continue;
            };
            let preferences = self.effective_preferences(&arn, parsed);
            // The options the exported recommendation consists of, and "the CPU vendor and
            // architecture for an instance or Auto Scaling group recommendations" they then
            // consist of: the crate's one selection of the candidate configurations the
            // preferences in force admit ([`AnalyzedAutoScalingGroup::admitted_options`]),
            // which is what the reader of these recommendations applies — "when you specify
            // AWS_ARM64 with an ExportAutoScalingGroupRecommendations request, Compute
            // Optimizer exports recommendations that consist of Graviton instance types only",
            // and the account's preferred-resource lists.
            let options = group.admitted_options(&preferences, requested_architectures);
            let architectures = stated_architectures(
                options.iter().map(|option| option.architecture.as_str()),
            );
            if options.is_empty() {
                rows.push(auto_scaling_group_row(
                    group,
                    &arn,
                    &preferences,
                    None,
                    &architectures,
                    last_refresh,
                ));
                continue;
            }
            for option in options {
                rows.push(auto_scaling_group_row(
                    group,
                    &arn,
                    &preferences,
                    Some(option),
                    &architectures,
                    last_refresh,
                ));
            }
        }
        rows
    }

    /// The export rows of the Amazon EC2 instance analysis: one per recommendation option of
    /// every analysed instance that the request's CPU vendor architecture preference admits,
    /// since each `RecommendationOptions*` field names ONE option's value — and one row
    /// carrying the instance's own columns for an instance the preference leaves no candidate
    /// of.
    ///
    /// Every cell is computed from the same stored observation the recommendation READER
    /// reports the instance from ([`ObservedInstance`]) rather than from the findings-catalog
    /// row, which records none of an instance's own configuration or of its options, so an
    /// exported row and a `GetEC2InstanceRecommendations` response cannot disagree about which
    /// options the recommendation consists of.
    fn instance_export_rows(
        &self,
        region: &str,
        requested_architectures: &[String],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let mut rows: Vec<ExportRow> = Vec::new();
        for instance in self
            .visible_instances()
            .filter(|instance| instance.account_id == account_id)
        {
            let arn = instance.instance_arn(region);
            let preferences =
                self.effective_preferences_for(&arn, RESOURCE_TYPE_EC2_INSTANCE, account_id);
            // The options the exported recommendation consists of, and "the CPU vendor and
            // architecture for an instance or Auto Scaling group recommendations" they then
            // consist of: the crate's one selection of the candidate types the preferences in
            // force admit ([`ObservedInstance::admitted_options`]), which is what the reader of
            // these recommendations applies — "when you specify AWS_ARM64 with an
            // ExportEC2InstanceRecommendations request, Compute Optimizer exports
            // recommendations that consist of Graviton instance types only", and the account's
            // preferred-resource lists.
            let options = instance.admitted_options(&preferences, requested_architectures);
            let architectures =
                stated_architectures(options.iter().map(|option| option.architecture.as_str()));
            // "Describes if an Amazon EC2 instance is idle": it is exactly when this service
            // also holds an IDLE recommendation for it, which is the record the reader of these
            // recommendations reports it from.
            let idle = self.idle_recommendations.get(&arn).is_some();
            if options.is_empty() {
                rows.push(instance_row(
                    instance,
                    &arn,
                    &preferences,
                    None,
                    &architectures,
                    idle,
                    last_refresh,
                ));
                continue;
            }
            for option in options {
                rows.push(instance_row(
                    instance,
                    &arn,
                    &preferences,
                    Some(option),
                    &architectures,
                    idle,
                    last_refresh,
                ));
            }
        }
        rows
    }

    /// The export rows of the findings catalog, for a resource type whose recommendation is
    /// reported from it: one row per analysed resource, carrying the columns the catalog row
    /// records ([`analysed_resource_row`]).
    fn analysed_resource_export_rows(
        &self,
        resource_type: &str,
        region: &str,
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let by_type = self.analysed_resources_by_type(account_id);
        by_type
            .get(resource_type)
            .map(|resources| {
                resources
                    .iter()
                    .map(|resource| {
                        let arn = resource.arn_in(region);
                        let preferences =
                            self.effective_preferences_for(&arn, resource_type, account_id);
                        analysed_resource_row(
                            resource,
                            resource_type,
                            &arn,
                            &preferences,
                            last_refresh,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The export rows of the Amazon Aurora and RDS database analysis: one per analysed
    /// database, carrying the members `GetRDSDatabaseRecommendations` reports for it
    /// ([`rds_database_row`]).
    ///
    /// A database's recommendation carries the engine, the DB instance class, the storage
    /// configuration, the utilization observed on it and the instance and storage options the
    /// analysis would move it to — none of which is a column of the findings-catalog row every
    /// other findings reader shares, so this surface builds its rows from the recommendation
    /// itself rather than from that row.
    ///
    /// `databases` are the analysed databases the plan resolved
    /// ([`ExportPlan::analysed_databases`]), which is the SAME selection
    /// `GetRDSDatabaseRecommendations` reads, so a file and a read of that moment hold the
    /// same databases.
    fn rds_database_export_rows(
        &self,
        databases: &[AnalysedResource],
        region: &str,
        requested_architectures: &[String],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        databases
            .iter()
            .map(|resource| {
                // The recommendation is built under the CPU vendor architectures the EXPORT
                // request asked for, exactly as the reader builds it under the ones a read
                // asks for, so the file's `EffectiveRecommendationPreferences*` columns state
                // the preferences the exported recommendation was generated under.
                let recommendation =
                    self.rds_recommendation(resource, region, Some(requested_architectures));
                rds_database_row(&recommendation, last_refresh)
            })
            .collect()
    }

    /// The export rows of the Amazon EBS volume analysis: one per analysed volume, carrying the
    /// members `GetEBSVolumeRecommendations` reports for it ([`volume_row`]).
    ///
    /// A volume's recommendation carries the volume's own configuration, the utilization
    /// observed on it, the configuration and performance risk of the option the analysis would
    /// move it to and the volume's tags — none of which is a column of the findings-catalog row
    /// every other findings reader shares, so this surface builds its rows from the
    /// recommendation itself rather than from that row.
    ///
    /// `volumes` are the observed volumes the plan resolved ([`ExportPlan::analysed_volumes`]),
    /// which is the SAME selection `GetEBSVolumeRecommendations` reads, and each recommendation
    /// is derived under the look-back period in force for the account's volumes — the one that
    /// reader resolves — so a file and a read of that moment hold the same volumes with the same
    /// analysis behind every cell.
    fn volume_export_rows(
        &self,
        volumes: &[ObservedVolume],
        region: &str,
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let look_back_period =
            self.effective_look_back_period(RESOURCE_TYPE_EBS_VOLUME, account_id);
        let last_refresh_timestamp = last_refresh.map(::aws_smithy_types::DateTime::from_secs_f64);
        volumes
            .iter()
            .map(|volume| {
                // The savings members net of discounts are in the file exactly where the
                // reader reports them: only once the account activated the preference that
                // activates them, resolved the way the reader resolves it.
                let after_discounts = self.reports_savings_after_discounts(
                    &volume.volume_arn(region),
                    RESOURCE_TYPE_EBS_VOLUME,
                    account_id,
                );
                let recommendation = volume.to_recommendation(
                    region,
                    &look_back_period,
                    after_discounts,
                    last_refresh_timestamp,
                );
                volume_row(&recommendation, last_refresh)
            })
            .collect()
    }

    /// The export rows of the AWS Lambda function analysis: one per analysed function version,
    /// carrying the members `GetLambdaFunctionRecommendations` reports for it
    /// ([`lambda_function_row`]).
    ///
    /// A function's recommendation carries the version analysed, the memory it is configured
    /// with, how often it ran, the duration and memory observed on it and the memory size its
    /// option would move it to with the durations that option projects — none of which is a
    /// column of the findings-catalog row every other findings reader shares, so this surface
    /// builds its rows from the recommendation itself rather than from that row.
    ///
    /// `functions` are the observed function versions the plan resolved
    /// ([`ExportPlan::analysed_functions`]), which is the SAME selection
    /// `GetLambdaFunctionRecommendations` reads, and each recommendation is derived under the
    /// look-back period in force for the account's functions — the one that reader resolves —
    /// so a file and a read of that moment hold the same functions with the same analysis
    /// behind every cell.
    ///
    /// "Functions with a finding of unavailable are not returned unless you specify the filter
    /// parameter with a value of `Unavailable`": a function the analysis reached no finding for
    /// is withheld from the file by the very rule the reader withholds it by
    /// ([`super::lambda_filters_name_unavailable`]), so an export carrying that filter holds
    /// the row the reader answers it with, and one carrying any other filter holds no row for a
    /// recommendation the reader does not report.
    fn lambda_function_export_rows(
        &self,
        functions: &[ObservedFunction],
        region: &str,
        filters: &[(String, Vec<String>)],
        last_refresh: Option<f64>,
    ) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        let look_back_period =
            self.effective_look_back_period(RESOURCE_TYPE_LAMBDA_FUNCTION, account_id);
        let last_refresh_timestamp = last_refresh.map(::aws_smithy_types::DateTime::from_secs_f64);
        let include_unavailable = super::lambda_filters_name_unavailable(
            filters
                .iter()
                .map(|(name, values)| (name.as_str(), values.as_slice())),
        );
        functions
            .iter()
            .filter_map(|function| {
                // The savings members net of discounts are in the file exactly where the reader
                // reports them: only once the account activated the preference that activates
                // them, resolved the way the reader resolves it.
                let after_discounts = self.reports_savings_after_discounts(
                    &function.function_arn(region),
                    RESOURCE_TYPE_LAMBDA_FUNCTION,
                    account_id,
                );
                let recommendation = function.to_recommendation(
                    region,
                    &look_back_period,
                    after_discounts,
                    last_refresh_timestamp,
                );
                let unavailable =
                    recommendation.finding.as_deref() == Some(LAMBDA_FINDING_UNAVAILABLE);
                (include_unavailable || !unavailable)
                    .then(|| lambda_function_row(function, &recommendation, last_refresh))
            })
            .collect()
    }

    /// The export rows of the commercial software license inventory: one per licensed
    /// instance the analysis observes, from the SAME observation
    /// `GetLicenseRecommendations` reports ([`ObservedLicensedInstance`]) rather than from the
    /// findings catalog, which records no license configuration.
    fn license_export_rows(&self, region: &str, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        self.visible_licenses()
            .filter(|license| license.account_id == account_id)
            .map(|license| {
                let arn = license.instance_arn(region);
                let preferences =
                    self.effective_preferences_for(&arn, RESOURCE_TYPE_LICENSE, account_id);
                license_row(license, &arn, region, &preferences, last_refresh)
            })
            .collect()
    }

    /// The export rows of the idle resource analysis: one per resource the analysis found
    /// idle, with the utilization it observed as the evidence for the finding.
    fn idle_export_rows(&self, region: &str, last_refresh: Option<f64>) -> Vec<ExportRow> {
        let account_id = ::engine::emulator_account_id();
        self.visible_idle_recommendations()
            .filter(|analysis| analysis.account_id == account_id)
            .map(|analysis| {
                let arn = analysis.arn_in(region);
                let preferences = self.effective_preferences_for(&arn, RESOURCE_TYPE_IDLE, account_id);
                idle_row(analysis, &arn, &preferences, last_refresh)
            })
            .collect()
    }
}

/// One Auto Scaling group recommendation option as an export row.
///
/// Every cell is the value the recommendation READER reports for that member, computed from
/// the stored analysis the same way ([`AnalyzedAutoScalingGroup`]), so an exported row and a
/// `GetAutoScalingGroupRecommendations` response cannot disagree. The fields this analysis
/// records nothing for are left empty: the reserved-instance prices and the storage and
/// network of a configuration (the analysis prices an instance-hour on public on-demand
/// pricing alone), the GPU accelerator settings and the GPU utilization (no instance type in
/// this analysis has an attached accelerator), the EBS, disk and network utilization (the
/// analysis observes CPU and memory), and an option's estimated instance-hour reduction
/// (which "is only available for EC2 Auto Scaling groups with scaling policies", and this
/// analysis records no scaling policy).
fn auto_scaling_group_row(
    group: &AnalyzedAutoScalingGroup,
    arn: &str,
    preferences: &EffectivePreferences,
    option: Option<&GroupConfiguration>,
    architectures: &[String],
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, group.account_id.clone());
    put(&mut row, "AutoScalingGroupArn", arn.to_string());
    put(&mut row, "AutoScalingGroupName", group.name.clone());
    put(&mut row, "Finding", group.finding());
    put(
        &mut row,
        "CurrentPerformanceRisk",
        group.current_performance_risk.clone(),
    );
    for metric in group.utilization_metrics() {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES,
                "UtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    put_configuration(&mut row, "CurrentConfiguration", &group.current);
    put_number(&mut row, "CurrentVCpus", group.current.vcpus);
    put_number(&mut row, "CurrentMemory", group.current.memory_gib);
    put_number(&mut row, "CurrentOnDemandPrice", group.current.hourly_price);
    put_preferences(&mut row, preferences);
    put_list(
        &mut row,
        "EffectiveRecommendationPreferencesCpuVendorArchitectures",
        architectures,
    );
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &group.inferred_workload_types,
        );
    }
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }

    let Some(option) = option else {
        return row;
    };
    put_configuration(&mut row, "RecommendationOptionsConfiguration", option);
    put_number(&mut row, "RecommendationOptionsVcpus", option.vcpus);
    put_number(&mut row, "RecommendationOptionsMemory", option.memory_gib);
    put_number(
        &mut row,
        "RecommendationOptionsOnDemandPrice",
        option.hourly_price,
    );
    put_number(
        &mut row,
        "RecommendationOptionsPerformanceRisk",
        option.performance_risk,
    );
    for metric in group.projected_utilization_metrics(option) {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_AUTO_SCALING_GROUP_FIELD_VALUES,
                "RecommendationOptionsProjectedUtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    put(
        &mut row,
        "RecommendationOptionsMigrationEffort",
        group.migration_effort(option, preferences.infers_workload_types()),
    );
    let (saving, percentage) = group.savings(option, 0.0);
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage,
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        saving,
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // The after-discount figures are reported only where the account activated the
    // preference, which is what "activates" savings net of Savings Plans and Reserved
    // Instances discounts.
    if preferences.after_discounts() {
        let (saving, percentage) = group.savings(option, group.savings_plans_discount_rate);
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityAfterDiscountsPercentage",
            percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts",
            saving,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts",
            CURRENCY_USD,
        );
    }
    row
}

/// One Auto Scaling group configuration's columns, under the prefix the field names give it
/// (`CurrentConfiguration…` for the group's own, `RecommendationOptionsConfiguration…` for an
/// option's): the members `AutoScalingGroupConfiguration` reports
/// ([`GroupConfiguration::to_wire`]).
fn put_configuration(row: &mut ExportRow, prefix: &str, configuration: &GroupConfiguration) {
    let wire = configuration.to_wire();
    put(
        row,
        &format!("{prefix}InstanceType"),
        wire.instance_type.unwrap_or_default(),
    );
    put_list(
        row,
        &format!("{prefix}MixedInstanceTypes"),
        &wire.mixed_instance_types.unwrap_or_default(),
    );
    put(row, &format!("{prefix}Type"), wire.r#type.unwrap_or_default());
    put(
        row,
        &format!("{prefix}AllocationStrategy"),
        wire.allocation_strategy.unwrap_or_default(),
    );
    put(
        row,
        &format!("{prefix}DesiredCapacity"),
        wire.desired_capacity.to_string(),
    );
    put(row, &format!("{prefix}MinSize"), wire.min_size.to_string());
    put(row, &format!("{prefix}MaxSize"), wire.max_size.to_string());
}

/// One findings-catalog row as an export row: the columns a catalog row records about any
/// analysed resource — whose account owns it, its ARN at the Region's endpoint, its finding
/// and the reason codes behind it, the risk its current configuration runs, what adopting the
/// recommendation saves, and the preferences the recommendation was generated under.
///
/// The columns specific to a resource type's own configuration are left empty here: they are
/// members of that type's recommendation rather than of the catalog row every findings reader
/// shares. The database surface, whose catalog row carries the configuration the analysis
/// recorded for it, reports those members from the recommendation itself
/// ([`rds_database_row`]).
///
/// A column whose value an effective recommendation preference switches off is left empty for
/// as long as that preference is off, the way the recommendation readers of the same analysis
/// leave the member unreported. The CPU vendor architectures of a resource's recommendation
/// options are not among the columns a catalog row fills: no resource type reported from the
/// catalog has an option carrying one — the instance surface, whose options do, builds its rows
/// from the observation itself ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::instance_export_rows`]).
fn analysed_resource_row(
    resource: &AnalysedResource,
    resource_type: &str,
    arn: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, resource.account_id.clone());
    put(&mut row, arn_field_of(resource_type), arn);
    put(&mut row, "Finding", resource.finding.clone());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &resource.finding_reason_codes,
    );
    put(
        &mut row,
        "CurrentPerformanceRisk",
        resource.current_performance_risk.clone(),
    );
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage_of(resource.monthly_savings(), resource.current_monthly_cost),
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        resource.monthly_savings(),
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // "The applications that might be running on the resource as inferred by Compute
    // Optimizer" are reported only while the inference is active for the resource — the same
    // gate the recommendation readers apply, so the cell cannot hold types the account
    // switched off while the row's effective-preference cell says they are switched off.
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &resource.inferred_workload_types,
        );
    }
    put_preferences(&mut row, preferences);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One Amazon EC2 instance recommendation option as an export row, or the instance's own
/// columns alone (`option` is `None`) where the request's preference admits no candidate of it.
///
/// Every cell is the value the recommendation READER reports for that member, computed from the
/// stored observation the same way ([`ObservedInstance`]), so an exported row and a
/// `GetEC2InstanceRecommendations` response cannot disagree. The fields this analysis records
/// nothing for are left empty: the reserved-instance prices and the storage and network of a
/// configuration (the analysis prices an instance-hour on public on-demand pricing alone), the
/// GPU accelerator settings and the GPU utilization (no instance type in this analysis has an
/// attached accelerator), and the utilization preferences, which are the preference RECORDS the
/// account wrote rather than a value this recommendation reports for the instance.
fn instance_row(
    instance: &ObservedInstance,
    arn: &str,
    preferences: &EffectivePreferences,
    option: Option<&InstanceConfiguration>,
    architectures: &[String],
    idle: bool,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, instance.account_id.clone());
    put(&mut row, "InstanceArn", arn.to_string());
    put(
        &mut row,
        "InstanceName",
        instance.instance_name().unwrap_or_default(),
    );
    put(&mut row, "InstanceState", instance.instance_state.clone());
    put(&mut row, "Finding", instance.finding());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &instance.finding_reason_codes,
    );
    put(
        &mut row,
        "CurrentPerformanceRisk",
        instance.current_performance_risk.clone(),
    );
    put(
        &mut row,
        "CurrentInstanceType",
        instance.current.instance_type.clone(),
    );
    put_number(&mut row, "CurrentVCpus", instance.current.vcpus);
    put_number(&mut row, "CurrentMemory", instance.current.memory_gib);
    put_number(
        &mut row,
        "CurrentOnDemandPrice",
        instance.current.hourly_price,
    );
    put(&mut row, "Idle", if idle { IDLE_TRUE } else { IDLE_FALSE });
    for metric in instance.utilization_metrics() {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_INSTANCE_FIELD_VALUES,
                "UtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    // "An array of objects that describe the source resource of the recommendation": an
    // instance recommendation is computed from the analysis of that very instance, which is
    // the source the reader reports for it.
    put(
        &mut row,
        "RecommendationsSourcesRecommendationSourceArn",
        arn.to_string(),
    );
    put(
        &mut row,
        "RecommendationsSourcesRecommendationSourceType",
        RESOURCE_TYPE_EC2_INSTANCE,
    );
    put_preferences(&mut row, preferences);
    // "Describes the CPU vendor and architecture for an instance or Auto Scaling group
    // recommendations": the architectures the instance's exported options consist of.
    put_list(
        &mut row,
        "EffectiveRecommendationPreferencesCpuVendorArchitectures",
        architectures,
    );
    // The external metrics provider the account prefers, and Compute Optimizer's integration
    // status with it — the crate's one rule for that status ([`external_metric_status`]), so the
    // file and the reader cannot report different integrations.
    let source = preferences
        .external_metrics_preference
        .as_ref()
        .and_then(|preference| preference.source.as_deref());
    put(
        &mut row,
        "EffectiveRecommendationPreferencesExternalMetricsSource",
        source.unwrap_or_default(),
    );
    let status = external_metric_status(source);
    put(
        &mut row,
        "ExternalMetricStatusCode",
        status.status_code.unwrap_or_default(),
    );
    put(
        &mut row,
        "ExternalMetricStatusReason",
        status.status_reason.unwrap_or_default(),
    );
    // "The applications that might be running on the instance as inferred by Compute Optimizer"
    // are reported only while the inference is active for it — the same gate the reader applies.
    if preferences.infers_workload_types() {
        put_list(
            &mut row,
            "InferredWorkloadTypes",
            &instance.inferred_workload_types,
        );
    }
    let tags: Vec<String> = instance
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }

    let Some(option) = option else {
        return row;
    };
    put(
        &mut row,
        "RecommendationOptionsInstanceType",
        option.instance_type.clone(),
    );
    put_number(&mut row, "RecommendationOptionsVcpus", option.vcpus);
    put_number(&mut row, "RecommendationOptionsMemory", option.memory_gib);
    put_number(
        &mut row,
        "RecommendationOptionsOnDemandPrice",
        option.hourly_price,
    );
    put_number(
        &mut row,
        "RecommendationOptionsPerformanceRisk",
        instance.option_performance_risk(option),
    );
    put_list(
        &mut row,
        "RecommendationOptionsPlatformDifferences",
        &instance.platform_differences(option),
    );
    put(
        &mut row,
        "RecommendationOptionsMigrationEffort",
        instance.migration_effort(option, preferences.infers_workload_types()),
    );
    for metric in instance.projected_utilization_metrics(option) {
        let (Some(name), Some(statistic)) = (metric.name, metric.statistic) else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_INSTANCE_FIELD_VALUES,
                "RecommendationOptionsProjectedUtilizationMetrics",
                &name,
                &statistic,
            ),
            metric.value,
        );
    }
    let (saving, percentage) = instance.savings(option);
    put_number(
        &mut row,
        "RecommendationOptionsSavingsOpportunityPercentage",
        percentage,
    );
    put_number(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        saving,
    );
    put(
        &mut row,
        "RecommendationOptionsEstimatedMonthlySavingsCurrency",
        CURRENCY_USD,
    );
    // The after-discount figures are reported only where the account activated the preference,
    // which is what "activates" savings net of Savings Plans and Reserved Instances discounts.
    if preferences.after_discounts() {
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityAfterDiscountsPercentage",
            percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts",
            saving,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts",
            CURRENCY_USD,
        );
    }
    row
}

/// The `ExportableRDSDBField` prefix the columns of ONE recommendation option carry: the DB
/// instance option's and the DB storage option's, each named for the member of
/// `RDSDBRecommendation` that option is reported in.
const INSTANCE_OPTION: &str = "InstanceRecommendationOptions";
/// See [`INSTANCE_OPTION`].
const STORAGE_OPTION: &str = "StorageRecommendationOptions";

/// One Amazon Aurora or RDS database recommendation as an export row: the value of every
/// `ExportableRDSDBField` the recommendation carries, taken from the member that field names.
///
/// `recommendation` is the one `GetRDSDatabaseRecommendations` reports for the same catalog
/// row, so an exported row and that reader's response cannot disagree about any column.
///
/// The fields that name no member of an `RDSDBRecommendation` are left empty, because the
/// analysis records no value for them: whether the database is a Multi-AZ deployment or the
/// writer of its cluster, and the per-hour and per-month PRICES of the current and recommended
/// configurations together with the cluster-level cost roll-ups — this analysis records a
/// database's cost per month as a whole, and the money the recommendation reports is what
/// adopting it saves.
fn rds_database_row(recommendation: &RDSDBRecommendation, last_refresh: Option<f64>) -> ExportRow {
    let text = |member: &Option<String>| member.clone().unwrap_or_default();
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, text(&recommendation.account_id));
    put(&mut row, "ResourceArn", text(&recommendation.resource_arn));
    put(&mut row, "Engine", text(&recommendation.engine));
    put(
        &mut row,
        "EngineVersion",
        text(&recommendation.engine_version),
    );
    put(
        &mut row,
        "DBClusterIdentifier",
        text(&recommendation.db_cluster_identifier),
    );
    if let Some(tier) = recommendation.promotion_tier {
        put(&mut row, "PromotionTier", tier.to_string());
    }
    put(&mut row, "Idle", text(&recommendation.idle));
    put(
        &mut row,
        "CurrentDBInstanceClass",
        text(&recommendation.current_db_instance_class),
    );
    if let Some(configuration) = &recommendation.current_storage_configuration {
        put_storage_configuration(&mut row, "CurrentStorageConfiguration", configuration);
    }
    put(
        &mut row,
        "CurrentStorageEstimatedMonthlyVolumeIOPsCostVariation",
        text(
            &recommendation.current_storage_estimated_monthly_volume_io_ps_cost_variation,
        ),
    );
    put(
        &mut row,
        "CurrentInstancePerformanceRisk",
        text(&recommendation.current_instance_performance_risk),
    );
    put(
        &mut row,
        "InstanceFinding",
        text(&recommendation.instance_finding),
    );
    put(
        &mut row,
        "StorageFinding",
        text(&recommendation.storage_finding),
    );
    put_list(
        &mut row,
        "InstanceFindingReasonCodes",
        recommendation
            .instance_finding_reason_codes
            .as_deref()
            .unwrap_or_default(),
    );
    put_list(
        &mut row,
        "StorageFindingReasonCodes",
        recommendation
            .storage_finding_reason_codes
            .as_deref()
            .unwrap_or_default(),
    );
    put_utilization_metrics(
        &mut row,
        "UtilizationMetrics",
        recommendation.utilization_metrics.as_deref().unwrap_or_default(),
    );

    // The DB instance option: the class the analysis would move the database to, the
    // utilization projected onto it, the risk it then runs and what adopting it saves. Each
    // `*RecommendationOptions*` field names ONE option, and this analysis reports one option
    // per side, so both sides' columns go in the same row.
    if let Some(option) = recommendation
        .instance_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        put(
            &mut row,
            "InstanceRecommendationOptionsDBInstanceClass",
            text(&option.db_instance_class),
        );
        put(
            &mut row,
            "InstanceRecommendationOptionsRank",
            option.rank.to_string(),
        );
        put_number(
            &mut row,
            "InstanceRecommendationOptionsPerformanceRisk",
            option.performance_risk,
        );
        put_utilization_metrics(
            &mut row,
            "InstanceRecommendationOptionsProjectedUtilizationMetrics",
            option
                .projected_utilization_metrics
                .as_deref()
                .unwrap_or_default(),
        );
        put_savings(&mut row, INSTANCE_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                INSTANCE_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
    }

    // The DB storage option: the storage configuration the analysis would move the database
    // to, and what adopting it saves.
    if let Some(option) = recommendation
        .storage_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        if let Some(configuration) = &option.storage_configuration {
            put_storage_configuration(&mut row, STORAGE_OPTION, configuration);
        }
        put(
            &mut row,
            "StorageRecommendationOptionsRank",
            option.rank.to_string(),
        );
        put(
            &mut row,
            "StorageRecommendationOptionsEstimatedMonthlyVolumeIOPsCostVariation",
            text(&option.estimated_monthly_volume_io_ps_cost_variation),
        );
        put_savings(&mut row, STORAGE_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                STORAGE_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
    }

    // The preferences the recommendation was generated under, as the recommendation itself
    // reports them. The database field set declares a column for each member an
    // `RDSEffectiveRecommendationPreferences` carries.
    if let Some(preferences) = &recommendation.effective_recommendation_preferences {
        put_list(
            &mut row,
            "EffectiveRecommendationPreferencesCpuVendorArchitectures",
            preferences
                .cpu_vendor_architectures
                .as_deref()
                .unwrap_or_default(),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesEnhancedInfrastructureMetrics",
            text(&preferences.enhanced_infrastructure_metrics),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesLookBackPeriod",
            text(&preferences.look_back_period),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesSavingsEstimationMode",
            preferences
                .savings_estimation_mode
                .as_ref()
                .map(|mode| text(&mode.source))
                .unwrap_or_default(),
        );
    }
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        recommendation.lookback_period_in_days,
    );
    put_modelled_tags(&mut row, recommendation.tags.as_deref());
    // "The timestamp of when the DB instance recommendation was last generated": the instant
    // the recommendation reports where the analysis recorded one, and otherwise the instant
    // this account's analysis was last refreshed, which is what every other export file of
    // this service states. The instant the recommendation itself carries is stated FROM that
    // value ([`timestamp_of`]), so the cell and the reader's own number denote one instant.
    let refreshed = recommendation
        .last_refresh_timestamp
        .as_ref()
        .map(timestamp_of)
        .or_else(|| last_refresh.map(timestamp));
    if let Some(stated) = refreshed {
        put(&mut row, "LastRefreshTimestamp", stated);
    }
    row
}

/// The columns one `DBStorageConfiguration` fills, under the `prefix` the field set names that
/// configuration with (`CurrentStorageConfiguration` for the database's own,
/// `StorageRecommendationOptions` for the one its storage option would move it to).
fn put_storage_configuration(
    row: &mut ExportRow,
    prefix: &str,
    configuration: &DBStorageConfiguration,
) {
    put(
        row,
        &format!("{prefix}StorageType"),
        configuration.storage_type.clone().unwrap_or_default(),
    );
    put(
        row,
        &format!("{prefix}AllocatedStorage"),
        configuration.allocated_storage.to_string(),
    );
    if let Some(iops) = configuration.iops {
        put(row, &format!("{prefix}IOPS"), iops.to_string());
    }
    if let Some(maximum) = configuration.max_allocated_storage {
        put(
            row,
            &format!("{prefix}MaxAllocatedStorage"),
            maximum.to_string(),
        );
    }
    if let Some(throughput) = configuration.storage_throughput {
        put(
            row,
            &format!("{prefix}StorageThroughput"),
            throughput.to_string(),
        );
    }
}

/// The savings columns of one recommendation option: the saving as a percentage of what the
/// resource costs today, and the amount and currency of that saving.
fn put_savings(row: &mut ExportRow, prefix: &str, opportunity: Option<&SavingsOpportunity>) {
    let Some(opportunity) = opportunity else {
        return;
    };
    put_number(
        row,
        &format!("{prefix}SavingsOpportunityPercentage"),
        opportunity.savings_opportunity_percentage,
    );
    if let Some(savings) = &opportunity.estimated_monthly_savings {
        put(
            row,
            &format!("{prefix}EstimatedMonthlySavingsCurrency"),
            savings.currency.clone().unwrap_or_default(),
        );
        put_number(
            row,
            &format!("{prefix}EstimatedMonthlySavingsValue"),
            savings.value,
        );
    }
}

/// The AFTER-DISCOUNTS savings columns of one recommendation option, which each option shape
/// carries under its own type but reports the same three figures in: the percentage, and the
/// currency and amount of the estimated monthly saving.
fn put_savings_after_discounts(
    row: &mut ExportRow,
    prefix: &str,
    percentage: f64,
    estimated: Option<(String, f64)>,
) {
    put_number(
        row,
        &format!("{prefix}SavingsOpportunityAfterDiscountsPercentage"),
        percentage,
    );
    if let Some((currency, value)) = estimated {
        put(
            row,
            &format!("{prefix}EstimatedMonthlySavingsCurrencyAfterDiscounts"),
            currency,
        );
        put_number(
            row,
            &format!("{prefix}EstimatedMonthlySavingsValueAfterDiscounts"),
            value,
        );
    }
}

/// The utilization columns of a database's observed or projected metrics: each metric under
/// the field that names it and the statistic it was measured with ([`utilization_field`]).
fn put_utilization_metrics(row: &mut ExportRow, prefix: &str, metrics: &[RDSDBUtilizationMetric]) {
    for metric in metrics {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            row,
            &utilization_field(
                EXPORTABLE_R_D_S_D_B_FIELD_VALUES,
                prefix,
                name,
                statistic,
            ),
            metric.value,
        );
    }
}

/// One Amazon EBS volume recommendation as an export row: the value of every
/// `ExportableVolumeField` the recommendation carries, taken from the member that field names.
///
/// `recommendation` is the one `GetEBSVolumeRecommendations` reports for the same observed
/// volume, so an exported row and that reader's response cannot disagree about any column —
/// including the look-back period in force, which decides the finding, the risks and the
/// option.
///
/// The two PRICE columns name no member of a `VolumeRecommendation`: the recommendation reports
/// what adopting its option saves rather than what either configuration costs, so each price is
/// the monthly cost of that configuration under the one price table the savings themselves are
/// computed from ([`super::volume_monthly_cost`]), and is empty for a volume type that table
/// holds no price for — the same volume whose option then carries no savings figure.
fn volume_row(recommendation: &VolumeRecommendation, last_refresh: Option<f64>) -> ExportRow {
    let text = |member: &Option<String>| member.clone().unwrap_or_default();
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, text(&recommendation.account_id));
    put(
        &mut row,
        arn_field_of(RESOURCE_TYPE_EBS_VOLUME),
        text(&recommendation.volume_arn),
    );
    put(&mut row, "Finding", text(&recommendation.finding));
    put(
        &mut row,
        "CurrentPerformanceRisk",
        text(&recommendation.current_performance_risk),
    );
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        recommendation.look_back_period_in_days,
    );
    // The volume's own configuration, and whether it is the root volume of the instance it is
    // attached to — which the field set names both as a column of that configuration and on its
    // own, two columns the one member fills.
    if let Some(configuration) = &recommendation.current_configuration {
        put_volume_configuration(&mut row, CURRENT_VOLUME_CONFIGURATION, configuration);
        if let Some(root) = configuration.root_volume {
            put(&mut row, "RootVolume", root.to_string());
            put(
                &mut row,
                &format!("{CURRENT_VOLUME_CONFIGURATION}RootVolume"),
                root.to_string(),
            );
        }
        put(
            &mut row,
            "CurrentMonthlyPrice",
            monthly_price(configuration),
        );
    }
    put_volume_utilization(
        &mut row,
        recommendation
            .utilization_metrics
            .as_deref()
            .unwrap_or_default(),
    );

    // The volume option: the configuration the analysis would move the volume to, the risk it
    // then runs and what adopting it saves. Each `RecommendationOptions*` field names ONE
    // option, and this analysis reports one option per volume, so its columns go in the row of
    // the volume it is of.
    if let Some(option) = recommendation
        .volume_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        if let Some(configuration) = &option.configuration {
            put_volume_configuration(&mut row, VOLUME_OPTION_CONFIGURATION, configuration);
            put(
                &mut row,
                "RecommendationOptionsMonthlyPrice",
                monthly_price(configuration),
            );
        }
        put_number(
            &mut row,
            "RecommendationOptionsPerformanceRisk",
            option.performance_risk,
        );
        put_savings(&mut row, VOLUME_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                VOLUME_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
    }

    // The preferences the recommendation was generated under, as the recommendation itself
    // reports them. The volume field set declares a column for each member an
    // `EBSEffectiveRecommendationPreferences` carries.
    if let Some(preferences) = &recommendation.effective_recommendation_preferences {
        put(
            &mut row,
            "EffectiveRecommendationPreferencesLookBackPeriod",
            text(&preferences.look_back_period),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesSavingsEstimationMode",
            preferences
                .savings_estimation_mode
                .as_ref()
                .map(|mode| text(&mode.source))
                .unwrap_or_default(),
        );
    }
    put_modelled_tags(&mut row, recommendation.tags.as_deref());
    // "The timestamp of when the volume recommendation was last generated": the instant the
    // recommendation reports where the analysis recorded one, and otherwise the instant this
    // account's analysis was last refreshed, which is what every other export file of this
    // service states. The instant the recommendation itself carries is stated FROM that value
    // ([`timestamp_of`]), so the cell and the reader's own number denote one instant.
    let refreshed = recommendation
        .last_refresh_timestamp
        .as_ref()
        .map(timestamp_of)
        .or_else(|| last_refresh.map(timestamp));
    if let Some(stated) = refreshed {
        put(&mut row, "LastRefreshTimestamp", stated);
    }
    row
}

/// The `ExportableVolumeField` prefix the columns of the volume's OWN configuration carry.
const CURRENT_VOLUME_CONFIGURATION: &str = "CurrentConfiguration";

/// The `ExportableVolumeField` prefix the columns of the configuration ONE recommendation option
/// would move the volume to carry.
const VOLUME_OPTION_CONFIGURATION: &str = "RecommendationOptionsConfiguration";

/// The `ExportableVolumeField` prefix the columns of ONE volume recommendation option carry,
/// named for the member of `VolumeRecommendation` that option is reported in.
const VOLUME_OPTION: &str = "RecommendationOptions";

/// What one month of a volume with this configuration costs, as a price column states it, or the
/// empty cell for a volume type this service holds no price for ([`super::volume_monthly_cost`])
/// — whose recommendation carries no savings figure either.
fn monthly_price(configuration: &VolumeConfiguration) -> String {
    super::volume_monthly_cost(configuration)
        .map(|price| round_hundredths(price).to_string())
        .unwrap_or_default()
}

/// The columns one `VolumeConfiguration` fills, under the `prefix` the field set names that
/// configuration with ([`CURRENT_VOLUME_CONFIGURATION`] for the volume's own,
/// [`VOLUME_OPTION_CONFIGURATION`] for the one its option would move it to): the type, the size
/// and the baseline and burst performance that configuration delivers.
///
/// `rootVolume` is not among them: it is a property of the volume rather than of a
/// configuration, so the field set declares a column for it only beside the volume's own
/// configuration, which is where [`volume_row`] states it.
fn put_volume_configuration(
    row: &mut ExportRow,
    prefix: &str,
    configuration: &VolumeConfiguration,
) {
    put(
        row,
        &format!("{prefix}VolumeType"),
        configuration.volume_type.clone().unwrap_or_default(),
    );
    put(
        row,
        &format!("{prefix}VolumeSize"),
        configuration.volume_size.to_string(),
    );
    put(
        row,
        &format!("{prefix}VolumeBaselineIOPS"),
        configuration.volume_baseline_iops.to_string(),
    );
    put(
        row,
        &format!("{prefix}VolumeBaselineThroughput"),
        configuration.volume_baseline_throughput.to_string(),
    );
    put(
        row,
        &format!("{prefix}VolumeBurstIOPS"),
        configuration.volume_burst_iops.to_string(),
    );
    put(
        row,
        &format!("{prefix}VolumeBurstThroughput"),
        configuration.volume_burst_throughput.to_string(),
    );
}

/// The utilization columns of a volume's observed metrics: each metric under the field that
/// names it and the statistic it was measured with ([`utilization_field`]).
fn put_volume_utilization(row: &mut ExportRow, metrics: &[EBSUtilizationMetric]) {
    for metric in metrics {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            row,
            &utilization_field(
                EXPORTABLE_VOLUME_FIELD_VALUES,
                "UtilizationMetrics",
                name,
                statistic,
            ),
            metric.value,
        );
    }
}

/// The `Tags` column of a row whose recommendation reports the resource's tags as the modelled
/// `Tags` list: the tags as `key=value` pairs in one cell ([`LIST_SEPARATOR`]), so one
/// exportable field is still one column. ONE spelling for every surface that reports that list,
/// so no two export files state a resource's tags differently.
fn put_modelled_tags(row: &mut ExportRow, tags: Option<&[Tag]>) {
    let pairs: Vec<String> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            format!(
                "{}={}",
                tag.key.clone().unwrap_or_default(),
                tag.value.clone().unwrap_or_default()
            )
        })
        .collect();
    put_list(row, "Tags", &pairs);
}

/// One AWS Lambda function recommendation as an export row: the value of every
/// `ExportableLambdaFunctionField` the recommendation carries, taken from the member that field
/// names.
///
/// `recommendation` is the one `GetLambdaFunctionRecommendations` reports for the same observed
/// function version, so an exported row and that reader's response cannot disagree about any
/// column — including the look-back period in force, which decides the finding, the invocations
/// counted, the utilization reported and the memory size the option names.
///
/// The TIMEOUT and the four COST columns name no member of a `LambdaFunctionRecommendation`:
/// the recommendation reports what adopting its option saves rather than what either
/// configuration costs, and carries the memory a version is configured with but not the time
/// one of its invocations may run. So they are taken from the observation the recommendation
/// itself was derived from ([`ObservedFunction`]) — the configured timeout, what the analysed
/// period's invocations cost at the memory in force, the mean cost of one of its days, and what
/// the same invocations would cost at the recommended memory size at the shortest and the
/// longest invocation observed, under the one price table the savings themselves are computed
/// from.
fn lambda_function_row(
    function: &ObservedFunction,
    recommendation: &LambdaFunctionRecommendation,
    last_refresh: Option<f64>,
) -> ExportRow {
    let text = |member: &Option<String>| member.clone().unwrap_or_default();
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, text(&recommendation.account_id));
    put(
        &mut row,
        arn_field_of(RESOURCE_TYPE_LAMBDA_FUNCTION),
        text(&recommendation.function_arn),
    );
    put(
        &mut row,
        "FunctionVersion",
        text(&recommendation.function_version),
    );
    put(&mut row, "Finding", text(&recommendation.finding));
    put_list(
        &mut row,
        "FindingReasonCodes",
        recommendation
            .finding_reason_codes
            .as_deref()
            .unwrap_or_default(),
    );
    put(
        &mut row,
        "CurrentPerformanceRisk",
        text(&recommendation.current_performance_risk),
    );
    put(
        &mut row,
        "NumberOfInvocations",
        recommendation.number_of_invocations.to_string(),
    );
    // The window every figure below was derived over: the one the recommendation itself
    // reports, so the file's cost columns are of the period the reader's own members are of.
    let look_back_days = recommendation.lookback_period_in_days;
    put_number(&mut row, "LookbackPeriodInDays", look_back_days);
    put(
        &mut row,
        "CurrentConfigurationMemorySize",
        recommendation.current_memory_size.to_string(),
    );
    put(
        &mut row,
        "CurrentConfigurationTimeout",
        function.configured_timeout_seconds.to_string(),
    );
    for metric in recommendation.utilization_metrics.iter().flatten() {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES,
                "UtilizationMetrics",
                name,
                statistic,
            ),
            metric.value,
        );
    }
    put_number(
        &mut row,
        "CurrentCostTotal",
        function.analysed_cost(look_back_days),
    );
    put_number(
        &mut row,
        "CurrentCostAverage",
        function.average_daily_cost(look_back_days),
    );

    // The memory size option: the memory the analysis would move the function to, the durations
    // it then projects and what adopting it saves. Each `RecommendationOptions*` field names ONE
    // option, and this analysis reports one option per function, so its columns go in the row of
    // the function it is of. A function the analysis reached no finding for has no option at all
    // — "Compute Optimizer was unable to generate a recommendation" for it — so every one of
    // these columns is empty for it, exactly as the reader reports no option.
    if let Some(option) = recommendation
        .memory_size_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        put(
            &mut row,
            "RecommendationOptionsConfigurationMemorySize",
            option.memory_size.to_string(),
        );
        for metric in option.projected_utilization_metrics.iter().flatten() {
            let (Some(name), Some(statistic)) =
                (metric.name.as_deref(), metric.statistic.as_deref())
            else {
                continue;
            };
            put_number(
                &mut row,
                &utilization_field(
                    EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES,
                    "RecommendationOptionsProjectedUtilizationMetrics",
                    name,
                    statistic,
                ),
                metric.value,
            );
        }
        let (low, high) = function.recommended_cost_bounds(look_back_days);
        put_number(&mut row, "RecommendationOptionsCostLow", low);
        put_number(&mut row, "RecommendationOptionsCostHigh", high);
        put_savings(&mut row, FUNCTION_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                FUNCTION_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
    }

    // The preferences the recommendation was generated under, as the recommendation itself
    // reports them: a `LambdaEffectiveRecommendationPreferences` carries one member, and the
    // field set declares one column for it.
    if let Some(preferences) = &recommendation.effective_recommendation_preferences {
        put(
            &mut row,
            "EffectiveRecommendationPreferencesSavingsEstimationMode",
            preferences
                .savings_estimation_mode
                .as_ref()
                .map(|mode| text(&mode.source))
                .unwrap_or_default(),
        );
    }
    put_modelled_tags(&mut row, recommendation.tags.as_deref());
    // "The timestamp of when the function recommendation was last generated": the instant the
    // recommendation reports, and otherwise the instant this account's analysis was last
    // refreshed, which is what every other export file of this service states. The instant the
    // recommendation itself carries is stated FROM that value ([`timestamp_of`]), so the cell
    // and the reader's own number denote one instant.
    let refreshed = recommendation
        .last_refresh_timestamp
        .as_ref()
        .map(timestamp_of)
        .or_else(|| last_refresh.map(timestamp));
    if let Some(stated) = refreshed {
        put(&mut row, "LastRefreshTimestamp", stated);
    }
    row
}

/// The `ExportableLambdaFunctionField` prefix the columns of ONE memory size recommendation
/// option carry, named for the member of `LambdaFunctionRecommendation` that option is reported
/// in.
const FUNCTION_OPTION: &str = "RecommendationOptions";

/// One commercial software license recommendation as an export row: the members
/// `LicenseRecommendation` reports, each computed from the stored observation the same way
/// `GetLicenseRecommendations` computes it — the license configuration in force, the finding
/// and the reason code behind it, and the one option the analysis has (the edition it would
/// move the license to) with what adopting it saves. A license with no price carries no
/// savings figure rather than an invented one.
fn license_row(
    license: &ObservedLicensedInstance,
    arn: &str,
    region: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, license.account_id.clone());
    put(&mut row, "ResourceArn", arn);
    put(&mut row, "Finding", license.finding());
    put_list(
        &mut row,
        "FindingReasonCodes",
        &license.finding_reason_codes(),
    );
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        license.analysed_days(&preferences.look_back_period),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationInstanceType",
        license.instance_type.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationOperatingSystem",
        license.operating_system.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseName",
        license.license_name.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseEdition",
        license.license_edition.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseModel",
        license.license_model.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationLicenseVersion",
        license.license_version.clone(),
    );
    put(
        &mut row,
        "CurrentLicenseConfigurationNumberOfCores",
        license.number_of_cores.to_string(),
    );
    // The metric source is the CloudWatch Application Insights application that watches the
    // instance, which the reader reports as its provider and that application's ARN.
    let sources: Vec<String> = license
        .metrics_source(region)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|source| source.provider_arn)
        .collect();
    put_list(
        &mut row,
        "CurrentLicenseConfigurationMetricsSource",
        &sources,
    );
    if let Some(recommended) = license.recommended_license_edition() {
        put(&mut row, "RecommendationOptionsLicenseEdition", recommended);
        put(
            &mut row,
            "RecommendationOptionsLicenseModel",
            license.license_model.clone(),
        );
        put(
            &mut row,
            "RecommendationOptionsOperatingSystem",
            license.operating_system.clone(),
        );
    }
    if let Some(savings) = license.savings() {
        put_number(
            &mut row,
            "RecommendationOptionsSavingsOpportunityPercentage",
            savings.percentage,
        );
        put_number(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsValue",
            savings.monthly_amount,
        );
        put(
            &mut row,
            "RecommendationOptionsEstimatedMonthlySavingsCurrency",
            CURRENCY_USD,
        );
    }
    let tags: Vec<String> = license
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One idle resource recommendation as an export row: the members
/// `IdleRecommendation` reports ([`IdleResourceAnalysis::to_recommendation`]), each computed
/// from the stored analysis the same way the reader computes it.
fn idle_row(
    analysis: &IdleResourceAnalysis,
    arn: &str,
    preferences: &EffectivePreferences,
    last_refresh: Option<f64>,
) -> ExportRow {
    let look_back_days = analysis.analysed_days(&preferences.look_back_period);
    let (percentage, _value) = analysis.savings();
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, analysis.account_id.clone());
    put(&mut row, "ResourceArn", arn);
    put(&mut row, "ResourceId", analysis.resource_id());
    put(
        &mut row,
        "ResourceType",
        analysis.resource_type().unwrap_or_default(),
    );
    put(&mut row, "Finding", analysis.finding.clone());
    put(
        &mut row,
        "FindingDescription",
        analysis.finding_description(look_back_days),
    );
    put_number(&mut row, "LookbackPeriodInDays", look_back_days);
    // `SavingsOpportunity` / `SavingsOpportunityAfterDiscount` are one column each, and what
    // an `IdleSavingsOpportunity` is named for is its percentage: "the estimated monthly
    // savings possible as a percentage of monthly cost".
    put_number(&mut row, "SavingsOpportunity", percentage);
    // The saving net of Savings Plans and Reserved Instances discounts is in the file exactly
    // where the reader reports it: only once the account activated the preference that
    // activates it ([`EffectivePreferences::after_discounts`]).
    if preferences.after_discounts() {
        put_number(&mut row, "SavingsOpportunityAfterDiscount", percentage);
    }
    for metric in &analysis.utilization_metrics {
        let (Some(name), Some(statistic)) = (metric.name.as_deref(), metric.statistic.as_deref())
        else {
            continue;
        };
        put_number(
            &mut row,
            &utilization_field(
                EXPORTABLE_IDLE_FIELD_VALUES,
                "UtilizationMetrics",
                name,
                statistic,
            ),
            metric.value,
        );
    }
    let tags: Vec<String> = analysis
        .tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    put_list(&mut row, "Tags", &tags);
    if let Some(at) = last_refresh {
        put(&mut row, "LastRefreshTimestamp", timestamp(at));
    }
    row
}

/// One Amazon ECS service recommendation as an export row: the value of every
/// `ExportableECSServiceField` the recommendation carries, taken from the member that field
/// names.
///
/// `recommendation` is the one `GetECSServiceRecommendations` reports for the same service
/// ([`super::ecs_service_recommendation`], the ONE computation both surfaces read), so an
/// exported row and that reader's response cannot disagree about any column. An Amazon ECS
/// service recommendation carries the service's current task size and container definitions,
/// the utilization observed on it, the task size the analysis would move it to and what
/// containers it then recommends — none of which the shared findings-catalog row records — so
/// this surface builds its rows from the recommendation itself, as the Amazon EC2 instance and
/// the database surfaces do.
fn ecs_service_row(
    recommendation: &ECSServiceRecommendation,
    last_refresh: Option<f64>,
) -> ExportRow {
    let text = |member: &Option<String>| member.clone().unwrap_or_default();
    let mut row = ExportRow::new();
    put(&mut row, ACCOUNT_ID_COLUMN, text(&recommendation.account_id));
    put(
        &mut row,
        arn_field_of(RESOURCE_TYPE_ECS_SERVICE),
        text(&recommendation.service_arn),
    );
    put(&mut row, "LaunchType", text(&recommendation.launch_type));
    put(&mut row, "Finding", text(&recommendation.finding));
    put_list(
        &mut row,
        "FindingReasonCodes",
        recommendation
            .finding_reason_codes
            .as_deref()
            .unwrap_or_default(),
    );
    put(
        &mut row,
        "CurrentPerformanceRisk",
        text(&recommendation.current_performance_risk),
    );
    put_number(
        &mut row,
        "LookbackPeriodInDays",
        recommendation.lookback_period_in_days,
    );

    // "The configuration of the current Amazon ECS service": the task size it runs today, the
    // task definition it runs and that definition's container reservations.
    if let Some(configuration) = &recommendation.current_service_configuration {
        if let Some(cpu) = configuration.cpu {
            put(&mut row, "CurrentServiceConfigurationCpu", cpu.to_string());
        }
        if let Some(memory) = configuration.memory {
            put(
                &mut row,
                "CurrentServiceConfigurationMemory",
                memory.to_string(),
            );
        }
        put(
            &mut row,
            "CurrentServiceConfigurationTaskDefinitionArn",
            text(&configuration.task_definition_arn),
        );
        put(
            &mut row,
            "CurrentServiceConfigurationAutoScalingConfiguration",
            text(&configuration.auto_scaling_configuration),
        );
        let containers: Vec<String> = configuration
            .container_configurations
            .iter()
            .flatten()
            .map(|container| {
                container_cell(
                    container.container_name.as_deref(),
                    container.cpu,
                    container.memory_size_configuration.as_ref(),
                )
            })
            .collect();
        put_list(
            &mut row,
            "CurrentServiceContainerConfigurations",
            &containers,
        );
    }
    for metric in recommendation.utilization_metrics.iter().flatten() {
        put_ecs_utilization(
            &mut row,
            "UtilizationMetrics",
            metric.name.as_deref(),
            metric.statistic.as_deref(),
            metric.value,
        );
    }

    // The recommendation option: the task size the analysis would move the service to, the
    // utilization projected onto it, what the containers then reserve and what adopting it
    // saves. Each `RecommendationOptions*` field names ONE option, and this analysis reports
    // one per service.
    if let Some(option) = recommendation
        .service_recommendation_options
        .iter()
        .flatten()
        .next()
    {
        if let Some(cpu) = option.cpu {
            put(&mut row, "RecommendationOptionsCpu", cpu.to_string());
        }
        if let Some(memory) = option.memory {
            put(&mut row, "RecommendationOptionsMemory", memory.to_string());
        }
        put_savings(&mut row, SERVICE_OPTION, option.savings_opportunity.as_ref());
        if let Some(after) = &option.savings_opportunity_after_discounts {
            put_savings_after_discounts(
                &mut row,
                SERVICE_OPTION,
                after.savings_opportunity_percentage,
                after
                    .estimated_monthly_savings
                    .as_ref()
                    .map(|savings| (savings.currency.clone().unwrap_or_default(), savings.value)),
            );
        }
        for metric in option.projected_utilization_metrics.iter().flatten() {
            // The projection is a single value computed from the observation, so both reported
            // bounds are that value and the column states it.
            put_ecs_utilization(
                &mut row,
                SERVICE_OPTION_PROJECTED_METRICS,
                metric.name.as_deref(),
                metric.statistic.as_deref(),
                metric.upper_bound_value,
            );
        }
        let containers: Vec<String> = option
            .container_recommendations
            .iter()
            .flatten()
            .map(|container| {
                container_cell(
                    container.container_name.as_deref(),
                    container.cpu,
                    container.memory_size_configuration.as_ref(),
                )
            })
            .collect();
        put_list(
            &mut row,
            "RecommendationOptionsContainerRecommendations",
            &containers,
        );
    }

    // The preferences the recommendation was generated under, as the recommendation itself
    // reports them: the Amazon ECS service field set declares a column for each member an
    // `ECSEffectiveRecommendationPreferences` carries.
    if let Some(preferences) = &recommendation.effective_recommendation_preferences {
        put(
            &mut row,
            "EffectiveRecommendationPreferencesLookBackPeriod",
            text(&preferences.look_back_period),
        );
        put(
            &mut row,
            "EffectiveRecommendationPreferencesSavingsEstimationMode",
            preferences
                .savings_estimation_mode
                .as_ref()
                .map(|mode| text(&mode.source))
                .unwrap_or_default(),
        );
    }
    put_modelled_tags(&mut row, recommendation.tags.as_deref());
    // "The timestamp of when the Amazon ECS service recommendation was last generated": the
    // instant the recommendation reports — the newest observation it was computed from — and
    // otherwise the instant this account's analysis was last refreshed, which is what every
    // other export file of this service states. The instant the recommendation itself carries
    // is stated FROM that value ([`timestamp_of`]), so the cell and the reader's own number
    // denote one instant.
    let refreshed = recommendation
        .last_refresh_timestamp
        .as_ref()
        .map(timestamp_of)
        .or_else(|| last_refresh.map(timestamp));
    if let Some(stated) = refreshed {
        put(&mut row, "LastRefreshTimestamp", stated);
    }
    row
}

/// The `ExportableECSServiceField` prefix of the ONE recommendation option an Amazon ECS
/// service recommendation carries, and of the utilization that option is projected to run at.
const SERVICE_OPTION: &str = "RecommendationOptions";
/// See [`SERVICE_OPTION`].
const SERVICE_OPTION_PROJECTED_METRICS: &str = "RecommendationOptionsProjectedUtilizationMetrics";

/// One observed or projected Amazon ECS service metric, in the column the field set names for
/// that metric AND that statistic — and in no column at all for a statistic it names none for.
///
/// `ExportableECSServiceField` declares one column per analysed metric, naming the statistic it
/// reports that metric under, while the recommendation reports each metric under every
/// statistic it was observed with. So the observation a column holds is the one the column's
/// own name states, and an observation the field set names no column for is reported in none
/// rather than under a name the model does not give it.
fn put_ecs_utilization(
    row: &mut ExportRow,
    prefix: &str,
    name: Option<&str>,
    statistic: Option<&str>,
    value: f64,
) {
    let (Some(name), Some(statistic)) = (name, statistic) else {
        return;
    };
    let field = format!("{prefix}{name}{statistic}");
    if EXPORTABLE_E_C_S_SERVICE_FIELD_VALUES.contains(&field.as_str()) {
        put_number(row, &field, value);
    }
}

/// One container of an Amazon ECS task as a cell: its name and each reservation it carries,
/// under the member names the model gives them, since one exportable field is one column.
///
/// AWS publishes no layout for the export file, so this is the emulator's own. The task
/// definition's containers (`ContainerConfiguration`) and the containers the recommendation
/// option would run (`ContainerRecommendation`) declare the same three members, so both are
/// stated the same way and a client reads the two columns alike.
fn container_cell(
    container_name: Option<&str>,
    cpu: Option<i32>,
    memory: Option<&MemorySizeConfiguration>,
) -> String {
    let mut reservations: Vec<String> = Vec::new();
    if let Some(cpu) = cpu {
        reservations.push(format!("cpu={cpu}"));
    }
    if let Some(memory) = memory {
        if let Some(value) = memory.memory {
            reservations.push(format!("memory={value}"));
        }
        if let Some(value) = memory.memory_reservation {
            reservations.push(format!("memoryReservation={value}"));
        }
    }
    let name = container_name.unwrap_or_default();
    if reservations.is_empty() {
        return name.to_string();
    }
    format!("{name}:{}", reservations.join(";"))
}

/// Write the export objects of every job the clock has already made due, and start this
/// store's clock task for the job the calling export has just recorded: what every
/// `Export*Recommendations` operation does once it has recorded its job, so none of them
/// differs on it.
pub fn deliver_and_schedule<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, now: f64) {
    write_due_exports(&**engine, now);
    drive_export_clock(engine, 2 * super::EXPORT_JOB_STEP);
}

/// Write the export objects of every job the clock has made `Complete` and that has not been
/// written yet.
///
/// What the stored due instant has ALREADY made due, and nothing else: a job still `Queued`
/// or `InProgress` at `now` keeps its objects, so a client reading the destination before the
/// job completes finds nothing there. Writing is idempotent — the objects are TAKEN from the
/// job, so two callers racing write them once.
///
/// A destination that will not take an object (the bucket has been removed since the export
/// was created, say) FAILS the job: "The reason for an export job failure" is what
/// `failureReason` reports, and the job's status is then `Failed` rather than `Complete`.
/// When the object store cannot be reached at all nothing is taken, so the writes stay owed.
pub fn write_due_exports<Eng: Send + Sync + 'static>(engine: &Eng, now: f64) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let Some(objects) = s3::state::service_state(engine) else {
        ::tracing::warn!("the object store is not reachable from this engine, so no \
                          recommendations export was written");
        return;
    };
    // The due objects are taken out of this service's store and the lock released before the
    // object store is locked: the two locks are never held at once.
    let due: Vec<DueExport> = state.lock().export_jobs.take_due_exports(now);
    if due.is_empty() {
        return;
    }

    let mut failures: Vec<(String, String)> = Vec::new();
    {
        let mut store = objects.lock();
        for export in due {
            if let Some(reason) = write_export_objects(&mut store, &export) {
                failures.push((export.job_id, reason));
            }
        }
    }
    if !failures.is_empty() {
        let mut guard = state.lock();
        for (job_id, reason) in failures {
            guard.export_jobs.fail(&job_id, reason);
        }
    }
}

/// Write one job's two objects, answering the reason the destination did not take them.
///
/// Both objects are attempted whatever the other did, so a destination that took one of them
/// keeps it: an export file without its metadata, or the other way round, is what a client
/// reading the bucket then finds beside the `Failed` job the reason is recorded against. The
/// reason names the first object the destination refused.
fn write_export_objects(
    store: &mut s3::state::ComAmazonawsS3Amazons3State,
    export: &DueExport,
) -> Option<String> {
    let bucket = export.bucket.as_str();
    let mut refused: Option<String> = None;
    for (key, body, content_type) in [
        (
            export.key.as_str(),
            export.pending.export_file.as_str(),
            "text/csv",
        ),
        (
            export.metadata_key.as_str(),
            export.pending.metadata_file.as_str(),
            "application/json",
        ),
    ] {
        let object = s3::state::Object::stored(
            body.as_bytes().to_vec(),
            Some(content_type.to_string()),
        );
        let outcome = store.put_object(bucket, key, object);
        if let Some(reason) = refusal_reason(bucket, key, outcome) {
            refused = refused.or(Some(reason));
        }
    }
    refused
}

/// Why the destination did not take the object written to `key`, or `None` when it did.
///
/// Worded from the CONDITION the object store reported rather than from its own types: this
/// is what a client reads as the job's `failureReason`.
fn refusal_reason(
    bucket: &str,
    key: &str,
    outcome: Option<s3::state::ObjectWrite>,
) -> Option<String> {
    use s3::state::ObjectWrite;
    let condition = match outcome {
        Some(ObjectWrite::Created | ObjectWrite::Replaced) => return None,
        // The bucket existed when the export job was created and does not any more.
        None => format!("the destination Amazon S3 bucket {bucket} does not exist"),
        Some(ObjectWrite::KeyTooLong { bytes }) => {
            format!("the object key is {bytes} bytes long, which Amazon S3 does not accept")
        }
        // What is left of the object store's own refusals is the emulator's whole-store
        // memory guard, which is about this process rather than about the destination bucket
        // — so the reason says so instead of claiming a limit of that bucket's.
        Some(ObjectWrite::TotalVersionCountReached) => {
            format!("Amazon S3 cannot hold another object for the export to {bucket}")
        }
        Some(ObjectWrite::TotalByteBudgetReached { .. }) => {
            format!("Amazon S3 cannot hold the bytes of the export to {bucket}")
        }
        Some(ObjectWrite::MetadataTooLarge { bytes }) => {
            format!("the object's metadata is {bytes} bytes, which Amazon S3 does not accept")
        }
    };
    Some(format!(
        "Compute Optimizer could not write the recommendations export to {key} because \
         {condition}."
    ))
}

/// Start the one short in-process task that writes this job's objects when the clock makes
/// the job complete.
///
/// A client that exports and then downloads the file makes no further Compute Optimizer call,
/// so a write performed only by the next call could be a write never performed. The task does
/// not change WHEN the objects appear: it only calls [`write_due_exports`], which applies
/// what the stored due instant has already made due, so a client reading the destination
/// before that instant still finds nothing. With no async runtime to spawn onto there is no
/// task, and the objects are written by the next operation instead.
pub fn drive_export_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, due_in: ::std::time::Duration) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let engine = Arc::clone(engine);
    runtime.spawn(async move {
        ::tokio::time::sleep(due_in).await;
        write_due_exports(&*engine, super::now_secs());
    });
}
