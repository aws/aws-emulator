// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemaker-sagemaker service.
//!
//! The inference-deployment lifecycle (model -> endpoint config -> endpoint), the
//! hyperparameter tuning jobs, the pipeline lifecycle, the model-card governance
//! resources, batch-transform jobs and the AI benchmarking control plane's workload
//! configurations and benchmark jobs are kept here, in
//! `BTreeMap`s keyed by the resource name AWS keys them by, so `ListModels` has one
//! stable order to sort and paginate over and every `Describe*` replays exactly what
//! the matching `Create*` stored. The Studio AppImageConfig registry, the Git
//! repositories `CreateCodeRepository` registers and the processing jobs
//! `CreateProcessingJob` accepts are kept the same way, keyed by `AppImageConfigName`,
//! `CodeRepositoryName` and `ProcessingJobName`, and so is the model-package-group
//! registry.
//!
//! The lineage entities (context, artifact) are keyed by their ARN instead: SageMaker
//! documents their names as unique to an account IN A REGION (and an artifact need not
//! have a name at all), so the ARN — which carries the Region and the account beside the
//! name — is what identifies one of them.
//!
//! The lineage groups the account's lineage entities belong to are kept the same way,
//! keyed by `(Region, LineageGroupName)` because a lineage group is a REGIONAL resource —
//! with no `Create*` of their own, because SageMaker models none: see
//! [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`]. `DescribeLineageGroup`
//! reports one of them and `ListLineageGroups` is a pure read over the Region's.
//!
//! Studio apps live here too, keyed by the [`AppKey`] their five input members name
//! rather than by a bare name, because that tuple — domain, owning user profile or
//! space, app type, app name — is what identifies one app. The ML Lineage entities
//! (artifacts, actions, trial components) are keyed by the ARN or name their own slices
//! document, and the associations BETWEEN them by the `(SourceArn, DestinationArn)` pair
//! that is one edge's whole identity.
//!
//! # A job's status is moved by the CLOCK, never by a read
//!
//! A resource AWS returns in a non-terminal status (an endpoint in `Creating`, a
//! processing job in `InProgress`) owes its next move to the clock: the instant it falls
//! due is stored at create time and the reporting operations only materialise what is
//! already due ([`Endpoint::settle_status`], [`ProcessingJob::settle_status`]), so a read
//! never changes what a later caller sees and the transient status is observable because
//! the span is never zero.
//!
//! # An entity's own collections are ALWAYS reported, and empty when it holds nothing
//!
//! The ML Lineage and experiment-tracking readers report every collection an entity owns
//! — a lineage entity's properties, a trial component's parameters and artifacts, the
//! source types of an artifact's source, the metrics and sources nothing has produced for
//! a component — whatever the creating request carried: an entity created with none of
//! them reports the EMPTY map or list, not a missing member, because the member is how the
//! entity reports what it holds rather than what the request mentioned. So the records
//! below hold those collections directly rather than as an `Option` of one, and the write
//! path resolves an absent request member to the empty collection ONCE, where the record
//! is written, instead of leaving each reader to decide again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

/// The AutoML job V2 slice: the record, its clock and its constants. Re-exported below,
/// so a handler names them as `crate::state::…` like every other slice.
pub mod auto_ml_job_v2;
pub use auto_ml_job_v2::*;

/// The AutoML job V2's own RUN: the dataset it analyses, the candidates it fits and
/// measures, the artifacts it writes. See `state/auto_ml_run.rs`.
pub mod auto_ml_run;
pub use auto_ml_run::*;

/// SERVING an inference from a model this emulator's own training produced: the variant an
/// invocation routes to, the artifact behind it, and the prediction it computes. See
/// `state/model_serving.rs`.
pub mod model_serving;
pub use model_serving::*;

use crate::ops::types::{
    AdditionalInferenceSpecificationDefinition, AppSpecification, ArtifactSource,
    AsyncInferenceConfig, AutoMLComputeConfig, AutoMLDataSplitConfig, AutoMLJobChannel,
    AutoMLJobObjective, AutoMLOutputDataConfig, AutoMLProblemTypeConfig, AutoMLSecurityConfig,
    Autotune, BatchDataCaptureConfig, CodeEditorAppImageConfig, ContainerDefinition,
    DataCaptureConfig, DataProcessing, DriftCheckBaselines, ExperimentConfig, ExplainerConfig,
    GitConfig, HyperParameterTrainingJobDefinition, HyperParameterTuningJobConfig,
    HyperParameterTuningJobWarmStartConfig, IamIdentity, InferenceExecutionConfig,
    InferenceSpecification, JupyterLabAppImageConfig, KernelGatewayImageConfig,
    ManagedConfiguration, MetadataProperties, MetricsConfig, ModelCardExportOutputConfig,
    ModelCardSecurityConfig, ModelClientConfig, ModelDeployConfig, ModelLifeCycle, ModelMetrics,
    ModelPackageModelCard, ModelPackageSecurityConfig, ModelPackageContainerDefinition,
    ModelPackageStatusDetails,
    ModelPackageStatusItem, ModelPackageValidationSpecification, NetworkConfig,
    ParallelismConfiguration, Parameter, ProcessingInput, ProcessingOutputConfig,
    ProcessingResources, ProcessingStoppingCondition, ProductionVariant, ProductionVariantSummary,
    RecommendationJobInputConfig, RecommendationJobOutputConfig,
    RecommendationJobStoppingConditions, ResourceSpec, SecondaryStatusTransition,
    SelectiveExecutionConfig,
    SourceAlgorithmSpecification, Tag, TrainingSpecification, TransformInput, TransformOutput,
    TransformResources,
    UserContext, VpcConfig,
};

/// The AI recommendation job slice — the record `CreateAIRecommendationJob` writes,
/// `DescribeAIRecommendationJob` replays and `DeleteAIRecommendationJob` releases, with
/// the ARN kind, quota, statuses, clock span and failure reason that belong to it. Kept
/// in its own file (as the CloudFormation service keeps `state::template`) so this one
/// resource kind reads as one unit instead of another block at the end of this module.
pub mod ai_recommendation_job;

pub use ai_recommendation_job::{
    ai_recommendation_job_not_deletable, ai_recommendation_job_not_found,
    ai_recommendation_job_not_running, AIRecommendationJob, AI_RECOMMENDATION_JOB_ENDS_AFTER,
    AI_RECOMMENDATION_JOB_ENTITY, AI_RECOMMENDATION_JOB_KIND, AI_RECOMMENDATION_JOB_QUOTA,
    AI_RECOMMENDATION_JOB_STOPS_AFTER, AI_RECOMMENDATION_OPTIMIZE_MODEL_DEFAULT,
    AI_RECOMMENDATION_STATE_PENDING, AI_RECOMMENDATION_STATUS_FAILED,
    AI_RECOMMENDATION_STATUS_IN_PROGRESS, AI_RECOMMENDATION_STATUS_STOPPED,
    AI_RECOMMENDATION_STATUS_STOPPING, NO_RECOMMENDATION_RUNTIME_REASON,
};

/// The model-customization job slice: the record `CreateJob` persists, the lookup every
/// job operation performs and the constants that bound and word it. Re-exported below,
/// so a handler names them as `crate::state::…` like every other slice.
pub mod job;
pub use job::{
    Job, JOB_ARN_KIND, JOB_KIND_LABEL, JOB_QUOTA, JOB_SECONDARY_STATUS_STARTING,
    JOB_STATUS_IN_PROGRESS,
};

/// The job configuration SCHEMA slice: the one schema `DescribeJobSchemaVersion` reports
/// and `CreateJob` admits a `JobConfigDocument` against. Re-exported below for the same
/// reason as the slice above.
pub mod job_config_schema;
pub use job_config_schema::{
    document_refusal as job_config_document_refusal, schema_document as job_config_schema_document,
    SCHEMA_VERSION as JOB_CONFIG_SCHEMA_VERSION,
};

/// The ADMISSION slice: the questions every create answers about the execution role and
/// the network placement a request names, and the sentences it refuses each of them with.
/// Re-exported below, so a handler names them as `crate::state::…` like every other slice.
pub mod admission;
pub use admission::{
    could_not_access_model_data, could_not_assume_role, could_not_find_model_data,
    execution_role_failure_reason, execution_role_is_unassumable, no_s3_objects_found,
    vpc_placement_refusal, SECURITY_GROUPS_NOT_SPECIFIED, SERVICE_PRINCIPAL,
};

/// The Amazon S3 ACCESS slice: the one reading of an Amazon S3 location a request names and
/// the one question a create asks about it — can the execution role read what it names?
/// Re-exported below, so a handler names them as `crate::state::…` like every other slice.
pub mod s3_access;
pub use s3_access::{
    arn_partition, first_unreadable_location, s3_client, S3Location, UnreadableLocation,
    DEFAULT_PARTITION, GET_OBJECT_ACTION, LIST_BUCKET_ACTION, PUT_OBJECT_ACTION,
};

/// The CONTAINER IMAGE slice: the one reading of a container image path a request names, and
/// the sentence a path that is no reference at all is refused with. Re-exported below, so a
/// handler names them as `crate::state::…` like every other slice.
pub mod container_image;
pub use container_image::{
    first_image_outside_the_documented_formats, first_malformed_container_image,
    image_names_no_registry, malformed_container_image, INVALID_IMAGE_PATH,
};

/// The cluster-policy slice of this state: HyperPod clusters and the cluster policies
/// on them, with their own records, quota, status span and id mint. Reached through the
/// single [`ComAmazonawsSagemakerSagemakerState::cluster_policies`] field.
pub mod cluster_policy;

/// The HyperPod cluster slice: the [`Cluster`] record `CreateCluster` writes with its
/// quota, status span and id mint, the one lookup that resolves the `ClusterNameOrArn`
/// every cluster operation takes, and the `*Details` / `*Output` projections a reader
/// builds from the stored record.
pub mod cluster;
pub use cluster::*;

/// The TRAINING CAPACITY slice: what a `ResourceConfig`'s ML compute has to be for
/// SageMaker to launch it, and the account quota it is measured against. Re-exported
/// below, so a handler names them as `crate::state::…` like every other slice.
pub mod training_capacity;
pub use training_capacity::*;

/// The RESOURCE POLICY slice: what makes a document a resource policy this service will
/// attach, and the sentence each condition is refused with. Re-exported below, so a
/// handler names them as `crate::state::…` like every other slice.
pub mod resource_policy;
pub use resource_policy::*;

/// The ARN service namespace (`arnNamespace` in the model's `aws.api#service` trait,
/// also SageMaker's sigv4 signing name).
pub const ARN_SERVICE: &str = "sagemaker";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. It matches the region the emulator's generated shared config
/// hands the AWS CLI.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateEndpoint` the clock owes the `Creating` -> `InService`
/// transition. A caller that describes the endpoint straight after creating it still
/// observes `Creating`; one that waits (as `aws sagemaker wait endpoint-in-service`
/// does) observes `InService`, so a waiter terminates. Real SageMaker takes minutes;
/// the span is compressed, never zero.
pub const ENDPOINT_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// How long after `CreateMlflowApp` the clock owes the `Creating` -> `Created`
/// transition of `MlflowAppStatus`.
///
/// A caller that describes the app straight after creating it still observes
/// `Creating`; one that waits observes `Created`. Real SageMaker takes minutes to stand
/// an MLflow app up; the span is compressed, never zero, and the due instant is stored
/// rather than slept on.
pub const MLFLOW_APP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// How long after `CreateApp` the clock owes the `Pending` -> `InService` transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the app
/// straight after creating it still observes `Pending`, one that waits observes
/// `InService`. Real Studio app launches take minutes; the span is compressed, never
/// zero.
pub const APP_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// The most records each of this service's stores holds, after which a create is
/// refused with the modelled `ResourceLimitExceeded`.
///
/// SageMaker publishes adjustable per-account quotas on the resources this service
/// owns, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on each store rather than
/// a claim about any account's limit. It sits far above any workflow an emulator serves,
/// and it is what stops the state growing without end: every record it holds is released
/// by the matching `Delete*`.
pub const MODEL_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const ENDPOINT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. A transform job is terminal and has no delete, so this is the
/// most transform jobs one emulator run holds.
pub const TRANSFORM_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. `CreateMlflowApp` models `ResourceLimitExceeded` for exactly
/// this refusal.
pub const MLFLOW_APP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Counts model packages of both kinds (unversioned packages and
/// the versions registered into model package groups), which is what `CreateModelPackage`
/// mints and `DeleteModelPackage` releases.
pub const MODEL_PACKAGE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const TRIAL_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the compute-allocation store, and is what
/// `CreateComputeQuota` answers `ResourceLimitExceeded` past.
pub const COMPUTE_QUOTA_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const PIPELINE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const APP_IMAGE_CONFIG_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const CODE_REPOSITORY_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const APP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const MODEL_PACKAGE_GROUP_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`] — this one is per cluster, the resource that holds the nodes.
pub const CLUSTER_NODE_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the lineage-artifact store the same way, and is what
/// `CreateArtifact` answers `ResourceLimitExceeded` past.
pub const ARTIFACT_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the model-card store, and is what `CreateModelCard`
/// answers `ResourceLimitExceeded` past.
pub const MODEL_CARD_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds how many versions ONE model card can hold, and is what
/// `UpdateModelCard` answers `ResourceLimitExceeded` past: every content update appends
/// a version, so without a bound one card could grow the store without end.
pub const MODEL_CARD_VERSION_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the model-card export-job store, and is what
/// `CreateModelCardExportJob` answers `ResourceLimitExceeded` past: an export job is
/// terminal and has no delete, so this is the most one emulator run can start.
pub const MODEL_CARD_EXPORT_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`].
pub const PROCESSING_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. `DeleteTrainingJob` is still a stub, so no call releases a
/// training-job record yet; the bound is what keeps the store finite in the meantime.
pub const TRAINING_JOB_QUOTA: usize = 1_000;
/// See [`MODEL_QUOTA`]. Bounds the lineage-group store as a whole, across Regions: a
/// group is materialised per Region, so this is the total the store holds rather than a
/// per-Region count. Declared last in this block, with the lineage-group record and its
/// two accessors at the END of this file, so a service growing another store does not
/// collide with the lineage-group one.
pub const LINEAGE_GROUP_QUOTA: usize = 1_000;

/// The most `BatchAddClusterNodes` client tokens one cluster remembers. See
/// [`Cluster::remember_node_addition`].
pub const NODE_ADDITION_HISTORY_CAP: usize = 64;

/// The most presigned-URL grants one partner app keeps, so a caller asking for URL after
/// URL cannot grow the store without end; a grant past the cap is forgotten, which a
/// caller cannot tell from the expiry it was minted with. Beside the other store bounds,
/// next to the [`PartnerApp`] it bounds.
pub const GRANT_HISTORY_CAP: usize = 32;

/// How long a presigned partner-app URL stays valid when the request names no
/// `ExpiresInSeconds`.
///
/// The emulator's OWN choice, not a published AWS default: the model documents none. The
/// widest value `ExpiresInSeconds` allows (`@range 5..300`), so a caller that names no
/// value gets a link a human has time to follow. Beside [`GRANT_HISTORY_CAP`], with the
/// other knobs of the [`PartnerApp`] record they govern.
pub const DEFAULT_PRESIGNED_URL_EXPIRES_IN_SECONDS: i32 = 300;

/// How long the session a presigned partner-app URL opens lasts when the request names
/// no `SessionExpirationDurationInSeconds`.
///
/// The emulator's OWN choice, not a published AWS default: the narrowest value the model
/// allows (`@range 1800..43200`), so an unnamed session is the shortest one rather than
/// the longest.
pub const DEFAULT_PRESIGNED_URL_SESSION_SECONDS: i32 = 1800;

/// The name of the lineage group every lineage entity of this account in `region`
/// belongs to.
///
/// SageMaker ML Lineage Tracking gives an ACCOUNT one lineage group per Region, with no
/// call creating it, and reports its ARN as the `LineageGroupArn` of each entity in it.
/// That group carries SageMaker's own fixed name, [`DEFAULT_LINEAGE_GROUP_NAME`], the
/// same in every Region: what makes two Regions' groups two groups is the Region their
/// ARN names, not their name. The `region` argument is taken so every caller reads the
/// group of the Region it is answering for through one function, whether or not the name
/// itself varies.
///
/// This is also the group the lineage-entity creates materialise through
/// [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`], so that
/// `DescribeLineageGroup` reports the group those entities are in and
/// `ListLineageGroups` lists it.
pub fn default_lineage_group_name(_region: &str) -> String {
    DEFAULT_LINEAGE_GROUP_NAME.to_string()
}

/// The name SageMaker gives the lineage group it creates for an account, which every
/// lineage entity belongs to unless another group is named for it.
///
/// Recorded behavior for the text, and it is FIXED: the group is SageMaker's own, so its
/// name carries nothing of the account or of the Region — the Region is what the group's
/// ARN names ([`lineage_group_arn`]). The form satisfies
/// `ExperimentEntityName` (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`), so the group is
/// addressable by name wherever a reader takes one.
pub const DEFAULT_LINEAGE_GROUP_NAME: &str = "sagemaker-default-lineage-group";

/// See [`MODEL_QUOTA`]. `DeleteHyperParameterTuningJob` is still a stub, so no call
/// releases a tuning-job record yet; the bound is what keeps the store finite in the
/// meantime.
pub const HYPER_PARAMETER_TUNING_JOB_QUOTA: usize = 1_000;

/// The most training jobs one hyperparameter tuning job may be asked to launch
/// (`ResourceLimits$MaxNumberOfTrainingJobs`).
///
/// SageMaker publishes this as an adjustable account quota — named by
/// [`TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME`] — and refuses a larger request with the
/// modelled `ResourceLimitExceeded`. This emulator models neither Service Quotas nor an
/// account's adjustments, so every account is held to the published default.
pub const MAX_TRAINING_JOBS_PER_TUNING_JOB: i32 = 750;

/// The most training jobs one hyperparameter tuning job may run at a time
/// (`ResourceLimits$MaxParallelTrainingJobs`), from the published adjustable quota
/// [`PARALLEL_TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME`]. See
/// [`MAX_TRAINING_JOBS_PER_TUNING_JOB`] for why the default is what every account gets.
pub const MAX_PARALLEL_TRAINING_JOBS_PER_TUNING_JOB: i32 = 10;

/// The published name of the quota [`MAX_TRAINING_JOBS_PER_TUNING_JOB`] is the default
/// of, as a refusal states it. INFERRED from the sibling
/// [`PARALLEL_TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME`], which is the one of the pair a
/// recorded refusal spells out: the two dimensions are told apart by name, so this one is
/// worded in that family's shape. A later recording of the total-jobs refusal is what
/// would settle it.
pub const TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME: &str =
    "Maximum number of training jobs each hyper parameter tuning job can run";
/// The published name of the quota [`MAX_PARALLEL_TRAINING_JOBS_PER_TUNING_JOB`] is the
/// default of, as a refusal states it.
pub const PARALLEL_TRAINING_JOBS_PER_TUNING_JOB_QUOTA_NAME: &str =
    "Maximum number of training jobs each hyper parameter tuning job can run in parallel \
     at once";

/// The unit [`account_quota_exceeded`] counts a training-job quota in: what the limit,
/// the utilisation and the delta of such a quota are numbers OF.
pub const TRAINING_JOBS_UNIT: &str = "TrainingJobs";

/// How SageMaker words a refusal of a request that is past an adjustable account quota
/// AWS PUBLISHES: the quota by its published name, its limit, what the account is already
/// using of it and what this request asks for on top — each count followed by the UNIT
/// the quota is counted in — and then how to ask for more.
///
/// The four numbers are the point of the template: a client reads the limit against the
/// utilisation to decide whether to ask for less or to request an increase, so none of
/// them is left out and none is replaced by prose about one member of the request.
/// Stated once here so every operation that evaluates a published account quota words it
/// the same way.
///
/// This is NOT the wording for the emulator's own bound on a store (see [`MODEL_QUOTA`]):
/// that bound is not a published quota, so a refusal past it states the bound in its own
/// words rather than claiming Service Quotas can raise it.
pub fn account_quota_exceeded(
    quota: &str,
    unit: &str,
    limit: i64,
    in_use: i64,
    delta: i64,
) -> String {
    format!(
        "The account-level service limit '{quota}' is {limit} {unit}, with current \
         utilization of {in_use} {unit} and a request delta of {delta} {unit}. Please use \
         AWS Service Quotas to request an increase for this quota. If AWS Service Quotas \
         is not available, contact AWS support to request an increase for this quota."
    )
}

/// The most hyperparameters one tuning job may search over, across
/// `ParameterRanges`' integer, continuous and categorical lists together.
///
/// The model bounds each of those lists at 30 entries and `ParameterRanges`' own doc
/// says that figure "refers to the maximum number of hyperparameters for each range and
/// also the maximum for the hyperparameter tuning job itself" — the sum is held to the
/// same bound the model states per list.
pub const MAX_SEARCHED_HYPERPARAMETERS: usize = 30;

/// How long after `CreateHyperParameterTuningJob` the clock owes the tuning job's
/// `InProgress` -> terminal transition.
///
/// A caller that describes the job straight after creating it still observes
/// `InProgress`; one that waits observes the terminal status, so a waiter terminates.
/// Real tuning jobs take hours; the span is compressed, never zero, and sits at the top
/// of the band this emulator uses for a clock-owed transition so a caller (or a test)
/// reading the transient status right after the create is not racing the clock.
pub const TUNING_JOB_SETTLES_AFTER: Duration = Duration::from_millis(700);

/// `HyperParameterTuningJobStatus`'s members this emulator moves a tuning job through.
pub const TUNING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TUNING_STATUS_IN_PROGRESS`].
pub const TUNING_STATUS_FAILED: &str = "Failed";
/// See [`TUNING_STATUS_IN_PROGRESS`]: the transient status
/// `StopHyperParameterTuningJob` moves a running job into.
pub const TUNING_STATUS_STOPPING: &str = "Stopping";
/// See [`TUNING_STATUS_IN_PROGRESS`]: the terminal status the clock owes a job that was
/// asked to stop, after which "it releases all reserved resources for the tuning job"
/// (`StopHyperParameterTuningJob`'s own doc).
pub const TUNING_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopHyperParameterTuningJob` the clock owes the job's `Stopping` ->
/// `Stopped` transition.
///
/// Same contract as [`TUNING_JOB_SETTLES_AFTER`] and chosen on the same terms (AWS
/// documents no duration for the stop): never zero, so a caller that describes the job
/// straight after asking it to stop still observes `Stopping`, and one that waits
/// observes `Stopped`.
pub const TUNING_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// Why a hyperparameter tuning job this emulator accepted ends in `Failed`.
///
/// A tuning job's work is the training jobs it launches, and this emulator runs no
/// training container: nothing evaluates an objective metric and nothing writes model
/// artifacts. Reporting `Completed`, a best training job or a metric value would claim
/// results of work that never ran, so the job ends in the modelled failure status
/// carrying this reason instead.
pub const NO_TRAINING_RUNNER_FAILURE_REASON: &str =
    "This emulator does not run training jobs: no training job was launched for this \
hyperparameter tuning job, so no objective metric was evaluated and no model artifacts were \
written to the output S3 location.";

/// How long after `CreateProcessingJob` the clock owes the job's move out of
/// `InProgress` into its terminal status.
///
/// A span chosen by this emulator (AWS documents no duration for a processing run),
/// inside the 250–750 ms band the project fixes for a transition the clock owes. It is
/// never zero, so a caller that describes the job straight after creating it still
/// observes `InProgress`, and one that waits (as `aws sagemaker wait
/// processing-job-completed` does) observes the terminal status, so a waiter terminates.
pub const PROCESSING_JOB_ENDS_AFTER: Duration = Duration::from_millis(400);

/// `ProcessingJobStatus`'s members this emulator moves a processing job through (the
/// modelled enum also carries `Completed`, which this slice does not reach because
/// nothing runs the job's container image). Named once here so no handler compares a
/// bare string literal.
pub const PROCESSING_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`PROCESSING_STATUS_IN_PROGRESS`].
pub const PROCESSING_STATUS_FAILED: &str = "Failed";
/// See [`PROCESSING_STATUS_IN_PROGRESS`]: the transient status a job carries from the
/// `StopProcessingJob` call until the clock owes it [`PROCESSING_STATUS_STOPPED`].
pub const PROCESSING_STATUS_STOPPING: &str = "Stopping";
/// See [`PROCESSING_STATUS_STOPPING`]: the terminal status of a job the caller stopped.
pub const PROCESSING_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopProcessingJob` the clock owes the job's `Stopping` -> `Stopped`
/// move.
///
/// Chosen on the same terms as [`PROCESSING_JOB_ENDS_AFTER`] (AWS documents no duration
/// for the stop either) and inside the same band: never zero, so a caller that describes
/// the job straight after asking it to stop still observes the transient `Stopping`, and
/// one that waits observes `Stopped`, so a waiter terminates.
pub const PROCESSING_JOB_STOPS_AFTER: Duration = Duration::from_millis(300);

/// How the processing-job API words a `ProcessingJobName` that resolves to no job for a
/// call that only REPORTS the job: fixed text, then the name the request presented.
///
/// Recorded wire behaviour: the service answers this one with the error code
/// `ValidationException` and HTTP 400, not with the `ResourceNotFound` the model lists, so
/// the handler returns it through the validation channel. Named once here so the reader
/// cannot word the fact differently from the template below.
pub fn processing_job_not_found_for_read(name: &str) -> String {
    format!("Could not find requested job with name: {name}")
}

/// How the processing-job API words a `ProcessingJobName` that resolves to no job for a
/// call that would CHANGE the job — the stop: fixed text, then the name the request
/// presented.
///
/// A separate template from [`processing_job_not_found_for_read`] because the service
/// words the two differently, and carried on the same `ValidationException` / HTTP 400
/// envelope for the same reason.
pub fn processing_job_not_found_for_update(name: &str) -> String {
    format!("Could not find job to update with name: {name}")
}

/// Why stopping a job whose run is already over is refused, naming the job and the status
/// it had already reached.
///
/// A job in a status nothing leaves on its own has no run left to stop.
/// `StopProcessingJob` models only `ResourceNotFound` and the job IS found, so the
/// refusal is a 400 `ValidationException` carrying this sentence.
pub fn processing_job_not_running(name: &str, status: &str) -> String {
    format!(
        "The processing job \"{name}\" is {status} and is not running, so there is \
         nothing to stop."
    )
}

/// `FailureReason` of a processing job whose container image could not be run here.
///
/// A processing job runs a customer Docker image (`AppSpecification$ImageUri`). This
/// emulator has no container runtime, and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids reporting a status, duration or exit code for
/// work nothing ran — so the run the caller asked for ends in the modelled `Failed`
/// state with this reason, and no `ProcessingStartTime` or `ExitMessage` is invented.
/// Everything around the run — accepting the job, validating it, minting its ARN,
/// replaying it, reporting its terminal status — is real.
pub const NO_CONTAINER_RUNTIME_REASON: &str =
    "This emulator has no container runtime, so the processing job's container image was \
     never started";

/// The instance count a processing cluster runs when the request names none, per
/// `ProcessingClusterConfig$InstanceCount`'s doc: "The default value is 1."
pub const DEFAULT_PROCESSING_INSTANCE_COUNT: i32 = 1;

/// `PipelineStatus`'s members (`smithy_query(query="shape",shape="PipelineStatus")`
/// answers exactly `Active` and `Deleting`). Named once here so no handler compares a
/// bare string literal.
///
/// Only `Active` is ever reported: recorded AWS makes a pipeline unreadable from the
/// `DeletePipeline` call on, so no reader observes a pipeline being deleted and there is
/// no transient status for `DeletePipeline` to leave behind.
pub const PIPELINE_STATUS_ACTIVE: &str = "Active";

/// The one pipeline-definition schema version SageMaker Pipelines publishes, and the
/// value a definition's required `Version` key carries.
pub const PIPELINE_DEFINITION_VERSION: &str = "2020-12-01";

/// The `Type` of the one step kind that holds steps of its own, in the branches the
/// schema gives it (`IfSteps` / `ElseSteps`). Every other step's `Arguments` are its own
/// payload, so nothing inside them is walked as a pipeline step.
const PIPELINE_CONDITION_STEP_TYPE: &str = "Condition";

/// The `Type` of the step kind that ENDS a run: it stops the pipeline with its own error
/// message, so nothing follows it.
const PIPELINE_FAIL_STEP_TYPE: &str = "Fail";

/// The step `Type`s no `DependsOn` entry may name as its target.
///
/// A `DependsOn` edge waits for the target step to COMPLETE, and neither of these two
/// kinds completes into something a successor can run after: a fail step ends the run,
/// and a condition step hands control to one of its own branches rather than to the
/// steps declared beside it. Each is refused by name, so a caller is told which kind it
/// tried to wait on.
const PIPELINE_UNDEPENDABLE_STEP_TYPES: [&str; 2] =
    [PIPELINE_FAIL_STEP_TYPE, PIPELINE_CONDITION_STEP_TYPE];

/// The key a pipeline definition writes a property reference under: the schema's
/// `{"Get": "<reference>"}` form, which is how a step's arguments name a value resolved
/// when the pipeline runs.
const PIPELINE_PROPERTY_REFERENCE_KEY: &str = "Get";

/// The property-reference prefix that addresses one of the definition's own parameters,
/// i.e. a reference of the form `Parameters.<name>`, where `<name>` is a name the
/// definition's top-level `Parameters` list declares.
const PIPELINE_PARAMETER_REFERENCE_PREFIX: &str = "Parameters.";

/// The version id a pipeline carries from the moment it is created. `UpdatePipeline`
/// mints later ones; nothing here does yet, so a created pipeline has exactly this one.
pub const PIPELINE_INITIAL_VERSION: i64 = 1;

/// `PipelineExecutionStatus`'s members (`smithy_query(query="shape",
/// shape="PipelineExecutionStatus")` answers `Executing`, `Stopping`, `Stopped`,
/// `Failed`, `Succeeded` and `CompletedWithFailures`). Named here so no handler compares
/// a bare string literal; the two an execution this emulator starts ever reports are
/// the non-terminal one it begins in and the terminal one [`PIPELINE_EXECUTION_NOT_RUN`]
/// explains.
pub const PIPELINE_EXECUTION_STATUS_EXECUTING: &str = "Executing";
/// See [`PIPELINE_EXECUTION_STATUS_EXECUTING`].
pub const PIPELINE_EXECUTION_STATUS_FAILED: &str = "Failed";

/// The most steps one selective run may name, and the least.
///
/// `SelectedStepList` is modelled `len 1..50`
/// (`smithy_query(query="shape",shape="SelectiveExecutionConfig")`), so a longer or an
/// empty list is outside the model. The bound is restated here because it is what keeps
/// the step-membership check, and the size of the record the run persists, proportional
/// to the model rather than to the request's own length.
pub const MIN_SELECTED_STEPS: usize = 1;
/// See [`MIN_SELECTED_STEPS`].
pub const MAX_SELECTED_STEPS: usize = 50;

/// How long after `StartPipelineExecution` the clock owes the execution's terminal
/// status.
///
/// Same contract as [`PROCESSING_JOB_ENDS_AFTER`]: a caller that describes the execution
/// straight after starting it still observes `Executing`, one that waits observes the
/// terminal status. The span is the emulator's own, compressed and never zero, and the
/// due instant is stored rather than slept on.
pub const PIPELINE_EXECUTION_ENDS_AFTER: Duration = Duration::from_millis(400);

/// Why an execution this emulator started ends [`PIPELINE_EXECUTION_STATUS_FAILED`].
///
/// A pipeline's steps are training, processing, transform, Lambda and callback work run
/// outside SageMaker's control plane, and this emulator has no runtime that performs any
/// of it. So an accepted execution is never reported `Succeeded`: nothing ran its steps,
/// and a success status, a job ARN, a step output or a runtime for work nothing performed
/// would be fabricated. The execution instead ends in the modelled failure state with
/// this reason, which says exactly that, and the steps the run was to perform report the
/// same failure state and the same reason — see [`step_status_of_execution`] and
/// [`pipeline_definition_step_declarations`], which is what
/// `ListPipelineExecutionSteps` reports them from. Within the modelled
/// `PipelineExecutionFailureReason` length (0..1300), and within the shorter
/// `FailureReason` (0..1024) a step execution reports it under.
pub const PIPELINE_EXECUTION_NOT_RUN: &str =
    "The pipeline execution was accepted but no step of it ran: this emulator has no \
     runtime that performs a pipeline step's work (training, processing, transform, \
     Lambda or callback), so the execution ended Failed rather than reporting work that \
     did not happen.";

/// The `StepStatus` the steps of an execution in `execution_status` report, or `None`
/// while that execution has reached no step at all.
///
/// A step execution belongs to the run that performs it, so its status follows the run's:
/// `PipelineExecutionStatus` and `StepStatus` share every member
/// (`smithy_query(query="shape",shape="StepStatus")` answers `Starting`, `Executing`,
/// `Stopping`, `Stopped`, `Failed`, `Succeeded`) but `CompletedWithFailures`, which names
/// no step's own outcome. An execution still [`PIPELINE_EXECUTION_STATUS_EXECUTING`] has
/// reached no step yet — the clock has not settled it, so nothing about any step of it is
/// known — and a status with no `StepStatus` member of its own reports no step rather
/// than one in a status the model does not name.
pub fn step_status_of_execution(execution_status: &str) -> Option<&'static str> {
    if execution_status == PIPELINE_EXECUTION_STATUS_EXECUTING {
        return None;
    }
    crate::ops::types::STEP_STATUS_VALUES
        .iter()
        .copied()
        .find(|member| *member == execution_status)
}

/// See [`MODEL_QUOTA`]. Bounds the pipeline-execution store, so a caller starting
/// executions cannot grow it without end; `StartPipelineExecution` refuses past it with
/// the modelled `ResourceLimitExceeded`.
pub const PIPELINE_EXECUTION_QUOTA: usize = 1_000;

/// How deeply a pipeline definition's steps may nest (a condition step's branches hold
/// steps, which may themselves be condition steps). A bound, so a definition cannot
/// drive the step walk below into unbounded recursion.
const PIPELINE_MAX_STEP_DEPTH: usize = 32;

/// The most steps one pipeline definition may declare, counting the steps nested in a
/// condition step's branches, and the most `DependsOn` edges those steps may declare
/// between them.
///
/// `PipelineDefinition` is modelled `len 1..1048576`, and a megabyte of JSON can declare
/// tens of thousands of steps and edges — the two factors the dependency walk's work is
/// the product of. These are the emulator's OWN bounds on one definition rather than a
/// claim about any account's published quota: each sits far above any pipeline an
/// emulator serves, and together they are what keeps validating a definition short and
/// bounded however large the document is. A definition past either bound is refused
/// before any graph work is done.
const PIPELINE_MAX_STEPS: usize = 512;
/// See [`PIPELINE_MAX_STEPS`].
const PIPELINE_MAX_STEP_DEPENDENCIES: usize = 4_096;

/// [`PIPELINE_MAX_STEPS`] and [`PIPELINE_MAX_STEP_DEPENDENCIES`], so a test can pin the
/// bound at the value this module enforces rather than repeating a literal.
pub const fn pipeline_definition_bounds() -> (usize, usize) {
    (PIPELINE_MAX_STEPS, PIPELINE_MAX_STEP_DEPENDENCIES)
}

/// The most records each of this service's paginated listings returns in one page,
/// including when `MaxResults` is absent or larger.
///
/// `MaxResults` is modelled `range 1..100` on these operations — `ListModels` documents
/// no default and `ListLineageGroups` documents 10 — so this is the service's own
/// server-side ceiling on top of whichever default applies: one call can never serialise
/// the whole store, and a caller that wants the rest follows `NextToken`. Matches the
/// ceiling the other paginated services in this workspace apply.
pub const MAX_PAGE_SIZE: i32 = 50;

/// `SortBy`'s documented default on this service's listings ("Sorts the list of results.
/// The default is `CreationTime`." — the member doc on both `ListModels` and
/// `ListEndpoints`).
pub const DEFAULT_SORT_BY: &str = "CreationTime";

/// `SortOrder`'s documented default ("The sort order for results. The default is
/// `Descending`.").
pub const DEFAULT_SORT_ORDER: &str = ORDER_DESCENDING;

/// `OrderKey`'s descending member, named once here so no handler decides a sort
/// direction by comparing a bare string literal, nor by assuming the documented default
/// is the descending one.
pub const ORDER_DESCENDING: &str = "Descending";

/// `OrderKey`'s ascending member — the documented default of the listings whose own
/// member doc says so (`ListClusters`, `ListCodeRepositories`), as opposed to
/// [`DEFAULT_SORT_ORDER`].
pub const ORDER_ASCENDING: &str = "Ascending";

/// A page boundary as an opaque `NextToken`: the hex of the resource name the page
/// stopped at.
///
/// The token names the item the page stopped at rather than an offset, so resuming lands
/// exactly after it even though the token is opaque to the caller. Stated once here so
/// every paginated listing in this service mints and reads one token format.
pub fn encode_page_token(resource_name: &str) -> String {
    hex::encode(resource_name.as_bytes())
}

/// The resource name a `NextToken` names, or `None` when the token was not minted by
/// [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// What a listing refuses a pagination token it cannot resolve against the current
/// result set with. Stated once, beside the token vocabulary above, so every paginated
/// listing of this service refuses alike.
pub const STALE_PAGE_TOKEN: &str =
    "NextToken does not name a page boundary of this result set; restart the listing \
     without it.";

/// `MlflowAppStatus`'s members this emulator moves an MLflow app through (the modelled
/// enum also carries the update, delete and failure states, which creating one does not
/// reach). Named once here so no handler compares a bare string literal.
pub const MLFLOW_APP_CREATING: &str = "Creating";
/// See [`MLFLOW_APP_CREATING`].
pub const MLFLOW_APP_CREATED: &str = "Created";

/// `ModelRegistrationMode`'s member `CreateMlflowApp` stores when the request names
/// none: the operation's model doc states "If not specified, AutomaticModelRegistration
/// defaults to AutoModelRegistrationDisabled".
pub const AUTO_MODEL_REGISTRATION_DISABLED: &str = "AutoModelRegistrationDisabled";

/// `EndpointStatus`'s and `ClusterStatus`'s members this emulator moves a resource
/// through (both modelled enums also carry the update, rollback and delete states,
/// which this slice does not reach). `InferenceComponentStatus` spells the same two
/// members, so an inference component moves through these too. Named once here so no
/// handler compares a bare
/// string literal.
pub const STATUS_CREATING: &str = "Creating";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_SERVICE: &str = "InService";
/// `ClusterStatus`'s member a cluster carries between the call that asked for its
/// deletion and the instant the deletion completes. See [`Cluster::begin_deletion`].
pub const STATUS_DELETING: &str = "Deleting";

/// How long after the call that starts it the clock owes a transient -> ready
/// transition for the resources in this slice: a scheduler resource's `Creating` ->
/// `Created`, a cluster node's `Pending` -> `Running`, a volume attachment's
/// `attaching` -> `attached`, and an export job's `InProgress` -> terminal.
///
/// Real SageMaker takes minutes; the span is compressed and never zero, so a caller
/// acting straight after the call still observes the transient state. The due instant is
/// stored and never slept on.
///
/// The span itself is stated ONCE for this service, by [`ENDPOINT_IN_SERVICE_AFTER`];
/// this is that one value under the name the resources above are settled by, so the two
/// cannot drift apart.
pub const TRANSIENT_STATE_SETTLES_AFTER: Duration = ENDPOINT_IN_SERVICE_AFTER;

/// `ClusterInstanceStatus`'s two members this emulator moves an added node through.
pub const NODE_STATUS_PENDING: &str = "Pending";
/// See [`NODE_STATUS_PENDING`].
pub const NODE_STATUS_RUNNING: &str = "Running";
/// `ClusterInstanceStatus`'s member a node carries while the instance it runs on is
/// being terminated, which is what a node replacement starts with. See
/// [`ClusterNode::begin_replacement`].
pub const NODE_STATUS_SHUTTING_DOWN: &str = "ShuttingDown";

/// `VolumeAttachmentStatus`'s two members this emulator moves an attachment through.
pub const VOLUME_STATUS_ATTACHING: &str = "attaching";
/// See [`VOLUME_STATUS_ATTACHING`].
pub const VOLUME_STATUS_ATTACHED: &str = "attached";

/// The device names this service hands successive volumes attached to one node.
///
/// AWS's EBS device-naming guidance recommends `/dev/sdf` through `/dev/sdp` for data
/// volumes on a Linux instance; every entry also satisfies the modelled
/// `VolumeDeviceName` pattern, whose `sd[b-z]` branch covers them.
pub const VOLUME_DEVICE_NAMES: [&str; 11] = [
    "/dev/sdf", "/dev/sdg", "/dev/sdh", "/dev/sdi", "/dev/sdj", "/dev/sdk", "/dev/sdl",
    "/dev/sdm", "/dev/sdn", "/dev/sdo", "/dev/sdp",
];

/// What every ARN this service mints, and every ARN-shaped member it accepts, begins
/// with — so a name-or-ARN member is told apart from a bare name once, here.
pub const ARN_PREFIX: &str = "arn:";

/// `GetScalingConfigurationRecommendation`'s documented default for
/// `TargetCpuUtilizationPerCore`: "The percentage of how much utilization you want an
/// instance to use before autoscaling. The default value is 50%."
pub const DEFAULT_TARGET_CPU_UTILIZATION_PER_CORE: i32 = 50;

/// The cooldowns the recommended dynamic scaling configuration carries, in seconds.
///
/// The emulator's OWN value rather than a claim about any published AWS default: the
/// model documents none, and a recommendation has to carry some cooldown for a caller to
/// apply. Equal for scale-in and scale-out, so neither direction is favoured.
pub const RECOMMENDED_SCALING_COOLDOWN_SECONDS: i32 = 300;

/// The target-tracking metric a SageMaker endpoint variant scales on, as Application
/// Auto Scaling names its predefined metric for SageMaker.
pub const PREDEFINED_SCALING_METRIC: &str = "SageMakerVariantInvocationsPerInstance";

/// A fresh 12-character lowercase-hex resource id: the form SageMaker's
/// `compute-quota/` and `cluster-scheduler-config/` ARNs carry (`^[a-z0-9]{12}$`), and
/// the one [`mint_cluster_id`] gives a cluster.
///
/// The digits come from this module's ONE id scheme, [`mint_opaque_resource_id`], so
/// every server-generated id in this crate is minted the same way and a new one needs no
/// second scheme: take as many of its digits as the model's pattern admits.
pub fn mint_short_id() -> String {
    mint_opaque_resource_id().chars().take(12).collect()
}

/// A fresh EC2 instance id for a node that has reached `Running`: `i-` and 17 hex
/// characters, the long form of the modelled `ClusterNodeId` pattern. Same scheme as
/// [`mint_short_id`], cut to the length this pattern admits.
pub fn mint_instance_id() -> String {
    let digits: String = mint_opaque_resource_id().chars().take(17).collect();
    format!("i-{digits}")
}

/// A fresh `NodeLogicalId`: the id `BatchAddClusterNodes` answers for a node it added,
/// before the node has an instance id. The 32 hex digits [`mint_short_id`]'s scheme
/// mints, which the modelled `ClusterNodeLogicalId` pattern
/// (`^[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9]$`, len 1..128) accepts.
pub fn mint_node_logical_id() -> String {
    mint_opaque_resource_id()
}

/// `TransformJobStatus`'s members this emulator moves a transform job through (the
/// modelled enum also carries `Completed`, `Stopping` and `Stopped`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal.
pub const TRANSFORM_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TRANSFORM_STATUS_IN_PROGRESS`].
pub const TRANSFORM_STATUS_FAILED: &str = "Failed";

/// How long after `CreateTransformJob` the clock owes the job's terminal status.
///
/// A caller that describes the job straight after creating it still observes
/// `InProgress`; one that waits (as `aws sagemaker wait transform-job-completed-or-stopped`
/// does) observes the terminal status. Real SageMaker takes minutes to provision the ML
/// instances; the span is compressed, never zero.
pub const TRANSFORM_JOB_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// Why a batch transform job this emulator accepted ends in the modelled `Failed`
/// status.
///
/// A batch transform runs the model's inference container against every record of the
/// input dataset; this emulator runs no container, so no inference is performed and no
/// transformed object is written. Reporting `Completed` — with an `S3OutputPath` nothing
/// wrote — would claim work that never ran, so the job ends in the status the model
/// carries for a job that did not produce its output, with this reason saying why.
pub const TRANSFORM_NOT_RUN_FAILURE: &str =
    "This emulator runs no model inference container, so no records were transformed and \
     no output object was written to the job's S3OutputPath.";

/// `MaxPayloadInMB`'s documented ceiling: "The value of MaxPayloadInMB cannot be greater
/// than 100 MB" and "the value of (MaxConcurrentTransforms * MaxPayloadInMB) also cannot
/// exceed 100 MB" (`CreateTransformJob$MaxPayloadInMB`'s model doc). The model constrains
/// the member only from below (`range 0..`), so the ceiling is enforced in the handler.
pub const MAX_PAYLOAD_IN_MB_CEILING: i32 = 100;

/// `MaxPayloadInMB` when the request leaves it unset: "The default value is 6 MB"
/// (`CreateTransformJob$MaxPayloadInMB`'s model doc).
pub const DEFAULT_MAX_PAYLOAD_IN_MB: i32 = 6;

/// `MaxConcurrentTransforms` when the request leaves it unset or zero: "If
/// MaxConcurrentTransforms is set to 0 or left unset, Amazon SageMaker checks the optional
/// execution-parameters to determine the settings for your chosen algorithm. If the
/// execution-parameters endpoint is not enabled, the default value is 1"
/// (`CreateTransformJob$MaxConcurrentTransforms`'s model doc). This emulator serves no
/// execution-parameters endpoint, so the documented fallback of 1 is what it resolves.
pub const DEFAULT_MAX_CONCURRENT_TRANSFORMS: i32 = 1;

/// `CompressionType` and `SplitType` when the request leaves them unset: "The default
/// value is `None`" (`TransformInput$CompressionType`) and "The default value for
/// SplitType is `None`" (`TransformInput$SplitType`).
pub const DEFAULT_NONE: &str = "None";

/// How long after `CreateModelPackage` the clock owes the `Pending` -> `Completed`
/// transition of `ModelPackageStatus`.
///
/// `CreateModelPackage` answers with an ARN while registration (and any validation the
/// request asked for) is still outstanding, so a caller that describes the package
/// straight away still observes `Pending`, and one that waits observes `Completed` —
/// which is what lets a waiter terminate. Real SageMaker takes anywhere from seconds to
/// minutes depending on the validation profiles; the span is compressed, never zero.
///
/// It also has to OUTLAST the caller's next request, or the non-terminal status is not
/// observable at all: a span of a few hundred milliseconds can be spent inside the single
/// round trip a client makes between the create and its first read, so a client that
/// describes the package it just registered would be told `Completed` and never see the
/// registration settle. This window is three seconds for that reason, an order of
/// magnitude more room than one request needs. The other clock-owed spans in this module
/// are narrower, each with the resource it belongs to; widening one of those belongs with
/// the readers and suites of that resource, not here.
pub const MODEL_PACKAGE_COMPLETED_AFTER: Duration = Duration::from_secs(3);

/// The `ModelPackageStatus` members this emulator moves a model package through (the
/// modelled enum also carries `InProgress`, `Failed` and `Deleting`, which this slice
/// does not reach). Named once here so no handler compares a bare string literal, and
/// separately from [`STATUS_COMPLETED`], which is a `ModelPackageGroupStatus` member.
pub const MODEL_PACKAGE_STATUS_PENDING: &str = "Pending";
/// See [`MODEL_PACKAGE_STATUS_PENDING`].
pub const MODEL_PACKAGE_STATUS_COMPLETED: &str = "Completed";

/// The `DetailedModelPackageStatus` members a per-profile validation item reports while
/// the package is `Pending` and once it is `Completed`.
pub const DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED: &str = "NotStarted";
/// See [`DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED`].
pub const DETAILED_MODEL_PACKAGE_STATUS_COMPLETED: &str = "Completed";

/// The `ModelApprovalStatus` a registered model version carries when the request named
/// none: SageMaker's model registry documents a new version as awaiting manual approval,
/// and `CreateModelPackage` documents the member as optional for versioned models.
pub const MODEL_APPROVAL_PENDING_MANUAL: &str = "PendingManualApproval";

/// The `ModelPackageRegistrationType` a package `CreateModelPackage` put in the registry
/// carries: it was REGISTERED. The enum's other member, `Logged`, is what a run logs.
pub const MODEL_PACKAGE_REGISTRATION_TYPE_REGISTERED: &str = "Registered";

/// How many `UpdateModelPackage` idempotency tokens one model package remembers.
///
/// `ClientToken` "guarantees that the call to this API is idempotent", so a retry must
/// replay rather than append a second copy of the additional inference specifications it
/// carried. Keeping the tokens on the record bounds them by the record's own lifetime,
/// and this cap bounds how many one record holds, so a caller updating a package in a
/// loop cannot grow the store without end. It is the emulator's own bound, not an AWS
/// one: real SageMaker's deduplication window is not published.
pub const MODEL_PACKAGE_UPDATE_TOKEN_HISTORY: usize = 64;

/// How many `CreateModelPackage` idempotency tokens this account remembers.
///
/// A spent token has to outlive the package it registered (see
/// [`ComAmazonawsSagemakerSagemakerState::model_package_client_tokens`]), so the record
/// cannot be bounded by the package store and is bounded here instead: a caller registering
/// package after package drops the oldest token rather than growing the store without end.
/// The emulator's own bound, not an AWS one: real SageMaker's deduplication window is not
/// published.
pub const MODEL_PACKAGE_CLIENT_TOKEN_HISTORY: usize = 256;

/// The largest number of additional inference specifications one model package may hold:
/// "Total number of additional Inference Specifications can not exceed 15"
/// (`UpdateModelPackageInput$AdditionalInferenceSpecificationsToAdd`'s model doc). The
/// member's own modelled length bounds ONE request's list, so the running total is
/// enforced in the handler.
pub const ADDITIONAL_INFERENCE_SPECIFICATIONS_MAX: usize = 15;

/// The largest number of customer metadata properties one model package may hold: the
/// modelled `length` of `CustomerMetadataMap` (`min 1, max 50`), which bounds the map a
/// request carries and therefore the merged map a later read replays.
pub const CUSTOMER_METADATA_PROPERTIES_MAX: usize = 50;

/// `AppStatus`'s members this emulator moves an app through. The modelled enum
/// (`smithy_query(query="shape", shape="AppStatus")`) also carries `Deleting`,
/// `Deleted` and `Failed`, which this slice does not reach: `DeleteApp` releases the
/// record, and nothing here fails a launch.
pub const APP_STATUS_PENDING: &str = "Pending";
/// See [`APP_STATUS_PENDING`].
pub const APP_STATUS_IN_SERVICE: &str = "InService";

/// The `Completed` status this service's resources report, shared by the two modelled
/// enums that name it.
///
/// It is the `ModelPackageGroupStatus` a model package group carries once
/// `CreateModelPackageGroup` has returned: a model group is a registry container, not a
/// provisioned resource — the call that creates it is synchronous, and the group is
/// immediately usable (a model package can be registered into it on the very next call).
/// So it is born in this TERMINAL status and the clock owes it no later transition — the
/// modelled enum's `Pending`/`InProgress` describe a group whose creation is still in
/// flight, which never happens here.
pub const STATUS_COMPLETED: &str = "Completed";

// One module file per resource slice of this state, each re-exported with `pub use` so
// every handler keeps naming what it needs as `crate::state::…` whichever file holds it.
// A slice owns its records, its bounds, its statuses and clock-owed spans, and the
// sentences it refuses a request with. They live in their own files so this module stays
// inside the project's file-length limit and so a slice's state reads as one unit instead
// of another block appended to a shared region.

/// The algorithm slice: see `state/algorithms.rs`.
pub mod algorithms;
pub use algorithms::*;

/// The AI-benchmark-job slice: see `state/ai_benchmark_job.rs`.
pub mod ai_benchmark_job;
pub use ai_benchmark_job::*;

/// The AI-workload-configuration slice: see `state/ai_workload_config.rs`.
pub mod ai_workload_config;
pub use ai_workload_config::*;

/// The compute-allocation slice: see `state/compute_quota.rs`.
pub mod compute_quota;
pub use compute_quota::*;

/// The hosting slice (endpoints and their configurations): see `state/endpoints.rs`.
pub mod endpoints;
pub use endpoints::*;

/// The experiment-management slice: see `state/experiment.rs`.
pub mod experiment;
pub use experiment::*;

/// The experiment-tracking slice: see `state/trial_component.rs`.
pub mod trial_component;
pub use trial_component::*;

/// Both experiment slices under one name, the way `state/experiments.rs` gathers them.
pub mod experiments;

/// The Feature Store control-plane slice: see `state/feature_group.rs`.
pub mod feature_group;
pub use feature_group::*;

/// The custom-image slice: see `state/image.rs`.
pub mod image;
pub use image::*;

/// The custom-image slice under the name `state/custom_image.rs` gathers it.
pub mod custom_image;

/// The Studio DOMAIN slice — the domain `CreateDomain` writes, `DescribeDomain` replays
/// and `ListDomains` pages over, with its ARN kind, id mint, Studio address, statuses,
/// clock span, store bound and not-found sentence: see `state/domain.rs`.
pub mod domain;
pub use domain::*;

/// The job slices (training, processing and transform jobs): see `state/jobs.rs`.
pub mod jobs;
pub use jobs::*;

/// The server-side RUN of an accepted training job — the training data it reads, the model
/// it fits, the artifact it writes and the metrics it measures: see `state/training_run.rs`.
pub mod training_run;
pub use training_run::*;

/// The lineage-action slice: see `state/lineage_action.rs`.
pub mod lineage_action;
pub use lineage_action::*;

/// The lineage-association slice: see `state/lineage_association.rs`.
pub mod lineage_association;
pub use lineage_association::*;

/// The lineage-context slice: see `state/lineage_context.rs`.
pub mod lineage_context;
pub use lineage_context::*;

/// The lineage-group slice: see `state/lineage_group.rs`.
pub mod lineage_group;
pub use lineage_group::*;

/// Every lineage slice under one name, the way `state/lineage.rs` gathers them.
pub mod lineage;

/// The MLflow tracking-server slice: see `state/mlflow_tracking_server.rs`.
pub mod mlflow_tracking_server;
pub use mlflow_tracking_server::*;

/// The model-card slice: see `state/model_card.rs`.
pub mod model_card;
pub use model_card::*;

/// What a model card EXPORT runs — the PDF it renders and the object it writes: see
/// `state/model_card_export.rs`.
pub mod model_card_export;
pub use model_card_export::*;

/// The notebook-instance slice: see `state/notebook_instance.rs`.
pub mod notebook_instance;
pub use notebook_instance::*;

/// The notebook lifecycle-configuration HOOK slice — what one hook's script does: see
/// `state/notebook_lifecycle_hook.rs`.
pub mod notebook_lifecycle_hook;
pub use notebook_lifecycle_hook::*;

/// Where a hook's output is published: see `state/notebook_hook_logs.rs`.
pub mod notebook_hook_logs;
pub use notebook_hook_logs::*;

/// The partner-AI-app slice: see `state/partner_app.rs`.
pub mod partner_app;
pub use partner_app::*;

/// The presigned-session slice — where a URL this service hands a caller is addressed and
/// what a fetch of it resolves to: see `state/presigned_session.rs`.
pub mod presigned_session;
pub use presigned_session::*;

/// The pipeline-definition slice: see `state/pipeline_definition.rs`.
pub mod pipeline_definition;
pub use pipeline_definition::*;

/// The resource-lookup slice: see `state/resource_lookups.rs`.
pub mod resource_lookups;

// ---------------------------------------------------------------------------
// The resource-tagging slice: the ONE `ResourceArn` -> tag-list resolution that
// `AddTags`, `ListTags` and `DeleteTags` share. A tag belongs to the resource it was
// attached to, so it lives on that resource's record (every taggable record carries a
// `tags` field its `Create*` filled from the request's `Tags`) rather than in a second
// map beside the stores — otherwise a resource would have two tag sets and the
// `Create*` one would be invisible to `ListTags`.
//
// Kept here, beside the hub slice below rather than at either end of the module, for the
// same reason that one is: this slice belongs to no single resource kind, so it must not
// sit in the region every new resource slice is appended to.
// ---------------------------------------------------------------------------

/// The most tags one SageMaker resource holds.
///
/// `TagList` is `len 0..50` wherever this service models it — on `AddTags$Tags`,
/// `ListTags$Tags` and every `Create*$Tags` (`smithy_query shape=TagList`) — so a
/// resource carrying more than 50 could not be reported at all. A write that would take
/// a resource past it is refused instead.
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The entity label [`does_not_exist`] names a tag operation's `ResourceArn` by.
///
/// `AddTags`, `ListTags` and `DeleteTags` address ANY SageMaker resource kind by ARN —
/// `ResourceArn`'s pattern constrains only the partition, service, Region and account —
/// so the refusal for an ARN this account holds no resource under names the generic
/// entity and the ARN the request sent, which is the only identifier the caller supplied.
pub const TAGGED_RESOURCE_ENTITY: &str = "Resource";

/// The tag-refusal slice: see `state/tagged_resource.rs` for the sentence a tag
/// operation states an unresolvable `ResourceArn` with, which is the owning resource
/// family's own — this label is what remains for an ARN naming no family of this account.
pub mod tagged_resource;
pub use tagged_resource::*;

/// The tag-NAMESPACE slice: which tag keys a caller may write, and the sentence a key
/// Amazon Web Services keeps for itself is refused with. Re-exported below, so a handler
/// names them as `crate::state::…` like every other slice.
pub mod tag_namespace;
pub use tag_namespace::*;

/// Generates the two tag accessors over the list of stores whose records carry BOTH an
/// `arn` and a `tags` list, so the reader (`ListTags`) and the writers (`AddTags`,
/// `DeleteTags`) resolve one ARN through exactly the same stores and cannot disagree
/// about which resource kinds this service tags.
///
/// The list below is EVERY store this service holds whose record carries both, and the
/// invariant it keeps is checkable with one grep: a record with a `tags: Vec<Tag>` field
/// and an `arn` that is not in the list would have its tags unreachable, and a tag call
/// naming it falsely refused. A store that is a slice of its own rather than a plain
/// `BTreeMap` field joins the list by answering `values`/`values_mut` the way
/// [`cluster_policy::Store`] does, so this expansion stays one shape.
///
/// Every store left out is left out for ONE reason: its record holds no `tags: Vec<Tag>`
/// field at all, so there is no tag set in it to reach. Named here in full so the
/// omission is a statement rather than an oversight — a tag call naming one of these
/// resources is refused with [`does_not_exist`], which is the refusal for an ARN this
/// resolution holds no TAG SET for:
///
/// * `lineage_groups` — materialised by [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`]
///   rather than created by a request, and SageMaker models no `Tags` anywhere on a
///   lineage group.
/// * `pipeline_executions` — `StartPipelineExecution` models no `Tags`.
/// * `model_card_export_jobs` — `CreateModelCardExportJob` models no `Tags` either
///   (smithy_query operation=CreateModelCardExportJob), so an export job never carried
///   a tag set.
/// * `image_versions` — the tags belong to the image, which is in the list.
/// * `associations` — an edge between two entities, not a taggable resource.
///
/// Each becomes taggable by growing a `tags` field its writer fills and being added to
/// this one list.
macro_rules! tag_accessors {
    ($($store:ident),+ $(,)?) => {
        impl ComAmazonawsSagemakerSagemakerState {
            /// The tags of the resource `arn` names, or `None` when no store holds a
            /// record with that ARN.
            ///
            /// A pure read: `ListTags` reports this and writes nothing.
            pub fn tags_of(&self, arn: &str) -> Option<&[Tag]> {
                let mut found: Option<&[Tag]> = None;
                $(
                    if found.is_none() {
                        found = self
                            .$store
                            .values()
                            .find(|record| record.arn == arn)
                            .map(|record| record.tags.as_slice());
                    }
                )+
                found
            }

            /// The tag list of the resource `arn` names, to add to or remove from, or
            /// `None` when no store holds a record with that ARN.
            pub fn tags_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
                let mut found: Option<&mut Vec<Tag>> = None;
                $(
                    if found.is_none() {
                        found = self
                            .$store
                            .values_mut()
                            .find(|record| record.arn == arn)
                            .map(|record| &mut record.tags);
                    }
                )+
                found
            }
        }
    };
}

tag_accessors!(
    actions,
    ai_benchmark_jobs,
    ai_workload_configs,
    algorithms,
    app_image_configs,
    apps,
    artifacts,
    auto_ml_jobs_v2,
    clusters,
    cluster_policies,
    code_repositories,
    compute_quotas,
    contexts,
    endpoint_configs,
    endpoints,
    experiments,
    feature_groups,
    hubs,
    hub_contents,
    images,
    inference_components,
    inference_recommendations_jobs,
    mlflow_apps,
    mlflow_tracking_servers,
    model_cards,
    model_package_groups,
    model_packages,
    models,
    notebook_instance_lifecycle_configs,
    notebook_instances,
    partner_apps,
    pipelines,
    processing_jobs,
    training_jobs,
    transform_jobs,
    trial_components,
    trials,
    tuning_jobs,
);

// ---------------------------------------------------------------------------
// The hub slice: the records `ListHubContents` reports. Kept here, beside the first
// resource record rather than at either end of the module, so it does not sit in the
// region every new resource slice is appended to.
// ---------------------------------------------------------------------------

/// The identity of ONE stored hub content version.
///
/// A hub content is identified by its hub, its `HubContentType` and its
/// `HubContentName` — the three members every hub-content operation requires — and
/// versioned by `HubContentVersion`. The version is held BOTH as its three parsed dotted
/// components (see [`version_key`]), so the map orders `1.10.0` AFTER `1.9.0`, and
/// verbatim, so two versions whose components parse the same are still two records rather
/// than one overwriting the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HubContentKey {
    pub hub_name: String,
    pub hub_content_type: String,
    pub hub_content_name: String,
    pub version: (u32, u32, u32),
    /// `HubContentVersion` verbatim — the tie-break that keeps the identity total.
    pub version_text: String,
}

/// One version of a hub content, as `ListHubContents` summarises it.
///
/// The members are `HubContentInfo`'s (the summary shape `ListHubContents` returns), plus
/// what the writer of a reference stores beside them; `ImportHubContent` and
/// `DescribeHubContent` (each its own open issue) extend this record with the document,
/// markdown and dependencies they store.
#[derive(Debug, Clone)]
pub struct HubContent {
    pub name: String,
    pub arn: String,
    /// A `HubContentType` member: `Model`, `Notebook`, `ModelReference`, `DataSet` or
    /// `JsonDoc`.
    pub content_type: String,
    /// `HubContentVersion`, verbatim as stored, e.g. `1.0.0`.
    pub version: String,
    pub document_schema_version: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// A `HubContentSupportStatus` member: `Supported`, `Deprecated` or `Restricted`.
    pub support_status: Option<String>,
    pub search_keywords: Option<Vec<String>>,
    /// A `HubContentStatus` member: `Available`, `Importing`, `Deleting`,
    /// `ImportFailed`, `DeleteFailed`, `PendingImport` or `PendingDelete`.
    pub status: String,
    /// When THIS version was created.
    pub creation_time: SystemTime,
    /// When the content was first created, before any later version replaced it; absent
    /// on a content that was never revised.
    pub original_creation_time: Option<SystemTime>,
    /// The public hub content this one references, for a `ModelReference`.
    pub sage_maker_public_hub_content_arn: Option<String>,
    /// The lowest version of the referenced public content this reference accepts
    /// (`ReferenceMinVersion`), as the request that created the reference named it, and
    /// absent on a content that references nothing.
    pub reference_min_version: Option<String>,
    /// The tags attached to this content version — `CreateHubContentReference$Tags` as
    /// sent, then whatever the tag operations added or removed. See [`tag_accessors`].
    pub tags: Vec<Tag>,
}

/// A dotted `<major>.<minor>.<patch>` version (`HubContentVersion`,
/// `DocumentSchemaVersion`, both `^\d{1,4}.\d{1,4}.\d{1,4}$`) as a comparable key, so
/// `1.10.0` orders ABOVE `1.9.0` rather than below it as the strings would.
///
/// A component that is not a number counts as 0: the model refuses a malformed version
/// before a handler sees it, so this only keeps the ordering total.
pub fn version_key(version: &str) -> (u32, u32, u32) {
    let mut components = version.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    (
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
        components.next().unwrap_or(0),
    )
}

/// A model created by `CreateModel`, as `DescribeModel` and `ListModels` replay it.
#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub arn: String,
    pub primary_container: Option<ContainerDefinition>,
    pub containers: Option<Vec<ContainerDefinition>>,
    pub inference_execution_config: Option<InferenceExecutionConfig>,
    pub execution_role_arn: Option<String>,
    pub vpc_config: Option<VpcConfig>,
    pub enable_network_isolation: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

/// How long after `CreateHub` the clock owes the hub's `Creating` -> `InService`
/// transition. `HubStatus` models `Creating` as well as `InService`, so a hub is not
/// born ready: a caller that describes it straight after creating it still observes
/// `Creating`, and one that waits observes `InService`. The span is the emulator's own
/// compressed one, never zero.
pub const HUB_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// The most hubs the store holds, after which `CreateHub` is refused with the modelled
/// `ResourceLimitExceeded`. The emulator's OWN bound, for the reason given on
/// [`MODEL_QUOTA`]: every record it holds is released by `DeleteHub`.
pub const HUB_QUOTA: usize = 1_000;

/// A hub created by `CreateHub`, as `DescribeHub` replays it, `ListHubContents`
/// resolves it and `DeleteHub` releases it.
///
/// `status` is a `HubStatus` member (the model's enum: `InService`, `Creating`,
/// `Updating`, `Deleting`, `CreateFailed`, `UpdateFailed`, `DeleteFailed`).
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Hub {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub display_name: Option<String>,
    pub search_keywords: Option<Vec<String>>,
    pub s3_storage_config: Option<crate::ops::types::HubS3StorageConfig>,
    pub status: String,
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl Hub {
    /// Materialise the `Creating` -> `InService` transition the clock already owes; see
    /// [`settle_creating`].
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        settle_creating(
            &mut self.status,
            &mut self.last_modified_time,
            self.in_service_at,
            now,
        )
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another recommendation job fits, i.e. the store is below
    /// [`INFERENCE_RECOMMENDATIONS_JOB_QUOTA`].
    pub fn inference_recommendations_jobs_below_quota(&self) -> bool {
        self.inference_recommendations_jobs.len() < INFERENCE_RECOMMENDATIONS_JOB_QUOTA
    }
}

/// A pipeline created by `CreatePipeline`, as `DescribePipeline` replays it until
/// `DeletePipeline` removes it.
///
/// A pipeline carries no deletion clock: recorded AWS makes a deleted pipeline
/// unreadable from the delete call on, so `DeletePipeline` drops the record outright
/// and there is no transient state for a later call to materialise.
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub name: String,
    pub arn: String,
    /// The display name the pipeline is shown under. A create that names none leaves the
    /// pipeline displayed under its `PipelineName`, so the member is resolved ONCE where
    /// the record is written and every reader replays the resolved value.
    pub display_name: String,
    pub definition: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub status: String,
    pub parallelism_configuration: Option<ParallelismConfiguration>,
    pub tags: Vec<Tag>,
    /// The identity that created the pipeline, and the identity of the last call that
    /// modified it — recorded when each call happens, as [`caller_user_context`] builds
    /// it, and replayed unchanged.
    pub created_by: UserContext,
    pub last_modified_by: UserContext,
    /// The `ClientRequestToken` the creating call presented, so a retry of that very
    /// call is completed "no more than one time" as the member documents.
    pub client_request_token: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// When an execution of this pipeline last started, which is what
    /// `StartPipelineExecution` stamps; `None` until one has.
    pub last_run_time: Option<SystemTime>,
}

/// One execution of a pipeline, started by `StartPipelineExecution` and replayed by
/// `DescribePipelineExecution`.
///
/// Keyed in the store by its own ARN (`<pipeline arn>/execution/<id>`), the only
/// identifier `DescribePipelineExecution` resolves one by, the way the lineage stores
/// here are keyed by the ARN their create mints.
///
/// `ends_at` is the instant the clock owes the execution's terminal status; it is
/// stored, never slept on, and materialised by [`PipelineExecution::settle_status`].
#[derive(Debug, Clone)]
pub struct PipelineExecution {
    pub arn: String,
    /// The `PipelineName` this execution belongs to — the pipeline store's key, so the
    /// record is found again when the pipeline is deleted or the pipeline's executions
    /// are enumerated.
    pub pipeline_name: String,
    pub pipeline_arn: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub failure_reason: Option<String>,
    /// The parameter values the start call supplied, each naming a parameter the stored
    /// definition declares.
    pub parameters: Vec<Parameter>,
    pub parallelism_configuration: Option<ParallelismConfiguration>,
    pub selective_execution_config: Option<SelectiveExecutionConfig>,
    pub pipeline_version_id: i64,
    /// The `MlflowExperimentName` the start call supplied, which is the only member of
    /// the reported `MLflowConfig` this emulator knows: it serves no MLflow tracking
    /// server, so no resource ARN is invented beside it.
    pub mlflow_experiment_name: Option<String>,
    /// The `ClientRequestToken` the starting call presented, so a retry of that very
    /// call completes "no more than one time" as the member documents.
    pub client_request_token: String,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub ends_at: SystemTime,
    /// Why the step that needed the pipeline's role could not be performed AS it, as the
    /// start call resolved the role against this account's IAM
    /// ([`admission::execution_role_failure_reason`]), or `None` when IAM answered
    /// nothing against it.
    ///
    /// Resolved when the run STARTS, which is when the role is used, and carried on the
    /// run rather than on the pipeline: a pipeline whose role is created after it is
    /// registered runs under a role that resolves, so nothing about the role is settled
    /// before a run begins. It is what [`PipelineExecution::settle_status`] reports as the
    /// failed run's reason, so the step the role was needed for carries it too.
    pub role_failure_reason: Option<String>,
}

impl PipelineExecution {
    /// Materialise the move out of `Executing` the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the move was due at
    /// `ends_at` whether or not anybody described the execution, so the observer makes
    /// no difference to the result. `LastModifiedTime` takes the instant the move was
    /// due, not "now", so it is stable across later reads.
    ///
    /// The terminal status is [`PIPELINE_EXECUTION_STATUS_FAILED`]. Its reason is the
    /// run's own `role_failure_reason` where the role its steps had to be performed as
    /// resolved to nothing, and [`PIPELINE_EXECUTION_NOT_RUN`] otherwise: see that
    /// constant for why an execution this emulator accepted cannot end `Succeeded`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PIPELINE_EXECUTION_STATUS_EXECUTING && now >= self.ends_at {
            self.status = PIPELINE_EXECUTION_STATUS_FAILED.to_string();
            self.failure_reason = Some(
                self.role_failure_reason
                    .clone()
                    .unwrap_or_else(|| PIPELINE_EXECUTION_NOT_RUN.to_string()),
            );
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

/// A batch transform job created by `CreateTransformJob`, as `DescribeTransformJob`
/// replays it and `ListTransformJobs` summarises it.
///
/// `ends_at` is the instant the clock owes the job's terminal status; it is stored,
/// never slept on, and materialised by whichever reporting call first runs at or after
/// it.
///
/// `last_modified_time` starts at the creation instant and moves only when the record
/// does, which is what `ListTransformJobs`' `LastModifiedTime` bounds select on and what
/// its summaries report.
#[derive(Debug, Clone)]
pub struct TransformJob {
    pub name: String,
    pub arn: String,
    pub model_name: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub max_concurrent_transforms: i32,
    pub model_client_config: Option<ModelClientConfig>,
    pub max_payload_in_mb: i32,
    pub batch_strategy: Option<String>,
    pub environment: Option<::std::collections::HashMap<String, String>>,
    pub transform_input: TransformInput,
    pub transform_output: TransformOutput,
    pub data_capture_config: Option<BatchDataCaptureConfig>,
    pub transform_resources: TransformResources,
    pub data_processing: Option<DataProcessing>,
    pub experiment_config: Option<ExperimentConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub transform_end_time: Option<SystemTime>,
    pub ends_at: SystemTime,
}

impl TransformJob {
    /// Materialise the terminal status the clock already owes.
    ///
    /// This is the one write a reporting operation may make: the job was due to end at
    /// `ends_at` whether or not anybody called, so the observer makes no difference to
    /// the result. `TransformEndTime` is the instant the end was due, not "now", so the
    /// value is stable across later reads, and `LastModifiedTime` moves to that same
    /// instant for the same reason.
    ///
    /// The status is [`TRANSFORM_STATUS_FAILED`] with [`TRANSFORM_NOT_RUN_FAILURE`]: see
    /// that constant for why a job this emulator accepted cannot end `Completed`.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRANSFORM_STATUS_IN_PROGRESS && now >= self.ends_at {
            self.status = TRANSFORM_STATUS_FAILED.to_string();
            self.failure_reason = Some(TRANSFORM_NOT_RUN_FAILURE.to_string());
            self.transform_end_time = Some(self.ends_at);
            self.last_modified_time = self.ends_at;
            return true;
        }
        false
    }
}

/// An MLflow app created by `CreateMlflowApp`, as `DescribeMlflowApp` replays it.
///
/// `created_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like an endpoint's, it is stored and materialised by whichever reporting call first
/// runs at or after it.
///
/// Every member mirrors what the create request carried, so a describe replays the
/// request rather than a value invented on the read path. `model_registration_mode` is
/// the one member with a DOCUMENTED default, so it is stored resolved; the others stay
/// `None` when the request named none, because the model documents no default for them.
///
/// `created_by` is stored at creation (through [`caller_user_context`], as
/// `CreateModelPackageGroup` does) rather than rebuilt per read, so every later read
/// reports the identity that actually created the app.
#[derive(Debug, Clone)]
pub struct MlflowApp {
    pub arn: String,
    pub name: String,
    pub artifact_store_uri: String,
    pub role_arn: String,
    pub kms_key_id: Option<String>,
    pub model_registration_mode: String,
    pub weekly_maintenance_window_start: Option<String>,
    pub account_default_status: Option<String>,
    pub default_domain_id_list: Option<Vec<String>>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_at: SystemTime,
    /// Every presigned-URL grant `CreatePresignedMlflowAppUrl` minted for this app whose
    /// session could still be in use, oldest first. Same record as a notebook instance's
    /// and a partner app's ([`PresignedUrlGrant`]), written through the crate's one
    /// presigned-grant record ([`record_presigned_grant`]), because the three carry the
    /// same thing: the token the published URL authenticates the session with.
    pub grants: Vec<PresignedUrlGrant>,
}

impl MlflowApp {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does: it was due at `created_at` whether or not
    /// anybody described the app, and `LastModifiedTime` moves to the instant it was
    /// due rather than to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MLFLOW_APP_CREATING && now >= self.created_at {
            self.status = MLFLOW_APP_CREATED.to_string();
            self.last_modified_time = self.created_at;
            return true;
        }
        false
    }
}

/// An AppImageConfig created by `CreateAppImageConfig`, as `DescribeAppImageConfig`
/// replays it.
///
/// AWS returns an AppImageConfig ready the moment it is created — the resource carries
/// no status member at all (`smithy_query` shape=AppImageConfigDetails) — so there is no
/// clock-owed transition to settle here, only the two timestamps.
#[derive(Debug, Clone)]
pub struct AppImageConfig {
    pub name: String,
    pub arn: String,
    pub kernel_gateway_image_config: Option<KernelGatewayImageConfig>,
    pub jupyter_lab_app_image_config: Option<JupyterLabAppImageConfig>,
    pub code_editor_app_image_config: Option<CodeEditorAppImageConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// The `<kind>` component [`resource_arn`] takes for an AppImageConfig, as the modelled
/// `AppImageConfigArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:app-image-config/`). Named once so
/// the create, the readers and the update mint and match the same ARN.
pub const APP_IMAGE_CONFIG_ARN_KIND: &str = "app-image-config";

/// How this service states an `AppImageConfigName` no record is stored under: the
/// entity, then the ARN the requested name resolves to in the Region the request signed
/// for IN SQUARE BRACKETS, then `does not exist` — the sentence the Studio family of
/// resources words this fact with, carrying no closing period.
///
/// Stated once so the reader, the update and the delete that resolve the same name word
/// the one fact the same way.
pub fn app_image_config_not_found(region: &str, name: &str) -> String {
    format!(
        "AppImageConfig [{}] does not exist",
        resource_arn(region, APP_IMAGE_CONFIG_ARN_KIND, name)
    )
}

/// A Git repository registered by `CreateCodeRepository`, as `DescribeCodeRepository`
/// replays it.
///
/// `GitConfig` is required on create, so it is stored unconditionally; the reader's
/// member is optional only because the model marks it so. `tags` is stored so a later
/// `ListTags` reports what the create carried.
/// `last_modified_time` is carried separately from `creation_time` because the modelled
/// output reports both; a repository nothing has changed reports the creation instant.
#[derive(Debug, Clone)]
pub struct CodeRepository {
    pub name: String,
    pub arn: String,
    pub git_config: GitConfig,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// Which Studio principal a Studio app hangs off.
///
/// `CreateApp` / `DescribeApp` / `DeleteApp` all take `UserProfileName` and `SpaceName`
/// as optional members whose model docs say each must be set when the other is not
/// (`smithy_query(query="doc", shape="DeleteAppRequest", member="SpaceName")`), so
/// exactly one of the two identifies the app.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppOwner {
    /// `UserProfileName`.
    UserProfile(String),
    /// `SpaceName`.
    Space(String),
}

impl AppOwner {
    /// The owner's name, whichever member named it — the component an app ARN carries.
    pub fn name(&self) -> &str {
        match self {
            Self::UserProfile(name) | Self::Space(name) => name,
        }
    }

    /// `UserProfileName` as the app reports it, i.e. only for a user-profile app.
    pub fn user_profile_name(&self) -> Option<String> {
        match self {
            Self::UserProfile(name) => Some(name.clone()),
            Self::Space(_) => None,
        }
    }

    /// `SpaceName` as the app reports it, i.e. only for a space app.
    pub fn space_name(&self) -> Option<String> {
        match self {
            Self::Space(name) => Some(name.clone()),
            Self::UserProfile(_) => None,
        }
    }
}

/// What identifies one Studio app: the domain, the owning user profile or space, the
/// app type and the app name — the five members every app operation takes. Two apps
/// differing in any one of them are different apps, so this is the store's key.
///
/// Ordered (domain, owner, type, name) so the store has one stable order to list over.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppKey {
    pub domain_id: String,
    pub owner: AppOwner,
    pub app_type: String,
    pub app_name: String,
}

/// Why a request naming neither or both of `UserProfileName` and `SpaceName` is refused.
///
/// Both members are optional in the model, so the boundary validator passes such a
/// request; the rule lives in their docs ("If this value is not set, then
/// UserProfileName must be set", and the mirror of it), and an app whose owner is
/// unnamed — or named twice — cannot be resolved.
pub const APP_OWNER_REQUIRED: &str =
    "Exactly one of UserProfileName or SpaceName must be specified.";

/// The key the app operations' five input members name, or [`APP_OWNER_REQUIRED`] when
/// the owner is not exactly one of the user profile and the space.
///
/// Shared by `CreateApp`, `DescribeApp` and `DeleteApp` so all three resolve an app the
/// same way: a create and the reader or delete that names the same five members must
/// agree on the key, or the resource is unreachable.
pub fn app_key(
    domain_id: &str,
    user_profile_name: Option<&str>,
    space_name: Option<&str>,
    app_type: &str,
    app_name: &str,
) -> Result<AppKey, &'static str> {
    let owner = match (user_profile_name, space_name) {
        (Some(user_profile), None) => AppOwner::UserProfile(user_profile.to_string()),
        (None, Some(space)) => AppOwner::Space(space.to_string()),
        (Some(_), Some(_)) | (None, None) => return Err(APP_OWNER_REQUIRED),
    };
    Ok(AppKey {
        domain_id: domain_id.to_string(),
        owner,
        app_type: app_type.to_string(),
        app_name: app_name.to_string(),
    })
}

/// A Studio app created by `CreateApp`, as `DescribeApp` replays it.
///
/// `in_service_at` is the instant the clock owes the `Pending` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct App {
    pub arn: String,
    pub status: String,
    pub resource_spec: Option<ResourceSpec>,
    pub recovery_mode: Option<bool>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
}

impl App {
    /// Materialise the `Pending` -> `InService` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does for an endpoint.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == APP_STATUS_PENDING && now >= self.in_service_at {
            self.status = APP_STATUS_IN_SERVICE.to_string();
            return true;
        }
        false
    }
}

/// An artifact created by `CreateArtifact`, as `DescribeArtifact` replays it.
///
/// `properties` is the map the artifact HOLDS, empty while nothing has put an entry in it
/// (see this module's note on an entity's own collections). It is a `BTreeMap` so the map
/// serialises in one stable order whatever order the request arrived in. `source` is held
/// with its `SourceTypes` already resolved to the list the artifact holds, so every reader
/// of it reports the same thing.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so the per-Region name uniqueness rule does not have to parse the ARN.
    pub region: String,
    pub name: Option<String>,
    pub artifact_type: String,
    pub source: ArtifactSource,
    pub properties: BTreeMap<String, String>,
    pub metadata_properties: Option<MetadataProperties>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// An artifact's `Source` as the store holds it: the source the request gave, with its
/// `SourceTypes` resolved to the list the artifact HOLDS — the empty list when the request
/// named none (see this module's note on an entity's own collections).
///
/// Applied where an artifact is written, so every reader of the stored source — the
/// describe and the listing's summary — reports the same thing without normalising it
/// again.
pub fn stored_artifact_source(source: ArtifactSource) -> ArtifactSource {
    ArtifactSource {
        source_uri: source.source_uri,
        source_types: Some(source.source_types.unwrap_or_default()),
    }
}

/// A model package group created by `CreateModelPackageGroup`, as
/// `DescribeModelPackageGroup` replays it, the container a VERSIONED model package is
/// registered into, and the resource `PutModelPackageGroupPolicy` attaches a policy to.
///
/// `created_by` is stored at creation rather than rebuilt per read, so every later read
/// reports the identity that actually created the group.
///
/// `resource_policy` is the resource-based policy `PutModelPackageGroupPolicy` attached to
/// the group, exactly as that call sent it, and `None` while the group carries none. It
/// lives ON the group record rather than in a store of its own, so the one group has one
/// policy whichever operation reads it, it is bounded by
/// [`MODEL_PACKAGE_GROUP_QUOTA`] with the record it belongs to, and
/// `DeleteModelPackageGroup` releases the policy with the group.
#[derive(Debug, Clone)]
pub struct ModelPackageGroup {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub managed_configuration: Option<ManagedConfiguration>,
    pub status: String,
    pub created_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The policy document the last `PutModelPackageGroupPolicy` stored, verbatim.
    pub resource_policy: Option<String>,
}

/// One presigned-URL grant a presign minted: the opaque token the URL carries and when
/// it stops being valid.
///
/// The token is a SECRET handed to the caller — it is presented back at the session
/// address the URL names and matched against this record, which is what authenticates the
/// session ([`crate::state::presigned_session`]) — so it is minted by the emulator's
/// credential store ([`mint_session_token`]) rather than constructed in this service, and
/// is never reported by any read of the resource it belongs to.
#[derive(Debug, Clone)]
pub struct PresignedUrlGrant {
    pub token: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub session_expires_at: SystemTime,
}

/// What one remembered `BatchAddClusterNodes` `ClientToken` replays: the nodes that call
/// added and the per-instance-group failures it reported, so the same token answers the
/// same thing instead of adding the nodes twice.
#[derive(Debug, Clone)]
pub struct NodeAdditionRecord {
    /// The `NodeLogicalId`s the call added, in the order it answered them.
    pub added: Vec<String>,
    pub failed: Vec<NodeAdditionFailure>,
    /// When the call that this record replays ran, so the token store evicts by age.
    pub issued_at: SystemTime,
}

/// One per-instance-group failure of a `BatchAddClusterNodes` call, as its `Failed` list
/// reported it.
#[derive(Debug, Clone)]
pub struct NodeAdditionFailure {
    pub instance_group_name: String,
    pub error_code: String,
    pub failed_count: i32,
    pub availability_zones: Option<Vec<String>>,
    pub instance_types: Option<Vec<String>>,
    pub message: Option<String>,
}

impl Cluster {
    /// The instance group of this cluster named `name`, as `CreateCluster` stored its
    /// specification.
    pub fn instance_group(
        &self,
        name: &str,
    ) -> Option<&crate::ops::types::ClusterInstanceGroupSpecification> {
        self.instance_groups
            .iter()
            .find(|group| group.instance_group_name == name)
    }

    /// [`Self::instance_group`], for a caller that raises the group's target count.
    pub fn instance_group_mut(
        &mut self,
        name: &str,
    ) -> Option<&mut crate::ops::types::ClusterInstanceGroupSpecification> {
        self.instance_groups
            .iter_mut()
            .find(|group| group.instance_group_name == name)
    }

    /// The node of this cluster with instance id `instance_id`, once the node has one.
    pub fn node_by_instance_id(&self, instance_id: &str) -> Option<&ClusterNode> {
        self.nodes
            .values()
            .find(|node| node.instance_id.as_deref() == Some(instance_id))
    }

    /// [`Self::node_by_instance_id`], for a caller that changes the node it resolves.
    pub fn node_by_instance_id_mut(&mut self, instance_id: &str) -> Option<&mut ClusterNode> {
        self.nodes
            .values_mut()
            .find(|node| node.instance_id.as_deref() == Some(instance_id))
    }

    /// The `NodeLogicalId` of the node of this cluster with instance id `instance_id`, so
    /// a caller holding an instance id reaches the node's own key. Resolved through
    /// [`Self::node_by_instance_id`], so an instance id resolves ONE way whichever caller
    /// asks.
    pub fn logical_id_of_instance(&self, instance_id: &str) -> Option<String> {
        self.node_by_instance_id(instance_id)
            .map(|node| node.logical_id.clone())
    }

    /// Drop the node `logical_id` names from this cluster, lowering by one the target
    /// count of the instance group it belonged to.
    ///
    /// The mirror of what adding a node does (`BatchAddClusterNodes` raises the same
    /// count), so a group's count and the nodes it holds cannot disagree: a group whose
    /// node was deleted asks for one instance fewer, and the provisioning the cluster does
    /// when it reaches `InService` therefore does not mint it again. Answers the node that
    /// was removed, or `None` when this cluster holds no such node.
    pub fn remove_node(&mut self, logical_id: &str) -> Option<ClusterNode> {
        let node = self.nodes.remove(logical_id)?;
        if let Some(group) = self.instance_group_mut(&node.instance_group_name) {
            group.instance_count = group.instance_count.saturating_sub(1);
        }
        Some(node)
    }

    /// Remember `record` under `token`, keeping the token store bounded.
    ///
    /// At most [`NODE_ADDITION_HISTORY_CAP`] tokens are held, so a caller retrying with
    /// token after token cannot grow the store without end. The one dropped is the one
    /// remembered LONGEST AGO (by its `issued_at`, not by how its token sorts), so the
    /// token just recorded is never the one evicted. A token dropped that way is one the
    /// cluster no longer remembers, so the next call carrying it adds nodes rather than
    /// replaying — the same outcome as a token whose idempotency window has passed.
    pub fn remember_node_addition(&mut self, token: String, record: NodeAdditionRecord) {
        self.node_additions.insert(token, record);
        while self.node_additions.len() > NODE_ADDITION_HISTORY_CAP {
            let oldest = self
                .node_additions
                .iter()
                .min_by_key(|(_, held)| held.issued_at)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => self.node_additions.remove(&token),
                // Unreachable while the map is over the cap, and breaking rather than
                // looping forever is the safe answer if it ever is not.
                None => break,
            };
        }
    }
}

/// One node of a cluster, as `DescribeClusterNode` replays it.
///
/// `running_at` is the instant the clock owes the `Pending` -> `Running` transition,
/// which is also when the node gets its EC2 instance id: a caller that describes a node
/// straight after adding it still observes `Pending` with no instance id.
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub logical_id: String,
    pub instance_id: Option<String>,
    pub instance_group_name: String,
    /// The type the node runs, which is the one its instance group names. Absent for a
    /// group whose specification names `InstanceRequirements` rather than an
    /// `InstanceType`, which the model makes mutually exclusive.
    pub instance_type: Option<String>,
    pub status: String,
    /// Where the node was placed: the zone the add request targeted, since a group's
    /// zones follow from its VPC configuration, which this emulator does not resolve.
    pub availability_zone: Option<String>,
    pub threads_per_core: Option<i32>,
    pub launch_time: SystemTime,
    pub running_at: SystemTime,
    /// The instant the clock owes this node its REPLACEMENT hardware, set by
    /// [`ClusterNode::begin_replacement`]; `None` while no replacement is in flight.
    /// Until then the node reports `ShuttingDown` on the instance being terminated, and
    /// past it it reports `Running` on a freshly minted instance id.
    pub replacement_due_at: Option<SystemTime>,
    /// The instant the clock owes this node its return from a REBOOT, set by
    /// [`ClusterNode::begin_reboot`]; `None` while no reboot is in flight. Until then the
    /// node is not available to a workload, and past it it reports `Running` again on the
    /// instance it was already running on.
    pub reboot_due_at: Option<SystemTime>,
    /// The volumes attached to this node, keyed by `VolumeId`.
    pub volume_attachments: BTreeMap<String, VolumeAttachment>,
}

/// How a per-node failure words the condition that another operation is already changing
/// the node, which is what `BatchReplaceClusterNodes`' `InstanceIdInUse` and
/// `BatchDeleteClusterNodes`' `NodeIdInUse` both describe ("another operation is already
/// in progress for this node"). Named ONCE here so the two operations cannot word the one
/// condition two ways.
pub const NODE_OPERATION_IN_FLIGHT_MESSAGE: &str = "Another operation is already in \
     progress for this node. Wait for the operation to complete before retrying.";

impl ClusterNode {
    /// Materialise the transition the clock already owes this node — the
    /// `Pending` -> `Running` one of a node that is coming up, or the
    /// `ShuttingDown` -> `Running` one of a node whose replacement hardware is due — and
    /// with it the node's instance id. Answers whether this call made a transition.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        // A reboot restarts the operating system of the instance the node ALREADY runs on,
        // so the node comes back on the same instance id and with the same launch time —
        // which is why it is judged before the provisioning transition below, the one that
        // mints an instance id for a node that has none yet.
        if let Some(due) = self.reboot_due_at {
            if now < due {
                return false;
            }
            self.status = NODE_STATUS_RUNNING.to_string();
            self.reboot_due_at = None;
            return true;
        }
        if self.status == NODE_STATUS_PENDING && now >= self.running_at {
            self.status = NODE_STATUS_RUNNING.to_string();
            self.instance_id = Some(mint_instance_id());
            return true;
        }
        // The replacement instance is a DIFFERENT instance of the same node: it carries a
        // newly minted instance id and its own launch time, while the `NodeLogicalId` —
        // the identifier that "persists throughout the node's lifecycle" — is untouched.
        if let Some(due) = self.replacement_due_at.filter(|due| now >= *due) {
            self.status = NODE_STATUS_RUNNING.to_string();
            self.instance_id = Some(mint_instance_id());
            self.launch_time = due;
            self.running_at = due;
            self.replacement_due_at = None;
            return true;
        }
        false
    }

    /// Whether this node is in a SETTLED status, so an operation that changes or removes
    /// it interrupts nothing the service is already doing to it.
    ///
    /// `Running` is the one settled status this emulator moves a node to; a node that is
    /// still coming up, shutting down, updating or being health-checked is mid-change,
    /// which is the condition the per-node invalid-status codes describe.
    pub fn is_settled(&self) -> bool {
        self.status == NODE_STATUS_RUNNING && !self.operation_in_flight()
    }

    /// Whether this node's hardware may be replaced right now: its status is a settled
    /// one, so nothing the service is already doing to the node is interrupted.
    pub fn accepts_replacement(&self) -> bool {
        self.is_settled()
    }

    /// Whether the service has a change of its own already under way on this node, i.e.
    /// "another operation is already in progress for this node" — the condition
    /// [`NODE_OPERATION_IN_FLIGHT_MESSAGE`] words. A node's hardware replacement and its
    /// reboot are both such a change, so one in-flight node operation is told apart from
    /// another request's by this one predicate rather than by naming a single kind.
    pub fn operation_in_flight(&self) -> bool {
        self.replacement_due_at.is_some() || self.reboot_due_at.is_some()
    }

    /// Whether this node may be rebooted right now: its status is a settled one, so the
    /// restart interrupts nothing the service is already doing to the node.
    pub fn accepts_reboot(&self) -> bool {
        self.is_settled()
    }

    /// Start the soft reboot `BatchRebootClusterNodes` asked for: the node's operating
    /// system restarts on the instance it is already running on, so the node is
    /// unavailable until the clock owes it back [`TRANSIENT_STATE_SETTLES_AFTER`] later,
    /// materialised by [`ClusterNode::settle_status`].
    ///
    /// The instance is NOT replaced — its id, its launch time and every volume it holds
    /// survive the restart, which is what a soft recovery preserves and a replacement does
    /// not. `ClusterInstanceStatus` models no reboot member, so the node carries the
    /// member it carries whenever it is coming up and not yet serving.
    pub fn begin_reboot(&mut self, now: SystemTime) {
        self.status = NODE_STATUS_PENDING.to_string();
        self.reboot_due_at = Some(now + TRANSIENT_STATE_SETTLES_AFTER);
    }

    /// Start the replacement `BatchReplaceClusterNodes` asked for: the instance the node
    /// runs on is terminated — so the node reports `ShuttingDown` and every volume it
    /// held is gone — and the clock owes it fresh hardware
    /// [`TRANSIENT_STATE_SETTLES_AFTER`] later, materialised by
    /// [`ClusterNode::settle_status`].
    ///
    /// The node's `NodeLogicalId` and its instance group, type, zone and threads-per-core
    /// are kept: the replacement runs "the same configuration but fresh hardware". The old
    /// instance id stays until the new instance comes up, because that instance is what
    /// the node is still running on while it shuts down. Replacing a node destroys all its
    /// instance volumes, root and secondary, so none survives the call.
    pub fn begin_replacement(&mut self, now: SystemTime) {
        self.status = NODE_STATUS_SHUTTING_DOWN.to_string();
        self.replacement_due_at = Some(now + TRANSIENT_STATE_SETTLES_AFTER);
        self.volume_attachments.clear();
    }

    /// Materialise every `attaching` -> `attached` transition this node's volumes are
    /// owed.
    pub fn settle_attachments(&mut self, now: SystemTime) {
        for attachment in self.volume_attachments.values_mut() {
            attachment.settle_status(now);
        }
    }

    /// The device name to hand the next volume attached to this node: the first entry of
    /// [`VOLUME_DEVICE_NAMES`] no attachment of this node holds, or `None` once the node
    /// has used them all.
    pub fn free_device_name(&self) -> Option<&'static str> {
        VOLUME_DEVICE_NAMES.into_iter().find(|candidate| {
            !self
                .volume_attachments
                .values()
                .any(|attachment| attachment.device_name == *candidate)
        })
    }
}

/// One EBS volume attached to a cluster node by `AttachClusterNodeVolume`.
#[derive(Debug, Clone)]
pub struct VolumeAttachment {
    pub volume_id: String,
    pub device_name: String,
    pub status: String,
    pub attach_time: SystemTime,
    /// The instant the clock owes the `attaching` -> `attached` transition.
    pub attached_at: SystemTime,
}

impl VolumeAttachment {
    /// Materialise the `attaching` -> `attached` transition the clock already owes.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == VOLUME_STATUS_ATTACHING && now >= self.attached_at {
            self.status = VOLUME_STATUS_ATTACHED.to_string();
            return true;
        }
        false
    }
}

/// One benchmarked recommendation of an Inference Recommender job.
#[derive(Debug, Clone)]
pub struct InferenceRecommendation {
    pub recommendation_id: String,
    pub endpoint_name: String,
    pub instance_type: String,
    /// What one instance of `instance_type` sustained, in invocations per minute.
    pub max_invocations_per_minute: i32,
    /// The model latency measured at that load, in milliseconds.
    pub model_latency_millis: i32,
}

/// The most notebook instance lifecycle configurations this service's store holds, after
/// which a create is refused with the modelled `ResourceLimitExceeded`. See
/// [`MODEL_QUOTA`] for why the bound is the emulator's own rather than an account's.
pub const NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA: usize = 1_000;

/// The `<kind>` component [`resource_arn`] takes for a notebook instance lifecycle
/// configuration, as the AWS ARN reference spells SageMaker's resource type for it.
/// Named once so the create and the describe mint and expect the same ARN.
pub const NOTEBOOK_LIFECYCLE_CONFIG_ARN_KIND: &str = "notebook-instance-lifecycle-config";

/// How this service states a notebook instance lifecycle configuration name no record is
/// stored under: the `Could not find <kind> "<arn>".` sentence this family refuses a
/// missing resource with, over the ARN the requested name resolves to in the Region the
/// request signed for.
///
/// Stated once so the reader and the update that resolve the same name — neither of which
/// models a not-found error of its own, so both refuse with a `ValidationException` — word
/// the one fact the same way.
pub fn notebook_instance_lifecycle_config_not_found(region: &str, name: &str) -> String {
    format!(
        "Could not find notebook instance lifecycle configuration \"{}\".",
        resource_arn(region, NOTEBOOK_LIFECYCLE_CONFIG_ARN_KIND, name)
    )
}

/// A notebook instance lifecycle configuration created by
/// `CreateNotebookInstanceLifecycleConfig`, as `DescribeNotebookInstanceLifecycleConfig`
/// replays it.
///
/// `OnCreate` and `OnStart` are modelled as lists of at most one hook each; they are
/// stored exactly as the create request carried them, because the scripts they hold are
/// opaque base64 the service only ever hands back.
#[derive(Debug, Clone)]
pub struct NotebookInstanceLifecycleConfig {
    pub name: String,
    pub arn: String,
    pub on_create: Option<Vec<crate::ops::types::NotebookInstanceLifecycleHook>>,
    pub on_start: Option<Vec<crate::ops::types::NotebookInstanceLifecycleHook>>,
    /// What the create carried, so a later `ListTags`/`AddTags` reports it; no
    /// implemented operation surfaces it yet (`DescribeNotebookInstanceLifecycleConfig`
    /// models no `Tags` member).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another notebook instance lifecycle configuration fits, i.e. the store is
    /// below [`NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA`].
    ///
    /// Kept with the rest of this resource's slice rather than in the first `impl` block,
    /// as the lineage-action slice at the end of this module is.
    pub fn notebook_instance_lifecycle_configs_below_quota(&self) -> bool {
        self.notebook_instance_lifecycle_configs.len() < NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG_QUOTA
    }
}

/// A model package created by `CreateModelPackage`, as `DescribeModelPackage` replays it.
///
/// One record covers both documented kinds: an UNVERSIONED package (the request named
/// `ModelPackageName`, so `group_name` and `version` are absent) and a VERSIONED one (the
/// request named `ModelPackageGroupName`, so `name` is the group's name and `version` is
/// the number this registration was given inside that group).
///
/// `created_by` is stored at creation, as [`ModelPackageGroup`]'s is, so every later read
/// reports the identity that actually registered the package.
///
/// `client_token` is the idempotency token the registering call presented, if any. It is
/// held here rather than in a map of its own so it is bounded by the same
/// [`MODEL_PACKAGE_QUOTA`] as the record and is released with it.
///
/// `completed_at` is the instant the clock owes the `Pending` -> `Completed` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it.
///
/// `last_modified_by` stays absent until a CALL modifies the package (`UpdateModelPackage`
/// records it), the way [`ModelCardVersion`]'s does: the clock settling the registration
/// status is not a caller, so it moves `last_modified_time` without claiming an identity.
///
/// `update_client_tokens` holds the idempotency tokens whose update has already been
/// applied, newest last, so a retry replays instead of appending a second copy of an
/// `AdditionalInferenceSpecificationsToAdd`. It is capped at
/// [`MODEL_PACKAGE_UPDATE_TOKEN_HISTORY`] entries so a caller updating in a loop cannot
/// grow the record without end.
#[derive(Debug, Clone)]
pub struct ModelPackage {
    pub name: String,
    pub group_name: Option<String>,
    pub version: Option<i32>,
    pub arn: String,
    pub description: Option<String>,
    pub registration_type: Option<String>,
    pub inference_specification: Option<InferenceSpecification>,
    pub validation_specification: Option<ModelPackageValidationSpecification>,
    pub source_algorithm_specification: Option<SourceAlgorithmSpecification>,
    pub certify_for_marketplace: Option<bool>,
    pub model_approval_status: Option<String>,
    pub approval_description: Option<String>,
    pub metadata_properties: Option<MetadataProperties>,
    pub model_metrics: Option<ModelMetrics>,
    pub domain: Option<String>,
    pub task: Option<String>,
    pub sample_payload_url: Option<String>,
    pub customer_metadata_properties: Option<HashMap<String, String>>,
    pub drift_check_baselines: Option<DriftCheckBaselines>,
    pub additional_inference_specifications: Option<Vec<AdditionalInferenceSpecificationDefinition>>,
    pub skip_model_validation: Option<String>,
    pub source_uri: Option<String>,
    pub security_config: Option<ModelPackageSecurityConfig>,
    pub model_card: Option<ModelPackageModelCard>,
    pub model_life_cycle: Option<ModelLifeCycle>,
    pub managed_storage_type: Option<String>,
    pub tags: Vec<Tag>,
    pub created_by: UserContext,
    pub last_modified_by: Option<UserContext>,
    pub client_token: Option<String>,
    /// The `ClientToken`s of the updates already applied to this package, newest last.
    pub update_client_tokens: Vec<String>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub completed_at: SystemTime,
}

impl ModelPackage {
    /// The key this package is stored under: its name for an unversioned package,
    /// `<group>/<version>` for a version registered into a group. It is also the
    /// resource part of the package's ARN, so one identifier serves both.
    pub fn key(&self) -> String {
        model_package_key(&self.name, self.version)
    }

    /// Whether `token` has already applied an update to this package, i.e. this call is a
    /// retry of one this record has seen.
    pub fn update_token_applied(&self, token: &str) -> bool {
        self.update_client_tokens.iter().any(|seen| seen == token)
    }

    /// Remember `token` as applied, dropping the oldest entries once the history is at
    /// [`MODEL_PACKAGE_UPDATE_TOKEN_HISTORY`], so the record stays bounded.
    pub fn remember_update_token(&mut self, token: String) {
        self.update_client_tokens.push(token);
        let excess = self
            .update_client_tokens
            .len()
            .saturating_sub(MODEL_PACKAGE_UPDATE_TOKEN_HISTORY);
        if excess > 0 {
            self.update_client_tokens.drain(..excess);
        }
    }

    /// Materialise the `Pending` -> `Completed` transition the clock already owes.
    ///
    /// The same single write a reporting operation may make as [`Endpoint::settle_status`]:
    /// the transition was due at `completed_at` whether or not anybody described the
    /// package, so the observer makes no difference to the result, and
    /// `LastModifiedTime` moves to the instant it was due rather than to "now" so the
    /// value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == MODEL_PACKAGE_STATUS_PENDING && now >= self.completed_at {
            self.status = MODEL_PACKAGE_STATUS_COMPLETED.to_string();
            self.last_modified_time = self.completed_at;
            return true;
        }
        false
    }

    /// `ModelPackageStatusDetails` for this package: one validation item per profile the
    /// request's `ValidationSpecification` named (no profiles, no items), each reporting
    /// `NotStarted` while the package is `Pending` and `Completed` once it has settled.
    ///
    /// `ImageScanStatuses` is reported EMPTY rather than absent, the way an algorithm's
    /// [`Algorithm::status_details`] reports it: the member is how the package reports the
    /// image scans it holds statuses for, and this emulator scans no container image, so
    /// it holds none — which a caller cannot tell from a missing member.
    pub fn status_details(&self) -> ModelPackageStatusDetails {
        let item_status = if self.status == MODEL_PACKAGE_STATUS_COMPLETED {
            DETAILED_MODEL_PACKAGE_STATUS_COMPLETED
        } else {
            DETAILED_MODEL_PACKAGE_STATUS_NOT_STARTED
        };
        let validation_statuses = self
            .validation_specification
            .as_ref()
            .map(|spec| {
                spec.validation_profiles
                    .iter()
                    .map(|profile| ModelPackageStatusItem {
                        name: profile.profile_name.clone(),
                        status: item_status.to_string(),
                        failure_reason: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        ModelPackageStatusDetails {
            validation_statuses,
            image_scan_statuses: Some(Vec::new()),
        }
    }
}

/// The store key (and ARN resource part) of a model package: `name` on its own for an
/// unversioned package, `name/version` for a version inside a model package group.
///
/// The two forms cannot collide: `name` is an `EntityName` / `ModelPackageGroupName`,
/// whose modelled pattern (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`) admits no `/`, and the
/// boundary validator refuses a name outside it before any handler runs.
pub fn model_package_key(name: &str, version: Option<i32>) -> String {
    match version {
        Some(version) => format!("{name}/{version}"),
        None => name.to_string(),
    }
}

/// The ARN segment that marks an identifier as a model package ARN rather than a name:
/// the `:model-package/` before the key [`model_package_lookup_key`] cuts out.
const MODEL_PACKAGE_ARN_SEGMENT: &str = ":model-package/";

/// The store key a caller-supplied model package identifier names.
///
/// `DescribeModelPackage` takes a `VersionedArnOrName`, whose modelled pattern admits
/// exactly three forms: a bare name, a `name/version` pair, or this service's
/// `...:model-package/<key>` ARN — so an identifier reaching here is one of them. An ARN
/// resolves to everything after its `:model-package/` segment, which is exactly the key
/// form above; anything else is already a key.
pub fn model_package_lookup_key(identifier: &str) -> &str {
    identifier
        .split_once(MODEL_PACKAGE_ARN_SEGMENT)
        .map_or(identifier, |(_, resource)| resource)
}

/// Per-service state for com-amazonaws-sagemaker-sagemaker. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerSagemakerState {
    /// HyperPod clusters and the cluster policies on them: one field for that whole
    /// slice, whose records, quota, statuses and id mint live in [`cluster_policy`].
    pub cluster_policies: cluster_policy::Store,
    /// Keyed by [`entity_key`] of `ExperimentName`. See [`Experiment`], with the rest
    /// of the experiment-management slice.
    pub experiments: BTreeMap<String, Experiment>,
    /// Whether this account's SageMaker Service Catalog portfolio is enabled, as
    /// `EnableSagemakerServicecatalogPortfolio` and
    /// `DisableSagemakerServicecatalogPortfolio` set it and
    /// `GetSagemakerServicecatalogPortfolioStatus` reports it, through
    /// [`Self::servicecatalog_portfolio_status`].
    ///
    /// Account-level, so it is a single flag rather than a map: the three operations take
    /// no input and name no resource. `Default` is `false` — an account that has never
    /// enabled the portfolio reads `Disabled`, the only other member of the modelled enum.
    pub servicecatalog_portfolio_enabled: bool,
    /// Keyed by `ModelName`.
    pub models: BTreeMap<String, Model>,
    /// Keyed by `TrainingJobName`.
    pub training_jobs: BTreeMap<String, TrainingJob>,
    /// Keyed by `InferenceComponentName`.
    pub inference_components: BTreeMap<String, InferenceComponent>,
    /// Keyed by `EndpointConfigName`.
    pub endpoint_configs: BTreeMap<String, EndpointConfig>,
    /// Keyed by `EndpointName`.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Keyed by [`entity_key`] of their `HyperParameterTuningJobName`:
    /// `HyperParameterTuningJobName`'s doc says the name "must be unique within the same
    /// Amazon Web Services account and Amazon Web Services Region" and "is not case
    /// sensitive", so two spellings differing only in case are ONE job, while the record
    /// keeps the casing the create sent (and the ARN minted from it).
    pub tuning_jobs: BTreeMap<String, HyperParameterTuningJob>,
    /// Trials, keyed by [`entity_key`] of their `TrialName`: the name "must be unique in
    /// your Amazon Web Services account and is not case-sensitive"
    /// (`CreateTrial$TrialName`), so two spellings differing only in case are ONE trial.
    /// The record keeps the casing the create sent. Reached through [`Self::trial`].
    pub trials: BTreeMap<String, Trial>,
    /// Keyed by `AlgorithmName`.
    pub algorithms: BTreeMap<String, Algorithm>,
    /// AutoML jobs V2, keyed by [`entity_key`] of their `AutoMLJobName`.
    pub auto_ml_jobs_v2: BTreeMap<String, AutoMLJobV2>,
    /// Model-customization jobs, keyed by `JobName`, which AWS documents unique "within
    /// your account and Amazon Web Services Region". See [`Job`], whose slice holds the
    /// record, the lookup and the constants.
    pub jobs: BTreeMap<String, Job>,
    /// Lineage contexts, keyed by `ContextArn`: `ContextName` is documented unique to an
    /// account in a Region, so the ARN — which carries the Region beside the name — is
    /// what identifies one context. See [`context_key`].
    pub contexts: BTreeMap<String, Context>,
    /// Keyed by `PipelineName`.
    pub pipelines: BTreeMap<String, Pipeline>,
    /// Pipeline executions, keyed by `PipelineExecutionArn` — the one identifier
    /// `DescribePipelineExecution` resolves an execution by, and the one
    /// `StartPipelineExecution` mints.
    pub pipeline_executions: BTreeMap<String, PipelineExecution>,
    /// Keyed by `TransformJobName`.
    pub transform_jobs: BTreeMap<String, TransformJob>,
    /// Keyed by `MlflowAppArn` — the app's ARN is its identity, since the model
    /// documents that an MLflow app's NAME is not part of its ARN.
    pub mlflow_apps: BTreeMap<String, MlflowApp>,
    /// Keyed by `TrackingServerName`. See [`MlflowTrackingServer`], further down this
    /// module with the rest of the MLflow tracking-server slice.
    pub mlflow_tracking_servers: BTreeMap<String, MlflowTrackingServer>,
    /// Keyed by `NotebookInstanceLifecycleConfigName`.
    pub notebook_instance_lifecycle_configs: BTreeMap<String, NotebookInstanceLifecycleConfig>,
    /// Keyed by `AppImageConfigName`, which AWS documents as unique to the account.
    pub app_image_configs: BTreeMap<String, AppImageConfig>,
    /// Keyed by `CodeRepositoryName`.
    pub code_repositories: BTreeMap<String, CodeRepository>,
    /// Keyed by [`AppKey`] — an app is named by five members, not by one name.
    pub apps: BTreeMap<AppKey, App>,
    /// Studio domains, keyed by the `DomainId` SageMaker mints for them: a domain is
    /// addressed by that id and by nothing else (`DescribeDomain` takes only
    /// `DomainId`), and `DomainName` carries no documented uniqueness, so the id is the
    /// key and `ListDomains` has one stable order to page over. See [`Domain`], whose
    /// slice lives in [`domain`].
    pub domains: BTreeMap<String, Domain>,
    /// Keyed by `ModelPackageGroupName`.
    pub model_package_groups: BTreeMap<String, ModelPackageGroup>,
    /// Keyed by `RecommendationJobName`, which AWS documents unique to the account in a
    /// Region. See [`InferenceRecommendationsJob`], with the rest of the Inference
    /// Recommender slice.
    pub inference_recommendations_jobs: BTreeMap<String, InferenceRecommendationsJob>,
    /// Keyed by [`model_package_key`], so an unversioned package and the versions of a
    /// group share one store and one stable order.
    pub model_packages: BTreeMap<String, ModelPackage>,
    /// Lineage artifacts, keyed by `ArtifactArn` — the only identifier `DescribeArtifact`
    /// resolves one by, and the one `CreateArtifact` mints.
    pub artifacts: BTreeMap<String, Artifact>,
    /// Keyed by `FeatureGroupName`. Served by the "Feature Store control plane" section
    /// of this module, which holds the whole feature-group slice — its constants, its
    /// record and its quota helper — in one block.
    pub feature_groups: BTreeMap<String, FeatureGroup>,
    /// Keyed by `NotebookInstanceName`. See [`NotebookInstance`], at the end of this
    /// module with the rest of the notebook-instance slice.
    pub notebook_instances: BTreeMap<String, NotebookInstance>,
    /// Lineage actions, keyed by `ActionArn` — the identifier that carries both the
    /// Region and the name, which is what `ActionName` is documented unique within, so a
    /// name taken in one Region leaves it free in another. See [`Action`], at the end of
    /// this module with the rest of the lineage-action slice.
    pub actions: BTreeMap<String, Action>,
    /// Keyed by `ModelCardName`. See [`ModelCard`], at the end of this module with the
    /// rest of the model-card slice.
    pub model_cards: BTreeMap<String, ModelCard>,
    /// Model card export jobs, keyed by [`model_card_export_job_key`] — the card the job
    /// exports plus the job's own name, which is exactly what the job's ARN names, so the
    /// export jobs of one card sit together in one stable order. See
    /// [`ModelCardExportJob`], at the end of this module with the rest of the model-card
    /// slice.
    pub model_card_export_jobs: BTreeMap<String, ModelCardExportJob>,
    /// Keyed by `AIWorkloadConfigName`. See [`AIWorkloadConfig`], at the end of this
    /// module with the rest of the AI-benchmarking workload-configuration slice.
    pub ai_workload_configs: BTreeMap<String, AIWorkloadConfig>,
    /// Keyed by `AIBenchmarkJobName`. See [`AIBenchmarkJob`], at the end of this module
    /// with the rest of the AI-benchmarking slice.
    pub ai_benchmark_jobs: BTreeMap<String, AIBenchmarkJob>,
    /// Keyed by `AIRecommendationJobName`. See [`AIRecommendationJob`], which lives with
    /// the rest of its slice in the [`ai_recommendation_job`] submodule.
    pub ai_recommendation_jobs: BTreeMap<String, AIRecommendationJob>,
    /// Keyed by `HubName`. See [`Hub`].
    pub hubs: BTreeMap<String, Hub>,
    /// Every stored hub content VERSION, keyed by [`HubContentKey`].
    pub hub_contents: BTreeMap<HubContentKey, HubContent>,
    /// Keyed by `TrialComponentName`, case-folded with [`entity_key`] (the name "is not
    /// case-sensitive" per its model doc), so one component cannot be stored twice under
    /// two spellings. See [`TrialComponent`], at the end of this module with the rest of
    /// the experiment-tracking slice.
    pub trial_components: BTreeMap<String, TrialComponent>,
    /// Which trial components are part of which trial: the
    /// `([entity_key] of TrialName, [entity_key] of TrialComponentName)` pair that IS one
    /// membership's whole identity, the way [`Self::associations`] keys a lineage edge by
    /// its two ends. A component may be part of several trials, and a trial holds many
    /// components, so the membership is an edge rather than a member of either record.
    ///
    /// Written by `AssociateTrialComponent` — the call that attaches a component to a
    /// trial — and read by `ListTrialComponents`, whose `TrialName` and `ExperimentName`
    /// filters are exactly "the components part of this trial" and "part of any trial of
    /// this experiment". The index lives here, beside both stores it relates, rather than
    /// in either of them: a component may be part of several trials and a trial holds many
    /// components, so neither record owns the membership.
    ///
    /// Bounded by [`TRIAL_COMPONENT_TRIAL_QUOTA`].
    pub trial_component_trials: BTreeSet<(String, String)>,
    /// Keyed by `ProcessingJobName`.
    pub processing_jobs: BTreeMap<String, ProcessingJob>,
    /// Keyed by `ComputeQuotaId` — the identifier SageMaker mints and every
    /// compute-quota operation names the resource by (its `Name` is unique only within
    /// one cluster, so the name is not a key). See [`ComputeQuota`], at the end of this
    /// module with the rest of the compute-allocation slice.
    pub compute_quotas: BTreeMap<String, ComputeQuota>,
    /// Keyed by `(Region, LineageGroupName)`: a lineage group is a REGIONAL resource
    /// (see [`lineage_group_arn`], "the ARN of the Region's default lineage group"), so
    /// the Region a lineage entity materialised it in is part of its identity and a
    /// reader signing for another Region does not find it. See [`LineageGroup`], at the
    /// end of this module with the rest of the lineage-group slice.
    pub lineage_groups: BTreeMap<(String, String), LineageGroup>,
    /// Keyed by the partner app's `app-...` id. See [`PartnerApp`], at the end of this
    /// module with the rest of the partner-AI-app slice.
    pub partner_apps: BTreeMap<String, PartnerApp>,
    /// The `PartnerAppType`s this account holds a partner-application subscription for.
    /// See [`ComAmazonawsSagemakerSagemakerState::subscribed_to_partner_app`], which is
    /// what `CreatePartnerApp` admits a type by. EMPTY for an account nothing has
    /// subscribed for: subscribing is a purchase in AWS Marketplace, which no operation
    /// SageMaker models performs, so nothing a client sends over the wire adds to this set
    /// — an in-process caller (a test, or a future AWS Marketplace integration of this
    /// emulator) is what grants one, exactly as an AWS account that has subscribed to
    /// nothing cannot create a partner app at all.
    pub partner_app_subscriptions: BTreeSet<String>,
    /// Keyed by `ImageName`. See [`Image`], at the end of this module with the rest of
    /// the custom-image slice.
    pub images: BTreeMap<String, Image>,
    /// Every stored image VERSION, keyed by `(ImageName, Version)` so the versions of
    /// one image sit together in ascending version order and the latest one is the last
    /// of that range. See [`ImageVersion`].
    pub image_versions: BTreeMap<(String, i32), ImageVersion>,
    /// Keyed by `ClusterName`. See [`Cluster`], at the end of this module with the rest
    /// of the HyperPod cluster slice.
    pub clusters: BTreeMap<String, Cluster>,
    /// Lineage associations, keyed by the `(SourceArn, DestinationArn)` pair that IS one
    /// association's identity — the only identifier `AddAssociation` and
    /// `DeleteAssociation` address one by. See [`Association`], at the end of this module
    /// with the rest of the lineage-association slice.
    pub associations: BTreeMap<(String, String), Association>,
    /// Every `CreateModelPackage` idempotency token this account has SPENT, oldest first:
    /// which identity it registered and the ARN it minted.
    ///
    /// Kept beside the packages rather than on them, because the question a retry asks
    /// outlives the record: a token already spent registers nothing a second time, even once
    /// the package it registered has been deleted, so a retry presenting it is refused
    /// ([`Self::model_package_registered_with_client_token`]). Bounded by
    /// [`MODEL_PACKAGE_CLIENT_TOKEN_HISTORY`], so a caller registering package after
    /// package cannot grow the store without end.
    pub model_package_client_tokens: Vec<SpentModelPackageToken>,
    /// The instant this account's SageMaker state came into being, i.e. when the store was
    /// constructed.
    ///
    /// What the resources an ACCOUNT holds without any call creating them report as their
    /// creation instant — today the Region's default lineage group
    /// ([`Self::default_lineage_group`]), which SageMaker gives every account and which no
    /// operation creates. Read on every report of such a resource rather than taken from
    /// the clock there, so the value is stable across reads.
    pub store_created_at: StoreEpoch,
}

/// The instant a [`ComAmazonawsSagemakerSagemakerState`] came into being.
///
/// Its own type because the store is `Default`-constructed by the engine: a bare
/// `SystemTime` field would default to the UNIX epoch, which is not when this account's
/// state began.
#[derive(Debug, Clone, Copy)]
pub struct StoreEpoch(pub SystemTime);

impl Default for StoreEpoch {
    fn default() -> Self {
        Self(SystemTime::now())
    }
}

/// One `CreateModelPackage` idempotency token this account has spent: the token, the
/// identity the call that spent it registered, and the ARN that call minted.
///
/// The identity is what scopes the record: a token reused for a DIFFERENT package is a fresh
/// call rather than a refusal about an unrelated one, so the name and the group are part of
/// it.
#[derive(Debug, Clone)]
pub struct SpentModelPackageToken {
    /// The `ClientToken` the call presented.
    pub token: String,
    /// The store key ([`model_package_key`]) that call registered, which is what resolves
    /// whether the package it minted is still held.
    pub key: String,
    /// The package name the retry has to ask for to be a replay of this call.
    pub name: String,
    /// The model package group the retry has to ask for, and `None` for an unversioned
    /// package.
    pub group: Option<String>,
    /// The ARN the call answered, which is what a refusal of a retry names.
    pub arn: String,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The account's Service Catalog portfolio status, as the modelled
    /// `SagemakerServicecatalogStatus` enum names it: its two members are `Enabled` and
    /// `Disabled` (`ops::types::SAGEMAKER_SERVICECATALOG_STATUS_VALUES`, generated from
    /// the model shape), and this is the one place either is named.
    pub fn servicecatalog_portfolio_status(&self) -> &'static str {
        if self.servicecatalog_portfolio_enabled {
            "Enabled"
        } else {
            "Disabled"
        }
    }

    /// Whether another model fits, i.e. the store is below [`MODEL_QUOTA`].
    pub fn models_below_quota(&self) -> bool {
        self.models.len() < MODEL_QUOTA
    }

    /// Whether another inference component fits, i.e. the store is below
    /// [`INFERENCE_COMPONENT_QUOTA`].
    pub fn inference_components_below_quota(&self) -> bool {
        self.inference_components.len() < INFERENCE_COMPONENT_QUOTA
    }

    /// Whether another endpoint configuration fits, i.e. the store is below
    /// [`ENDPOINT_CONFIG_QUOTA`].
    pub fn endpoint_configs_below_quota(&self) -> bool {
        self.endpoint_configs.len() < ENDPOINT_CONFIG_QUOTA
    }

    /// Whether another endpoint fits, i.e. the store is below [`ENDPOINT_QUOTA`].
    pub fn endpoints_below_quota(&self) -> bool {
        self.endpoints.len() < ENDPOINT_QUOTA
    }

    /// How many instances of `instance_type` the endpoints this account holds are already
    /// using — the utilisation its hosting quota is measured against.
    ///
    /// Read off the endpoints themselves rather than off the configurations they name, so
    /// what an endpoint holds is what it was created with and a deleted endpoint uses
    /// nothing.
    pub fn endpoint_instances_in_use(&self, instance_type: &str) -> i64 {
        self.endpoints
            .values()
            .filter_map(|endpoint| endpoint.instance_usage.get(instance_type))
            .sum()
    }

    /// Why this account cannot host `demand` on top of what its endpoints already hold, or
    /// `None` when every instance type it names is within quota.
    ///
    /// One instance type past its quota is enough to refuse the whole deployment, and the
    /// one reported is the first in type order, so the same request is always refused the
    /// same way. The sentence is [`account_quota_exceeded`]'s, counted in
    /// [`INSTANCES_UNIT`]: the limit, the utilisation and the delta, which is what a client
    /// reads to decide between asking for fewer instances and requesting an increase.
    pub fn hosting_quota_exceeded(&self, demand: &BTreeMap<String, i64>) -> Option<String> {
        demand.iter().find_map(|(instance_type, delta)| {
            let limit = endpoint_usage_quota(instance_type);
            let in_use = self.endpoint_instances_in_use(instance_type);
            (in_use + delta > limit).then(|| {
                account_quota_exceeded(
                    &endpoint_usage_quota_name(instance_type),
                    INSTANCES_UNIT,
                    limit,
                    in_use,
                    *delta,
                )
            })
        })
    }

    /// Whether another hub fits, i.e. the store is below [`HUB_QUOTA`].
    pub fn hubs_below_quota(&self) -> bool {
        self.hubs.len() < HUB_QUOTA
    }

    /// How many content VERSIONS the hub `hub_name` holds, across every
    /// `HubContentType`: what [`HUB_CONTENT_QUOTA`] bounds.
    pub fn hub_contents_held(&self, hub_name: &str) -> usize {
        self.hub_contents
            .keys()
            .filter(|key| key.hub_name == hub_name)
            .count()
    }

    /// Whether another hyperparameter tuning job fits, i.e. the store is below
    /// [`HYPER_PARAMETER_TUNING_JOB_QUOTA`].
    pub fn tuning_jobs_below_quota(&self) -> bool {
        self.tuning_jobs.len() < HYPER_PARAMETER_TUNING_JOB_QUOTA
    }

    /// Whether another pipeline fits, i.e. the store is below [`PIPELINE_QUOTA`].
    pub fn pipelines_below_quota(&self) -> bool {
        self.pipelines.len() < PIPELINE_QUOTA
    }

    /// Whether another pipeline execution fits, i.e. the store is below
    /// [`PIPELINE_EXECUTION_QUOTA`].
    pub fn pipeline_executions_below_quota(&self) -> bool {
        self.pipeline_executions.len() < PIPELINE_EXECUTION_QUOTA
    }

    /// The execution of `pipeline_name` the `ClientRequestToken` `token` started, if one
    /// is still stored.
    ///
    /// How `StartPipelineExecution` completes "no more than one time": a retry of a
    /// start call answers the execution that call started rather than starting a second
    /// one. The token is scoped to the pipeline, so the same token under another
    /// pipeline's name starts that pipeline's own execution.
    pub fn pipeline_execution_by_token(
        &self,
        pipeline_name: &str,
        token: &str,
    ) -> Option<&PipelineExecution> {
        self.pipeline_executions.values().find(|execution| {
            execution.pipeline_name == pipeline_name && execution.client_request_token == token
        })
    }

    /// Remove the pipeline stored under `name`, with everything that belongs to it, and
    /// answer the record that went — `None` when the account holds no such pipeline.
    ///
    /// `DeletePipeline` is observable AS a delete: recorded AWS makes the pipeline
    /// unreadable from the delete call on, so the record goes at once rather than
    /// lingering in a transient status for a later call to materialise.
    ///
    /// A pipeline's executions go with it: an execution belongs to the pipeline that ran
    /// it, so once the pipeline is gone its executions are no longer resolvable either —
    /// and leaving them would let a pipeline recreated under the freed name inherit the
    /// executions of the one deleted before it.
    pub fn remove_pipeline(&mut self, name: &str) -> Option<Pipeline> {
        let removed = self.pipelines.remove(name)?;
        self.pipeline_executions
            .retain(|_, execution| execution.pipeline_name != removed.name);
        Some(removed)
    }

    /// Whether another transform job fits, i.e. the store is below
    /// [`TRANSFORM_JOB_QUOTA`].
    pub fn transform_jobs_below_quota(&self) -> bool {
        self.transform_jobs.len() < TRANSFORM_JOB_QUOTA
    }

    /// Whether another MLflow app fits, i.e. the store is below [`MLFLOW_APP_QUOTA`].
    pub fn mlflow_apps_below_quota(&self) -> bool {
        self.mlflow_apps.len() < MLFLOW_APP_QUOTA
    }

    /// Whether another AppImageConfig fits, i.e. the store is below
    /// [`APP_IMAGE_CONFIG_QUOTA`].
    pub fn app_image_configs_below_quota(&self) -> bool {
        self.app_image_configs.len() < APP_IMAGE_CONFIG_QUOTA
    }

    /// Whether another model package fits, i.e. the store is below
    /// [`MODEL_PACKAGE_QUOTA`].
    pub fn model_packages_below_quota(&self) -> bool {
        self.model_packages.len() < MODEL_PACKAGE_QUOTA
    }

    /// The version number the next registration into `group` gets: one past the highest
    /// version the group currently holds, so a group's versions count up from 1 and each
    /// registration into it gets its own.
    pub fn next_model_package_version(&self, group: &str) -> i32 {
        self.model_packages
            .values()
            .filter(|package| package.group_name.as_deref() == Some(group))
            .filter_map(|package| package.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    /// Whether another code repository fits, i.e. the store is below
    /// [`CODE_REPOSITORY_QUOTA`].
    pub fn code_repositories_below_quota(&self) -> bool {
        self.code_repositories.len() < CODE_REPOSITORY_QUOTA
    }

    /// Whether another app fits, i.e. the store is below [`APP_QUOTA`].
    pub fn apps_below_quota(&self) -> bool {
        self.apps.len() < APP_QUOTA
    }

    /// Whether another model package group fits, i.e. the store is below
    /// [`MODEL_PACKAGE_GROUP_QUOTA`].
    pub fn model_package_groups_below_quota(&self) -> bool {
        self.model_package_groups.len() < MODEL_PACKAGE_GROUP_QUOTA
    }

    /// Whether another artifact fits, i.e. the store is below [`ARTIFACT_QUOTA`].
    pub fn artifacts_below_quota(&self) -> bool {
        self.artifacts.len() < ARTIFACT_QUOTA
    }

    /// The artifact called `name` in `region`, if one is stored.
    ///
    /// `CreateArtifact`'s `ArtifactName` is documented unique to an account in a Region,
    /// so this is what a create checks before taking a name. Artifacts are keyed by ARN
    /// (the name is optional), hence the scan over a store bounded by [`ARTIFACT_QUOTA`].
    pub fn artifact_named(&self, region: &str, name: &str) -> Option<&Artifact> {
        self.artifacts
            .values()
            .find(|artifact| artifact.region == region && artifact.name.as_deref() == Some(name))
    }

    /// The ARN of the artifact in `region` whose source URI is `source_uri`, if one is
    /// stored — the OLDEST such artifact when several share the URI.
    ///
    /// `DeleteArtifact` accepts either an `ArtifactArn` or a `Source`, and `ListArtifacts`
    /// filters artifacts by `SourceUri` alone: the URI is what identifies a source, so a
    /// source-addressed request resolves on it rather than on the optional
    /// `SourceTypes` beside it. Nothing stops two creates recording the same URI (the
    /// store is keyed by ARN), so the match is made deterministic by creation time, with
    /// the ARN breaking a tie. Same scan, and same reason, as [`Self::artifact_named`].
    pub fn artifact_arn_for_source(&self, region: &str, source_uri: &str) -> Option<String> {
        self.artifacts
            .values()
            .filter(|artifact| {
                artifact.region == region && artifact.source.source_uri == source_uri
            })
            .min_by(|left, right| {
                left.creation_time
                    .cmp(&right.creation_time)
                    .then_with(|| left.arn.cmp(&right.arn))
            })
            .map(|artifact| artifact.arn.clone())
    }

    /// Whether another processing job fits, i.e. the store is below
    /// [`PROCESSING_JOB_QUOTA`].
    pub fn processing_jobs_below_quota(&self) -> bool {
        self.processing_jobs.len() < PROCESSING_JOB_QUOTA
    }

    /// Whether another training job fits, i.e. the store is below
    /// [`TRAINING_JOB_QUOTA`].
    pub fn training_jobs_below_quota(&self) -> bool {
        self.training_jobs.len() < TRAINING_JOB_QUOTA
    }

    /// How much of the account's parallel-training-jobs quota its hyperparameter tuning
    /// jobs are using: the training jobs a tuning job launched that are still running.
    ///
    /// The quota is counted in TrainingJobs, so the utilisation is a count of training-job
    /// records — the ones a tuning job owns ([`TrainingJob::tuning_job_arn`]) and that have
    /// not yet reached a terminal status. Nothing in this emulator launches a training job
    /// on a tuning job's behalf, so an account that has only ever created training jobs
    /// itself is using none of this quota, and that is read off the store rather than
    /// assumed.
    pub fn parallel_training_jobs_in_use(&self) -> usize {
        self.training_jobs
            .values()
            .filter(|job| {
                job.tuning_job_arn.is_some() && job.status == TRAINING_STATUS_IN_PROGRESS
            })
            .count()
    }

    /// The ARN of the model package the call that presented `token` registered under the same
    /// identity (`name` plus `group`), if this account has spent that token.
    ///
    /// This is what makes `CreateModelPackage` idempotent: a token already spent registers
    /// nothing a second time — neither a second package nor a second version — so a retry
    /// presenting it is refused, naming the registration it already stands for. The record
    /// outlives the package, so a retry presenting a token whose registration has since been
    /// deleted is refused too rather than quietly registering a fresh version under a token
    /// already used.
    ///
    /// The match is scoped to the identity the retry asks for, so a token reused for a
    /// DIFFERENT package is a fresh call rather than a refusal naming an unrelated package.
    pub fn model_package_registered_with_client_token(
        &self,
        token: &str,
        name: &str,
        group: Option<&str>,
    ) -> Option<&str> {
        self.model_package_client_tokens
            .iter()
            .rev()
            .find(|spent| {
                spent.token == token && spent.name == name && spent.group.as_deref() == group
            })
            .map(|spent| spent.arn.as_str())
    }

    /// Record that `token` was spent registering the package `key` under `arn`, so a retry
    /// presenting it is refused rather than registering again.
    ///
    /// The oldest records are dropped past [`MODEL_PACKAGE_CLIENT_TOKEN_HISTORY`], which a
    /// caller cannot tell from a token whose deduplication window has passed. A call that
    /// presented no token records nothing.
    pub fn remember_model_package_client_token(
        &mut self,
        token: Option<&str>,
        key: &str,
        name: &str,
        group: Option<&str>,
        arn: &str,
    ) {
        let Some(token) = token else {
            return;
        };
        self.model_package_client_tokens.push(SpentModelPackageToken {
            token: token.to_string(),
            key: key.to_string(),
            name: name.to_string(),
            group: group.map(ToString::to_string),
            arn: arn.to_string(),
        });
        if self.model_package_client_tokens.len() > MODEL_PACKAGE_CLIENT_TOKEN_HISTORY {
            let overflow =
                self.model_package_client_tokens.len() - MODEL_PACKAGE_CLIENT_TOKEN_HISTORY;
            self.model_package_client_tokens.drain(..overflow);
        }
    }

    /// Whether another model card fits, i.e. the store is below [`MODEL_CARD_QUOTA`].
    pub fn model_cards_below_quota(&self) -> bool {
        self.model_cards.len() < MODEL_CARD_QUOTA
    }

    /// Whether another model card export job fits, i.e. the store is below
    /// [`MODEL_CARD_EXPORT_JOB_QUOTA`].
    pub fn model_card_export_jobs_below_quota(&self) -> bool {
        self.model_card_export_jobs.len() < MODEL_CARD_EXPORT_JOB_QUOTA
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerSagemakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerSagemakerState>)
}

/// The default time-to-live `config` states for the records of a feature group's
/// OnlineStore, in seconds, and `None` for a configuration that states none (or one whose
/// `TtlDuration` names no unit, no value, or a unit the model's `TtlDurationUnit` enum
/// does not declare).
///
/// Read through the ONE conversion of a Feature Store `TtlDuration`
/// ([`::service_common::feature_store_groups::ttl_duration_seconds`]), so the duration
/// this service records for a group and the one the FeatureStore Runtime data plane reads
/// off a request cannot be converted two ways.
pub fn online_store_default_ttl_seconds(
    config: &crate::ops::types::OnlineStoreConfig,
) -> Option<f64> {
    let ttl = config.ttl_duration.as_ref()?;
    ::service_common::feature_store_groups::ttl_duration_seconds(
        ttl.unit.as_deref()?,
        ttl.value?,
    )
}

/// The SHARED feature-group directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A feature group is DEFINED here (`CreateFeatureGroup`) and USED by the SageMaker
/// FeatureStore Runtime data plane, whose own API carries none of that definition: a
/// `PutRecord` names a feature group and nothing else. So the data plane can resolve that
/// name — and know which feature of a record identifies it and which carries its event
/// time — only because this service RECORDED it, in ONE directory under one shared type
/// ([`::service_common::feature_store_groups::FeatureGroupDirectory`]). The engine keys
/// state by TYPE, so a copy of its own inside [`ComAmazonawsSagemakerSagemakerState`]
/// would be invisible to that reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// `FeatureGroupDirectory::forget` belongs to `DeleteFeatureGroup`, which calls it as
/// soon as a deletion is asked for: "Data cannot be accessed from the OnlineStore
/// immediately after DeleteFeatureGroup is called", so the directory stops resolving the
/// name from that instant even though the group itself reports `Deleting` until the clock
/// completes its removal.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn feature_group_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::feature_store_groups::FeatureGroupDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::feature_store_groups::FeatureGroupDirectory,
            >,
        )
}

/// The SHARED job directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A job of a reinforcement-fine-tuning category is CREATED here (`CreateJob`) and its
/// rollouts are SERVED by the AgenticRFT job runtime data plane, whose own API carries no
/// job definition and binds no call that creates one: a sampling request names a job ARN
/// in a header and nothing else. So that data plane can resolve the ARN only because this
/// service RECORDED the job, in ONE directory under one shared type
/// ([`::service_common::agentic_rft_jobs::JobDirectory`]). The engine keys state by TYPE,
/// so a copy of its own inside [`ComAmazonawsSagemakerSagemakerState`] would be invisible
/// to that reader.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// `JobDirectory::forget` belongs to `DeleteJob`, which is still a scaffold stub
/// answering 501 (its own issue): nothing in this emulator deletes a job yet, so nothing
/// calls it yet.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn job_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::agentic_rft_jobs::JobDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::agentic_rft_jobs::JobDirectory>)
}

/// The message a handler refuses with when the service is mounted on an engine that
/// holds no store for it. Unreachable in the assembled emulator (the router mounts
/// every service on the one shared `InMemoryEngine`); stated once so no handler
/// invents its own wording — and never by building a private engine, which would
/// drop the request's writes.
pub const NO_STATE: &str =
    "SageMaker state is unavailable: this service is not mounted on the shared in-memory engine";

/// The message SageMaker's `ResourceInUse` carries for the resources whose OWN refusal of
/// a name already taken is this sentence.
///
/// It carries nothing from the request: for these resources the service answers the same
/// sentence whatever name the caller asked for, and does not repeat the name back. It is
/// NOT a service-wide default — a name collision is worded per RESOURCE TYPE, and a create
/// whose resource has its own sentence (`CreateAIWorkloadConfig`, `CreateFeatureGroup`,
/// `CreateHub`, `CreateImage`) states that one in its own handler rather than borrowing
/// this. Stated once here so the creates that DO share this sentence — the app image
/// config and the AI job family — cannot word the same fact differently.
pub const RESOURCE_IN_USE_MESSAGE: &str = "The ID or Name specified is already in use.";

/// Whether `value` is a well-formed AWS region token: lowercase alphanumerics and
/// hyphens, at most `emulator_telemetry::policy::REGION_MAX_LEN` (32) bytes.
///
/// An ARN's region component is positional, so a scope carrying `:` or `/` would mint a
/// structurally malformed ARN. Anything outside the grammar is not accepted as a region.
fn is_region_token(value: &str) -> bool {
    is_dns_label(value, 32)
}

/// Whether `label` is a DNS label of at most `max_len` bytes: non-empty, and lowercase
/// alphanumerics and hyphens only. The one definition of that grammar in this crate —
/// [`is_region_token`], which bounds a credential scope's region, and the host-label
/// check `CreatePresignedMlflowAppUrl` applies to an ARN id it splices into a host name,
/// differ only in the cap they pass.
pub(crate) fn is_dns_label(label: &str, max_len: usize) -> bool {
    !label.is_empty()
        && label.len() <= max_len
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region this request's SigV4 credential scope names, or [`DEFAULT_REGION`] when
/// the request is unsigned or its scope region is not a region token. Every ARN this
/// service mints takes its region from here, so a client signing for one region never
/// reads back an ARN naming another.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Record a call that changed this service's state, or a status transition it
/// materialised, so an emulator run is auditable the way CloudTrail makes a real one.
///
/// Carries only request metadata: the operation, the resource ARN it changed and the
/// access-key id the REQUEST presented (`emulator_server::extract_akid`). The
/// emulator's own credentials and every secret stay out of it.
pub fn record_state_change(
    operation: &str,
    resource_arn: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    tracing::info!(
        operation,
        resource = resource_arn,
        access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
        "sagemaker state change"
    );
}

/// `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` — the one ARN form this
/// service mints, for the resource kinds it owns. `name` is the segment the modelled
/// ARN shape requires: the resource's name for most kinds, and the minted
/// `[a-z0-9]{12}` id for a cluster (see [`mint_cluster_id`]).
pub fn resource_arn(region: &str, kind: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("{kind}/{name}"))
}

/// The RESOURCE portion of `arn` when `arn` is one [`resource_arn`] mints in `region` for
/// this account, or `None` when it names anything else.
///
/// The read side of [`resource_arn`], and the one place the fixed segments are checked:
/// a resolver that takes an identifier in ARN form (an `…NameOrArn` member, or an
/// ARN-typed member naming a resource) must refuse an ARN of another partition, service,
/// Region or account rather than addressing whatever local record shares its trailing
/// path. The six fields themselves are read by `service_common::arn`, the ONE ARN parse
/// this workspace has, so no service splits an ARN of its own again; only the `aws`
/// partition is accepted, because that is the one every ARN this emulator mints carries
/// and a partition is a closed world — an `aws-cn` ARN names no resource in `aws`.
fn arn_resource<'v>(arn: &'v str, region: &str) -> Option<&'v str> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_SERVICE)?;
    (parsed.partition == "aws"
        && parsed.region == region
        && parsed.account == ::emulator_server::EMULATOR_ACCOUNT_ID)
        .then_some(parsed.resource)
}

/// How SageMaker's lineage, pipeline and experiment-trial readers state a resource the
/// account does not hold: the entity kind, then the identifier RESOLVED TO ITS ARN in
/// single quotes, then `does not exist.`
///
/// These resources are addressed by name OR by ARN (`ExperimentEntityNameOrArn`,
/// `PipelineNameOrArn`) — or, for a trial, by name alone — and the recorded refusals
/// report the ARN the request resolved to whichever form arrived, so the identifier a
/// consumer extracts from the message is the same one it would read off a successful
/// call. The single quotes are part of that rule: a parser that cuts on them has to find
/// them, and they are what delimits the ARN when the resource name carries no separator
/// of its own.
///
/// Stated once here so the reader and the delete of one resource kind, and the kinds
/// sharing the template, cannot word the same fact differently. The hosting resources
/// (model, endpoint configuration, endpoint) use their OWN recorded template instead —
/// `Could not find <kind> "<identifier as the caller sent it>".` — so they do not come
/// through here.
pub fn does_not_exist(entity: &str, arn: &str) -> String {
    entity_not_found(entity, arn, ArnQuoting::SingleQuoted)
}

/// The entity label [`does_not_exist`] names a lineage action by.
pub const ACTION_ENTITY: &str = "Action";

/// The entity label [`does_not_exist`] names a lineage artifact by.
pub const ARTIFACT_ENTITY: &str = "Artifact";

/// How this service states an `ArtifactName` the Region already has an artifact under:
/// the name the request sent, then the ARN of the artifact holding it.
///
/// `ArtifactName` is documented unique to an account in a Region, so every call that
/// takes a name for an artifact — the create that mints one and the update that renames
/// one — refuses a name already taken, and they word that one fact here rather than
/// twice.
pub fn artifact_name_in_use(name: &str, holder_arn: &str) -> String {
    format!("The artifact name \"{name}\" is already in use by \"{holder_arn}\".")
}

/// The entity label [`does_not_exist`] names a lineage context by.
pub const CONTEXT_ENTITY: &str = "Context";

/// The entity label [`does_not_exist`] names ONE END of a lineage association by.
///
/// An `AssociationEntityArn` may name any of five entity kinds (experiment,
/// experiment-trial-component, artifact, action, context), and the operations that resolve
/// one do not care which: the refusal names the end generically rather than guessing the
/// kind out of the ARN it could not resolve.
pub const ASSOCIATION_END_ENTITY: &str = "Resource";

/// The entity label [`does_not_exist`] names a lineage group by — the resource TYPE name,
/// one word, as the service spells it there (the same way
/// [`MODEL_PACKAGE_GROUP_ENTITY`] spells its kind): `LineageGroup '<arn>' does not
/// exist.`
pub const LINEAGE_GROUP_ENTITY: &str = "LineageGroup";

/// The entity label [`does_not_exist`] names a pipeline by.
pub const PIPELINE_ENTITY: &str = "Pipeline";

/// The ARN resource kind a pipeline carries, i.e. the `<kind>` in [`resource_arn`], as
/// the modelled `PipelineArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:([0-9]{12}|aws):pipeline/`). Named once here
/// so the create mints, the readers resolve and the refusals match the one kind.
pub const PIPELINE_ARN_KIND: &str = "pipeline";

/// The entity label [`does_not_exist`] names one execution of a pipeline by.
pub const PIPELINE_EXECUTION_ENTITY: &str = "Pipeline execution";

/// The entity label [`does_not_exist`] names an experiment trial by.
pub const TRIAL_ENTITY: &str = "Trial";

/// The entity label [`does_not_exist`] names an experiment by.
pub const EXPERIMENT_ENTITY: &str = "Experiment";

/// The entity label [`does_not_exist`] names an experiment trial component by — two
/// words, as the sentence spells the kind the ARN segment hyphenates
/// ([`TRIAL_COMPONENT_KIND`]).
pub const TRIAL_COMPONENT_ENTITY: &str = "Trial component";

/// How every operation that cannot resolve the experiment a request names states it: the
/// [`does_not_exist`] sentence over the experiment's ARN, resolved from the requested
/// `ExperimentName` against the Region the request signed for.
///
/// The same template as [`trial_does_not_exist`], because an experiment is addressed the
/// same way its trials are — by NAME only (`ExperimentEntityName`) — so the ARN in the
/// message is one [`resource_arn`] resolves rather than the name echoed, and a consumer
/// cuts the same identifier out of the refusal that a successful call would have answered.
pub fn experiment_does_not_exist(region: &str, experiment_name: &str) -> String {
    does_not_exist(
        EXPERIMENT_ENTITY,
        &resource_arn(region, EXPERIMENT_KIND, experiment_name),
    )
}

/// The ARN resource kind an experiment trial carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrialArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`), and named once
/// here so the create mints and every reader matches the same ARN.
pub const TRIAL_KIND: &str = "experiment-trial";

/// How every operation that cannot resolve the trial a request names states it: the
/// [`does_not_exist`] sentence over the trial's ARN, resolved from the requested
/// `TrialName` against the Region the request signed for.
///
/// A trial is addressed by NAME only (`ExperimentEntityName`), so the ARN in the message
/// is always one [`resource_arn`] resolves — and it is resolved, rather than the name
/// echoed, so the identifier a consumer cuts out of the refusal is the same one a
/// successful call would have answered. Stated once here so the readers, the delete and
/// the two membership operations cannot word the same fact differently.
pub fn trial_does_not_exist(region: &str, trial_name: &str) -> String {
    does_not_exist(TRIAL_ENTITY, &resource_arn(region, TRIAL_KIND, trial_name))
}

/// How every operation that cannot resolve the trial component a request names states it:
/// the [`does_not_exist`] sentence over the component's ARN, resolved from the requested
/// `TrialComponentName` against the Region the request signed for.
///
/// The same template as [`trial_does_not_exist`], for the same reason: a component is
/// addressed by name (or, for the reader, by the ARN that name resolves to), so the
/// identifier the sentence names is the one a successful call would have answered. Stated
/// once here so the reader, the delete and the two membership operations cannot word the
/// same fact differently.
pub fn trial_component_does_not_exist(region: &str, trial_component_name: &str) -> String {
    does_not_exist(
        TRIAL_COMPONENT_ENTITY,
        &resource_arn(region, TRIAL_COMPONENT_KIND, trial_component_name),
    )
}

/// How the `does not exist.` sentence renders the ARN it names — the ONE thing the two
/// recorded templates that use it differ in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnQuoting {
    /// As the lineage and pipeline family records it: `'<arn>'`.
    SingleQuoted,
    /// As the algorithm and model-package-group family records it: `<arn>`.
    Bare,
}

/// The one `<Entity> <identifier> does not exist.` sentence, with `quoting` deciding how
/// the resolved ARN is rendered.
///
/// Private, and reached through [`does_not_exist`] / [`entity_does_not_exist`], so a call
/// site names the recorded family it belongs to rather than passing a bare flag — and so
/// the sentence itself is written once and the two shapes cannot drift apart.
fn entity_not_found(entity: &str, arn: &str, quoting: ArnQuoting) -> String {
    let identifier = match quoting {
        ArnQuoting::SingleQuoted => format!("'{arn}'"),
        ArnQuoting::Bare => arn.to_string(),
    };
    format!("{entity} {identifier} does not exist.")
}

/// How SageMaker's algorithm and model-package-group operations state a resource the
/// account does not hold: the entity label as the service words it, then the identifier
/// RESOLVED TO ITS ARN — UNQUOTED — then `does not exist.`
///
/// Same sentence as [`does_not_exist`] and the same reason for reporting the resolved ARN
/// (both kinds are addressed by name or by ARN, so the identifier a consumer extracts is
/// the one a successful call would report); the single quotes that family's recorded
/// refusals carry are what differs, which is why the quoting is [`ArnQuoting`]'s to decide
/// and not each caller's.
///
/// Stated once here so the readers, the deletes and the creates that reference one of
/// these resources cannot word the same fact differently.
pub fn entity_does_not_exist(entity: &str, arn: &str) -> String {
    entity_not_found(entity, arn, ArnQuoting::Bare)
}

/// The entity label [`entity_does_not_exist`] names an algorithm by.
pub const ALGORITHM_ENTITY: &str = "Algorithm";

/// The entity label [`entity_does_not_exist`] names a model package group by — one word,
/// as the service spells it there, where the group's own conflict refusal spells it out.
pub const MODEL_PACKAGE_GROUP_ENTITY: &str = "ModelPackageGroup";

/// The ARN resource kind a model package group carries, i.e. the `<kind>` in
/// [`resource_arn`], as the modelled `ModelPackageGroupArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:model-package-group/[\S]{1,2048}$`).
/// Named once here so the create mints, the readers resolve and the refusals match the
/// one kind.
pub const MODEL_PACKAGE_GROUP_ARN_KIND: &str = "model-package-group";

/// How SageMaker states that something a request named could not be found at all:
/// `Cannot find`, then WHAT was looked for, then the ARN it resolves to — BARE, with no
/// quoting around it and NO closing period.
///
/// A second not-found sentence beside [`does_not_exist`], and the one the recorded
/// refusals of this service's code-repository and model-package-group-policy surfaces
/// carry. Written once here so every subject stated through it — a resource kind, or a
/// resource POLICY of a resource — words the one sentence identically, and so the
/// identifier a consumer cuts out of the message is in the same place whichever subject
/// it names.
pub fn cannot_find(subject: &str, arn: &str) -> String {
    format!("Cannot find {subject}: {arn}")
}

/// What [`cannot_find`] names a code repository as — one word, as the service spells that
/// resource kind there.
pub const CODE_REPOSITORY_SUBJECT: &str = "CodeRepository";

/// The ARN resource kind a code repository carries, i.e. the `<kind>` in
/// [`resource_arn`], as the modelled `CodeRepositoryArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:code-repository/`). Named once
/// here so the create mints, the readers resolve and the refusals match the one kind.
pub const CODE_REPOSITORY_ARN_KIND: &str = "code-repository";

/// What [`cannot_find`] names a model package group as — spelled in WORDS there, where the
/// group's own `does not exist.` sentence spells it as one
/// ([`MODEL_PACKAGE_GROUP_ENTITY`]).
///
/// The subject a PUT of a group policy names: the group it would attach a document to is
/// not one this account holds, which is a different fact from the policy's absence a read
/// reports ([`resource_policy_not_found`]) — so the two states a caller tells apart by
/// text stay distinguishable.
pub const MODEL_PACKAGE_GROUP_SUBJECT: &str = "Model Package Group";

/// What [`cannot_find`] names a resource POLICY of a model package group as. The `for`
/// belongs to the subject: the ARN the sentence ends with names the group, not the policy.
const RESOURCE_POLICY_SUBJECT: &str = "resource policy for";

/// How SageMaker states that there is no resource policy to report or release for a model
/// group: the [`cannot_find`] sentence naming the policy.
///
/// A name no group is stored under and a group carrying no policy are the SAME answer —
/// there is no policy — so the sentence names the ARN the request's `ModelPackageGroupName`
/// resolves to in the Region it signed for rather than distinguishing the two. Stated once
/// here so the read and the delete of a group policy word the same fact the same way.
pub fn resource_policy_not_found(group_arn: &str) -> String {
    cannot_find(RESOURCE_POLICY_SUBJECT, group_arn)
}

/// How SageMaker's hosting operations state a resource the account does not hold: the
/// resource kind as the service words it, then the identifier AS THE CALLER SENT IT, in
/// double quotes.
///
/// A model, an endpoint configuration and an endpoint are addressed by NAME, and the
/// recorded refusals quote that name — never an ARN the service synthesised from it,
/// which is an identifier the caller never supplied and which a consumer extracting the
/// offending resource from the message cannot match against what it sent.
///
/// Stated once here so every operation that resolves one of these references — the
/// readers, the deletes, and the creates that reference another resource — words the
/// same fact the same way.
pub fn could_not_find(kind: &str, identifier: &str) -> String {
    format!("Could not find {kind} \"{identifier}\".")
}

/// The kind label [`could_not_find`] names a model by.
pub const MODEL_KIND_LABEL: &str = "model";

/// The kind label [`could_not_find`] names an endpoint configuration by.
pub const ENDPOINT_CONFIG_KIND_LABEL: &str = "endpoint configuration";

/// The kind label [`could_not_find`] names an endpoint by.
pub const ENDPOINT_KIND_LABEL: &str = "endpoint";

/// The kind label [`could_not_find`] names a job configuration schema version by.
pub const JOB_CONFIG_SCHEMA_KIND_LABEL: &str = "job config schema version";

/// The entity label [`entity_does_not_exist`] names a model package by — one word, the
/// way the service spells it for this resource.
pub const MODEL_PACKAGE_ENTITY: &str = "ModelPackage";

/// The ARN resource kind a model package carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `ModelPackageArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:model-package/`).
pub const MODEL_PACKAGE_KIND: &str = "model-package";

/// How every operation that cannot resolve the model package a request names states it:
/// the [`entity_does_not_exist`] sentence over the package's ARN.
///
/// The identifier is the ARN [`arn_from_name_or_arn`] decides rather than the name echoed,
/// so a bare name, a `<group>/<version>` pair and an ARN all report the one identifier a
/// successful call would have answered with. Stated once here so the readers and the
/// deletes of this resource cannot word the same fact differently.
pub fn model_package_not_found(region: &str, identifier: &str) -> String {
    entity_does_not_exist(
        MODEL_PACKAGE_ENTITY,
        &arn_from_name_or_arn(region, MODEL_PACKAGE_KIND, identifier),
    )
}

/// The `<Entity> '<identifier>' not found` clause SageMaker's AI benchmarking family
/// states an unresolvable reference with, written once so the two sentences built on it
/// cannot drift apart.
fn not_found_clause(entity: &str, identifier: &str) -> String {
    format!("{entity} '{identifier}' not found")
}

/// How SageMaker's AI benchmarking family states a resource the account does not hold: the
/// entity label as the service words it, then the identifier AS THE CALLER SENT IT in
/// single quotes, then `not found.`
///
/// An AI workload configuration, a benchmark job and a recommendation job are addressed by
/// NAME (a job reference is an `AIEntityName`; a configuration reference also admits its
/// ARN), and the recorded refusals quote the identifier the REQUEST carried — never an ARN
/// the service synthesised from it, which is a value the caller never supplied and which a
/// consumer extracting the offending resource from the message cannot match against what
/// it sent.
///
/// Stated once here so the readers, the stops and the deletes of this family word the same
/// fact the same way; a CREATE that cannot resolve a reference of this family states the
/// scope it searched instead ([`ai_entity_not_found_for_account`]).
pub fn ai_entity_not_found(entity: &str, identifier: &str) -> String {
    format!("{}.", not_found_clause(entity, identifier))
}

/// How a CREATE of this family states a referenced resource the account does not hold: the
/// [`ai_entity_not_found`] clause, then the scope the service searched — the account the
/// request was signed for.
///
/// The extra clause is the recorded difference between resolving a reference on the way to
/// creating something and reading the resource itself: a create's caller named a
/// configuration it expected the account to hold, so the refusal says where it was looked
/// for.
pub fn ai_entity_not_found_for_account(entity: &str, identifier: &str) -> String {
    format!("{} for this account.", not_found_clause(entity, identifier))
}

/// The internal service the AI family's DELETE operations front, as the rendering they
/// relay names it.
const AI_INNER_SERVICE: &str = "SageMakerLumen";

/// The status that internal service raised the refusal with, as its rendering states it —
/// the client-error status, the same one this endpoint answers the call with.
const AI_INNER_SERVICE_STATUS: u16 = 400;

/// How many attempts the relayed call took: one. A client error is not retried, so the
/// attempt that raised it is the only one the rendering can count.
const AI_INNER_SERVICE_ATTEMPTS: u8 = 1;

/// How the AI family's DELETE operations state a resource the account does not hold: the
/// family's [`ai_entity_not_found`] sentence, then the tail that names where the failure
/// came from.
///
/// These deletes front an internal service, and what they report is THAT service's own
/// rendering of the failure — the sentence, the service it was raised by, the status it was
/// raised with, the request id of that inner call and how many attempts it took. The
/// family's reads and creates answer the bare sentence, so the tail belongs to the deletes
/// alone.
///
/// The request id is the only per-run field: it is minted per call from the emulator's one
/// request-id mint (`service_common::query_error_envelope::new_request_id`, a fresh v4
/// UUID), because nothing in the request carries one to echo and the inner call is this
/// call's own — so it is indistinguishable from the id the framework's own renderers
/// report, and no value of one recorded run is baked in.
pub fn ai_entity_not_found_relayed(entity: &str, identifier: &str) -> String {
    format!(
        "{} (Service: {AI_INNER_SERVICE}, Status Code: {AI_INNER_SERVICE_STATUS}, \
         Request ID: {}) (SDK Attempt Count: {AI_INNER_SERVICE_ATTEMPTS})",
        ai_entity_not_found(entity, identifier),
        ::service_common::query_error_envelope::new_request_id(),
    )
}

/// The entity label the AI family's not-found sentences name an AI workload configuration
/// by — one word, as the service spells the resource in these messages.
pub const AI_WORKLOAD_CONFIG_ENTITY: &str = "AIWorkloadConfig";

/// The entity label the AI family's not-found sentences name an AI benchmark job by.
pub const AI_BENCHMARK_JOB_ENTITY: &str = "AIBenchmarkJob";

/// How SageMaker's training-job readers state a job this account does not hold.
///
/// A FIXED sentence naming no identifier at all — the recorded wording for this resource,
/// where the hosting operations quote the name the caller sent ([`could_not_find`]) and
/// the lineage family reports a resolved ARN ([`does_not_exist`]). A reader that answers
/// it has stored nothing under the name the request carried.
pub const REQUESTED_RESOURCE_NOT_FOUND: &str = "Requested resource not found.";

/// How SageMaker's presigned-URL call states a notebook instance this account does not
/// hold: the resource kind as the service spells it, the ARN the request's
/// `NotebookInstanceName` resolves to, and `does not exist` with no closing period.
///
/// A separate template from the `RecordNotFound. Notebook Instance Arn: <arn>` sentence the
/// READERS of this resource answer, because the service words the two differently — the
/// same split the processing-job family has between its read and its update templates.
#[must_use]
pub fn notebook_instance_does_not_exist(arn: &str) -> String {
    format!("Notebook Instance {arn} does not exist")
}

/// How SageMaker states a hyperparameter tuning job the account does not hold: the name
/// AS THE CALLER SENT IT, in the sentence that tells it to send another one.
///
/// The recorded wording for this resource — neither the hosting family's quoted
/// identifier ([`could_not_find`]) nor the lineage family's resolved ARN
/// ([`does_not_exist`]) — stated once so every operation that resolves a tuning-job
/// reference words the same fact the same way.
pub fn tuning_job_not_found(name: &str) -> String {
    format!("Amazon SageMaker can't find a tuning job called {name}. Type another name.")
}

/// How SageMaker words a HyperPod cluster this account does not hold, for the operations
/// whose SUBJECT is something other than the cluster itself — a node of it, or a policy
/// hanging off it.
///
/// A FIXED sentence carrying NOTHING from the request: the recorded refusals of those
/// operations state the condition and no identifier, so no refusal leaks an ARN the caller
/// never supplied or a name it can only match against its own input. The sentence is the
/// whole message — the class of the failure travels in the error CODE, so no name of a
/// condition or type is prefixed onto it. Stated once here so every one of them words the
/// same fact the same way. The operations whose subject IS the cluster name it instead
/// ([`cluster_not_found`]).
pub const CLUSTER_NOT_FOUND: &str = "Specified cluster not found";

/// How SageMaker words a HyperPod cluster this account does not hold, for the operations
/// that address the cluster ITSELF: the identifier the request carried, unquoted, inside
/// the recorded sentence — which ends with no period.
///
/// `ClusterName` is a `ClusterNameOrArn`, so the identifier is whichever form arrived,
/// echoed rather than resolved: a cluster's ARN carries the minted cluster id rather than
/// its name, so resolving the name of a cluster that does not exist would report an
/// identifier nobody supplied, and a caller reading the refusal matches it against what it
/// sent. Stated once here so the cluster readers, the delete and the node-inventory and
/// node-batch operations cannot word the same fact differently.
pub fn cluster_not_found(identifier: &str) -> String {
    format!("Cluster with name {identifier} not found")
}

/// How the HyperPod node lookup words a node the named cluster does not own.
///
/// Fixed text, and DIFFERENT from [`CLUSTER_NOT_FOUND`]: the two
/// unknown-identifier cases of one request — the cluster, and the node inside it — are
/// what a caller tells apart by the refusal text, since both carry the same code.
pub const CLUSTER_NODE_NOT_FOUND: &str = "Specified cluster node not found";

/// The one `<Entity> with arn <arn> could not be found` sentence — how the HyperPod
/// task-governance resources (the cluster policy and the compute quota) state one this
/// account does not hold.
///
/// Private, and reached through [`cluster_scheduler_config_not_found`] /
/// [`compute_quota_not_found`], so a call site names the resource it refuses rather than
/// spelling the sentence again. Both resources are addressed by the id SageMaker minted,
/// and both refusals name the ARN that id resolves to — unquoted, with no trailing period.
fn arn_could_not_be_found(entity: &str, arn: &str) -> String {
    format!("{entity} with arn {arn} could not be found")
}

/// The ARN resource kind a cluster policy carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `ClusterSchedulerConfigArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:cluster-scheduler-config/`).
pub const CLUSTER_SCHEDULER_CONFIG_KIND: &str = "cluster-scheduler-config";

/// The entity label [`arn_could_not_be_found`] names a cluster policy by, as the service
/// words it — not the modelled shape name, and not this crate's `cluster policy` prose.
const CLUSTER_SCHEDULER_CONFIG_ENTITY: &str = "Cluster scheduler config";

/// How every operation that cannot resolve the cluster policy a request names states it:
/// the [`arn_could_not_be_found`] sentence over the policy's ARN, resolved from the
/// requested `ClusterSchedulerConfigId` against the Region the request signed for.
pub fn cluster_scheduler_config_not_found(region: &str, config_id: &str) -> String {
    arn_could_not_be_found(
        CLUSTER_SCHEDULER_CONFIG_ENTITY,
        &resource_arn(region, CLUSTER_SCHEDULER_CONFIG_KIND, config_id),
    )
}

/// The ARN resource kind a compute quota carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `ComputeQuotaArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:compute-quota/`).
pub const COMPUTE_QUOTA_KIND: &str = "compute-quota";

/// The entity label [`arn_could_not_be_found`] names a compute quota by, as the service
/// words it.
const COMPUTE_QUOTA_ENTITY: &str = "Compute Quota";

/// How every operation that cannot resolve the compute quota a request names states it:
/// the [`arn_could_not_be_found`] sentence over the quota's ARN, resolved from the
/// requested `ComputeQuotaId` against the Region the request signed for.
pub fn compute_quota_not_found(region: &str, quota_id: &str) -> String {
    arn_could_not_be_found(
        COMPUTE_QUOTA_ENTITY,
        &resource_arn(region, COMPUTE_QUOTA_KIND, quota_id),
    )
}

/// The ARN resource kind a JumpStart hub carries, i.e. the `<kind>` in [`resource_arn`],
/// as the modelled `HubNameOrArn` pattern spells it
/// (`^(arn:[a-z0-9-\.]{1,63}:sagemaker:\w+(?:-\w+)+:(\d{12}|aws):hub\/)?`).
pub const HUB_KIND: &str = "hub";

/// How every operation that cannot resolve the hub a request names states it: the entity,
/// the hub's NAME, and `does not exist.`
///
/// A hub is addressed by `HubNameOrArn` and its ARN carries the hub's name, so both forms
/// resolve to the one name this sentence reports — never an ARN, which is not what the
/// service names here. Stated once so the readers, the delete and the content operations
/// that resolve a hub cannot word the same fact differently.
pub fn hub_not_found(hub_name: &str) -> String {
    format!("Hub with name {hub_name} does not exist.")
}

/// The ARN resource kind a hub content carries, i.e. the `<kind>` in [`resource_arn`], as
/// the modelled `SageMakerPublicHubContentArn` pattern spells it
/// (`^arn:…:sagemaker:\w+(?:-\w+)+:aws:hub-content\/…`).
pub const HUB_CONTENT_KIND: &str = "hub-content";

/// The ARN of ONE version of one hub content:
/// `arn:aws:sagemaker:<region>:<account>:hub-content/<hub>/<type>/<name>/<version>`.
///
/// The hub, the `HubContentType` and the name are what identify a hub content (they are
/// the three members every hub-content operation requires) and the version segment is what
/// makes the ARN name one VERSION of it, which is what the store holds one record per.
pub fn hub_content_arn(
    region: &str,
    hub_name: &str,
    content_type: &str,
    content_name: &str,
    version: &str,
) -> String {
    resource_arn(
        region,
        HUB_CONTENT_KIND,
        &format!("{hub_name}/{content_type}/{content_name}/{version}"),
    )
}

/// How this service refuses a `SageMakerPublicHubContentArn` that resolves to no stored
/// hub content: recorded wire behaviour, carried on `ValidationException` and HTTP 400.
///
/// The sentence carries NOTHING from the request — the service answers the same words
/// whatever ARN arrived — and it is answered BEFORE the hub a reference would be added to
/// is resolved, so a request that names neither a stored source nor an existing hub is
/// refused for the source. Stated once here so every operation that resolves a referenced
/// source cannot word the same fact differently.
pub const SOURCE_ARN_NOT_FOUND: &str = "Requested source ARN does not exist.";

/// The most content versions one hub holds, after which a write that would add another is
/// refused with the modelled `ResourceLimitExceeded`. The emulator's OWN bound, for the
/// reason given on [`MODEL_QUOTA`], and per HUB because that is the resource whose contents
/// grow.
pub const HUB_CONTENT_QUOTA: usize = 1_000;

/// The ARN resource kind a model card carries, i.e. the `<kind>` in [`resource_arn`], as
/// the modelled `ModelCardArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:model-card/`).
pub const MODEL_CARD_KIND: &str = "model-card";

/// How every operation that cannot resolve the model card a request names states it: the
/// entity as the service spells it — one word — the card's ARN, and `does not exist` with
/// NO trailing period.
///
/// A card is addressed by name or by ARN and the refusal reports the ARN
/// [`arn_from_name_or_arn`] decides, so the identifier a consumer cuts out of the message is
/// the one a successful call would have answered with. Stated once here so the readers, the
/// update, the delete and the export jobs over a card word the same fact the same way.
pub fn model_card_not_found(region: &str, identifier: &str) -> String {
    format!(
        "Modelcard {} does not exist",
        arn_from_name_or_arn(region, MODEL_CARD_KIND, identifier)
    )
}

/// An app's ARN: `...:app/<DomainId>/<owner>/<AppType>/<AppName>`.
///
/// A Studio app is identified by five members, so its ARN names all of them — a single
/// `AppName` would not distinguish two apps of different types under one user profile.
/// Each component is carried exactly as the request supplied it, which is safe for a
/// positional ARN segment because every one of them is already refused at the boundary
/// unless it matches its modelled constraint: `AppName`, `UserProfileName` and
/// `SpaceName` are `len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`, `DomainId`
/// is `len 0..63, pattern ^d-(-*[a-z0-9]){1,61}$` and `AppType` is an enum
/// (`smithy_query(query="operation", operation="CreateApp")`), so none can carry a `:`,
/// a `/` or a control character and the result always matches `AppArn`'s
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:app/`. The real service composes
/// the ARN from the same five members, so its `len 0..256` bound binds the same inputs.
pub fn app_arn(region: &str, key: &AppKey) -> String {
    resource_arn(
        region,
        "app",
        &format!(
            "{}/{}/{}/{}",
            key.domain_id,
            key.owner.name(),
            key.app_type,
            key.app_name
        ),
    )
}

/// The resource policy a resource nothing has shared carries: an IAM policy document with
/// no statements, which is the empty JSON object.
///
/// `GetLineageGroupPolicy` reports a document for every lineage group it finds — both of
/// its output members are optional, yet a group nobody has granted access to still has a
/// policy, and an IAM policy document is a JSON object (see
/// [`resource_policy_violation`]). So this is the document a group starts with, rather than
/// an absent member a caller would have to read as "there is no policy".
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// The ARN resource kind a lineage group carries, i.e. the `<kind>` in [`resource_arn`].
/// Fixed by `LineageGroupArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:lineage-group/`), and named once
/// here so the ARN mint and the readers that resolve an ARN back to a name cannot drift.
pub const LINEAGE_GROUP_KIND: &str = "lineage-group";

/// The ARN of one named lineage group, as `LineageGroupArn` carries it. The single owner
/// of the `lineage-group` ARN resource kind, so a listing and a describe cannot drift
/// apart.
pub fn named_lineage_group_arn(region: &str, name: &str) -> String {
    resource_arn(region, LINEAGE_GROUP_KIND, name)
}

/// The ARN of the Region's default lineage group (see [`default_lineage_group_name`]), as
/// `LineageGroupArn` carries it for an entity that belongs to no other group.
pub fn lineage_group_arn(region: &str) -> String {
    named_lineage_group_arn(region, &default_lineage_group_name(region))
}

/// Mint the opaque id an ARN carries for a resource whose ARN does NOT contain the
/// resource's name: 32 lowercase hex digits, the form SageMaker's `artifact/<id>` and
/// `mlflow-app/<id>` ARNs take. The caller never supplies it — such a resource is named
/// by its ARN, so the id is the server's to generate, and it is derived from nothing in
/// the request. (`MlflowAppName` is `len 1..256` while `MlflowAppArn` is `len 1..128`,
/// and the `CreateMlflowApp` model doc says the name "is not part of the ... ARN": an
/// id-bearing ARN is what keeps a minted ARN inside the modelled length whatever the
/// name's length.)
///
/// Emulator-grade and deliberately NOT cryptographic: the bits come from SHA-256 over
/// three sources, which make an id unique within a run but no more unpredictable than
/// what they carry — a `RandomState` hash of the counter (std's hash-DoS seed, not a
/// CSPRNG), a monotonic counter, so two ids minted in the same nanosecond on different
/// threads differ by construction, and the wall clock in nanoseconds, so ids do not
/// repeat across processes. Such an id is an identifier inside an authenticated store,
/// never a secret; nothing here may be relied on as one.
pub fn mint_opaque_resource_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, entropy.to_le_bytes());
    ::sha2::Digest::update(&mut digest, counter.to_le_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_le_bytes());
    let hashed = ::sha2::Digest::finalize(digest);

    // 16 bytes of the digest, hex-rendered: the 32-digit form the real ARNs carry.
    ::hex::encode(&hashed[..16])
}

/// The digest this emulator reports for a container image, in the form `ImageDigest`
/// carries (`len 0..72`, pattern `^[Ss][Hh][Aa]256:[0-9a-fA-F]{64}$`).
///
/// It is the emulator's OWN identifier for the image reference, not a digest any registry
/// resolved: nothing here pulls an image, so there is no manifest to hash and the value
/// cannot match what ECR would report for that reference. What it does keep is every
/// property a caller can check without a registry — the modelled form, one digest per
/// reference, the same digest on every read of it, and practically never a collision
/// between two references — because it is a SHA-256 over the reference itself and over
/// nothing else, so it depends on neither the clock nor the request that asked for it.
pub fn derived_image_digest(image: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(image.as_bytes());
    format!("sha256:{}", ::hex::encode(digest))
}

/// Fill in what SageMaker itself resolves for one container of an inference
/// specification: the digest of the image the container names, and whether its model data
/// is a training checkpoint — which a request that says nothing leaves `false`.
///
/// Called on the WRITE path, so the stored record is the materialised container every
/// reader of it replays, rather than each reader defaulting again.
fn materialise_container(container: &mut ModelPackageContainerDefinition) {
    if container.image_digest.is_none() {
        container.image_digest = container.image.as_deref().map(derived_image_digest);
    }
    container.is_checkpoint = Some(container.is_checkpoint.unwrap_or(false));
}

/// [`materialise_container`] for every container of an inference specification.
pub fn materialise_inference_specification(specification: &mut InferenceSpecification) {
    for container in &mut specification.containers {
        materialise_container(container);
    }
}

/// [`materialise_container`] for every container of an ADDITIONAL inference
/// specification, whose containers are the same shape and are resolved the same way.
pub fn materialise_additional_inference_specifications(
    specifications: &mut [AdditionalInferenceSpecificationDefinition],
) {
    for specification in specifications {
        for container in &mut specification.containers {
            materialise_container(container);
        }
    }
}

/// The `ContainerMode` a model's container hosts its models in when the request named
/// none: one model per container, as the model's `ContainerMode` enum spells it.
pub const CONTAINER_MODE_SINGLE_MODEL: &str = "SingleModel";

/// Fill in what SageMaker itself resolves for a model's container: whether it "hosts a
/// single model or multiple models" (`ContainerDefinition$Mode`), which a request that
/// says nothing leaves at one model per container.
pub fn materialise_container_mode(container: &mut ContainerDefinition) {
    if container.mode.is_none() {
        container.mode = Some(CONTAINER_MODE_SINGLE_MODEL.to_string());
    }
}

/// Fill in what SageMaker itself resolves for an algorithm's training specification: the
/// digest of the training image, and each training channel's `IsRequired` — which a
/// request that says nothing leaves `false`.
pub fn materialise_training_specification(specification: &mut TrainingSpecification) {
    if specification.training_image_digest.is_none() {
        specification.training_image_digest =
            Some(derived_image_digest(&specification.training_image));
    }
    for channel in &mut specification.training_channels {
        channel.is_required = Some(channel.is_required.unwrap_or(false));
    }
}

/// The resource NAME an `ArnOrName`-typed member addresses.
///
/// Several SageMaker readers accept either a bare name or the resource's full ARN for the
/// same resource (the `ArnOrName` and `HubNameOrArn` shapes, the latter modelled
/// `^(arn:…:hub\/)?<name>$`), and the store is keyed by the name — so an ARN is
/// reduced to the resource portion after its final `/`. A value that is not an ARN is
/// already a name and is returned unchanged.
///
/// KNOWN ASYMMETRY, recorded here rather than fixed in one operation: this reduction does
/// NOT check the ARN's partition, Region or account, so an ARN naming another Region or
/// account still resolves the locally stored resource of the same name, and an ARN this
/// service mints for it carries the REQUEST's Region rather than the one the caller
/// supplied. The resolvers for ARN-typed members are strict instead (they go through
/// `arn_resource`, which is what [`entity_name_from_name_or_arn`] and
/// [`model_card_export_job_key_from_arn`] check with). Tightening this one changes every
/// `ArnOrName` reader in the service at once — `DescribeModelCard`, `UpdateModelCard`,
/// `CreateModelCardExportJob`, the hub readers — so it belongs to a change that covers
/// them together, with their refusal wording, rather than to any single operation.
pub fn name_from_arn_or_name(value: &str) -> &str {
    match value.strip_prefix(ARN_PREFIX) {
        Some(_) => value.split('/').next_back().unwrap_or(value),
        None => value,
    }
}

/// The IAM user name the request's SigV4 credential names, or a fixed `emulator`
/// principal when the request carries no usable access key id.
///
/// The access key id is request text and the value is spliced into an ARN, whose segments
/// are positional, so anything not shaped like an access key id is replaced rather than
/// interpolated. `extract_akid` performs no verification — the emulator verifies signatures
/// at its boundary.
fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
    let shaped_like_an_access_key_id = !akid.is_empty()
        && akid.len() <= 128
        && akid.chars().all(|character| character.is_ascii_alphanumeric());
    if shaped_like_an_access_key_id {
        akid
    } else {
        "emulator".to_string()
    }
}

/// The `UserContext` SageMaker records as the creator of a resource.
///
/// The emulator serves no Studio domain, so the Studio members (`UserProfileArn`,
/// `UserProfileName`, `DomainId`) are ABSENT rather than empty: a caller cannot tell an
/// empty profile name from a missing one. The IAM identity is the one the request signed
/// with, reported as both the ARN of that identity and the unique id AWS gives it — the id
/// is a function of the ARN ([`service_common::caller_principal::aws_unique_id`]), so it is
/// stable across reads and across restarts and two identities practically never share one.
/// `SourceIdentity` stays absent: it is set only when a role session carries one, and no
/// call here does.
pub fn caller_user_context(ctx: &smithy_server_runtime::OperationContext<'_>) -> UserContext {
    let principal = caller_principal(ctx);
    // IAM is global, so the region segment is empty.
    let arn = ::emulator_server::emulator_arn("iam", "", &format!("user/{principal}"));
    UserContext {
        user_profile_arn: None,
        user_profile_name: None,
        domain_id: None,
        iam_identity: Some(IamIdentity {
            principal_id: Some(::service_common::caller_principal::aws_unique_id(
                IAM_USER_ID_PREFIX,
                &arn,
            )),
            arn: Some(arn),
            source_identity: None,
        }),
    }
}

/// The type prefix an IAM USER's unique id carries, from IAM's "unique identifier
/// prefixes" table — the identity kind this emulator's long-term credential speaks for.
const IAM_USER_ID_PREFIX: &str = "AIDA";

/// A stored `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_and_nanos(
            i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
            since_epoch.subsec_nanos(),
        ),
        // A host clock set before the epoch: keep the sign rather than clamping the
        // instant to zero and reporting a time the resource was not created at. Built
        // from whole seconds plus a non-negative nanos remainder, so this branch is as
        // lossless as the one above.
        Err(before_epoch) => {
            let magnitude = before_epoch.duration();
            let secs = i64::try_from(magnitude.as_secs()).unwrap_or(i64::MAX);
            match magnitude.subsec_nanos() {
                0 => ::aws_smithy_types::DateTime::from_secs(-secs),
                nanos => ::aws_smithy_types::DateTime::from_secs_and_nanos(
                    -secs - 1,
                    1_000_000_000 - nanos,
                ),
            }
        }
    }
}

/// A modelled `Timestamp` as a comparable key, so a stored time and a filter bound
/// order against each other without going through a lossy float.
pub fn timestamp_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

// ---------------------------------------------------------------------------
// Feature Store control plane
//
// Everything the feature-group slice of this service needs — its constants, its record,
// its status transition and its quota helper — is kept together in this ONE section, the
// way the lineage-action and AI-workload-configuration slices below are, so one
// resource's state reads as one block. Its shapes are named by their full
// `crate::ops::types::` path rather than through a `use`, so the slice carries no import
// of its own.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Notebook instances
//
// Everything `CreateNotebookInstance`, `StopNotebookInstance` and
// `DescribeNotebookInstance` need, kept in one
// block: the record the store holds, the spans and defaults those operations apply,
// and the two values the create derives (the Jupyter host and the network-interface id).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Model cards. `CreateModelCard` stores one here and `DescribeModelCard` replays it;
// the store itself is `ComAmazonawsSagemakerSagemakerState::model_cards` above, and the
// export jobs `CreateModelCardExportJob` starts against a card sit beside it in
// `model_card_export_jobs`. Kept together at the end of the file so a model-card change
// touches one span of it.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Lineage actions (`CreateAction` / `DescribeAction` / `DeleteAction`).
//
// Kept together at the end of the file, rather than interleaved with the stores
// above, so this slice is one block to read — and one block to merge.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The AI-benchmarking workload-configuration slice: `CreateAIWorkloadConfig`,
// `DescribeAIWorkloadConfig` and the `DeleteAIWorkloadConfig` that releases the record.
// Kept together at the end of this module, rather than spread through the blocks above,
// so this one resource kind reads as one slice.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The AI benchmark job slice: what `CreateAIBenchmarkJob` writes and
// `DescribeAIBenchmarkJob` replays, beside the workload-configuration slice the jobs
// reference.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The compute-allocation slice: the definitions `ListComputeQuotas` lists and
// `DescribeComputeQuota` replays. The pagination-token refusal those listings share
// with every other paginated listing here lives with the token vocabulary at the top
// of this module ([`STALE_PAGE_TOKEN`]).
//
// Kept together at the end of the file, rather than interleaved with the blocks
// above, so this slice is one block to read — and one block to merge.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The lineage-context slice: `CreateContext`, `DescribeContext`, `UpdateContext` and
// `DeleteContext`. Kept together at the end of this module, rather than spread through
// the blocks above, so this one resource kind reads as one slice (the same shape the
// AI-benchmarking slice above uses).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SageMaker Partner AI Apps: the store `ListPartnerApps` pages. Kept together at the
// end of this module, the way the lineage-action and workload-configuration slices
// above are, so this resource kind reads as one slice — and merges as one block.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The SageMaker HyperPod cluster slice: everything `CreateCluster` writes and
// `ListClusters` reports, kept together at the end of this module the way the
// lineage-action and AI-workload slices above are, with its own quota, its own
// clock-owed span and its own inherent `impl` block on the state struct.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The lineage-group slice: the record `DescribeLineageGroup` reports, and the two
// accessors the reader and the lineage-entity creates reach it through. Kept at the end
// of the module, with its own inherent `impl` block, the way the lineage-action and
// workload-configuration slices above are.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The MLflow tracking-server slice: `CreateMlflowTrackingServer` and
// `DescribeMlflowTrackingServer`. Kept together as its own slice, rather than spread
// through the blocks around it, so this one resource kind reads as one block.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The experiment-tracking slice: `CreateTrialComponent` and `DescribeTrialComponent`.
// Kept together at the end of this module, the way the AI-benchmarking slice above is,
// rather than spread through the blocks at the top.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The experiment-management slice: `CreateExperiment`, `DescribeExperiment` and the
// `DeleteExperiment` that releases one. Kept
// together as one span, the way the slices above are, so this one resource kind reads
// as one block. The store key is [`entity_key`], shared with the experiment-tracking
// slice: an `ExperimentEntityName` is unique per account and not case-sensitive.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The custom-image slice: `CreateImage` / `CreateImageVersion`, the `DeleteImage` that
// releases an image with every version of it, and the two
// `Describe*` readers. Kept together at the end of this module, the way the
// lineage-action, model-card and notebook-instance slices are, so one slice's
// records, constants and helpers read as one unit.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The lineage-association slice: `AddAssociation` and the `DeleteAssociation` that
// releases one. Kept together at the end of this module, the way the lineage-action,
// model-card and custom-image slices are, so one slice's record, constants and helpers
// read as one unit.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The HyperPod scheduling, experiment-trial, model-card-export and partner-app
// control-plane slice's own lookups and store bounds. They sit in their OWN inherent
// `impl` block at the end of this module — the convention the slices above follow —
// rather than in the first one, so a slice added beside them edits its own block
// instead of the one every other slice also edits.
// ---------------------------------------------------------------------------
