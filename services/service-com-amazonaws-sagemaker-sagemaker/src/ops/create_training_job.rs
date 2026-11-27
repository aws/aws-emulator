// SPDX-License-Identifier: CC0-1.0

//! @op CreateTrainingJob
//! IMPLEMENTED: admits a training job into `crate::state` — refusing an
//! `AlgorithmSpecification` that does not name the training algorithm exactly once (the
//! `AlgorithmName` reference is RESOLVED FIRST, so a request naming an algorithm this
//! account does not hold is refused for that whatever else the spec carries) with a 400
//! `ValidationException`, a role SageMaker cannot assume or an input data source that names
//! no object that role can list with the same class, a `TrainingJobName` already in use with
//! the modelled `ResourceInUse` and a request past the store's bound with the modelled
//! `ResourceLimitExceeded` — and stores it as `InProgress` together with the instant the
//! clock owes its terminal status. A `ResourceConfig` that names no ML compute to launch
//! (see [`crate::state::resource_config_refusal`]) is refused with the same
//! `ValidationException` class, and one asking for capacity past an account quota — the ML
//! storage volume per instance, or the instances of one type — with the modelled
//! `ResourceLimitExceeded`. The accepted request is kept as sent, but for the
//! hyperparameters the service itself does not keep (see
//! [`crate::state::retained_hyper_parameters`]) and the members the SERVICE supplies for an
//! absent one (see [`materialise_job`], which also resolves the metric set of the algorithm
//! the job names). This is the ONLY file to touch for this
//! operation.
//!
//! ## The accepted job is RUN, server-side
//!
//! The run is this emulator's own ([`crate::state::run_training_job`]): it reads the
//! training data the request's channels name out of Amazon S3 under the execution role,
//! fits a model on those records, builds the model artifact for
//! `OutputDataConfig.S3OutputPath` out of what the job's CONTAINER left in `/opt/ml/model`
//! and measures the error it reports, named per channel and per the metric the algorithm's
//! learning task is evaluated by ([`crate::state::TrainingAlgorithm`]). It is performed HERE,
//! before this handler answers, and its outcome is RECORDED on the stored job rather than
//! revealed: the terminal status is owed by the clock, which
//! [`crate::state::TrainingJob::settle_status`] materialises, so a caller that describes the
//! job straight after creating it still observes `InProgress`. A run that could not read its
//! data or write its artifact leaves the job's modelled `Failed` status with the reason
//! naming that condition — never a `Completed` status for a run that produced nothing.
//!
//! The stages of the work are revealed in the same way, laid out from the instant the run
//! REPORTED ([`crate::state::TrainingJob::record_run`]) so none of them has gone by before
//! this call answers: the run records the `SecondaryStatus` phases it went through and the
//! clock materialises them in order ([`crate::state::TrainingJob::settle_status`]), so a
//! caller polling the job observes `Downloading`, `Training` and `Uploading` before the
//! terminal status, and `SecondaryStatusTransitions` reports every stage the job was in. Each
//! of those stages is in effect for a whole [`crate::state::TRAINING_JOB_PHASE_SPAN`], which is
//! a SHARE of the span the whole walk out of `InProgress` is owed in
//! ([`crate::state::TRAINING_JOB_ENDS_AFTER`]) rather than a span of its own: the job is one leg
//! of a flow that goes on to register the model and host it on an endpoint, and that flow waits
//! out every one of those clock-owed lifecycles inside one budget of its own. The
//! artifact the run built appears under the output location once the stage that UPLOADS it is
//! over, and the checkpoints it produced from the stage that TRAINS, which this call arms for
//! the instants the clock owes them ([`crate::state::publish_when_due`]), so a client that
//! polls `SecondaryStatus` to decide when the artifact exists never finds it before the job
//! says so. The stage that runs the CONTAINER is in effect for as long as that container's own
//! program stated its work took, bounded by how much of that work the clock reveals
//! ([`crate::state::MAX_REVEALED_CONTAINER_WORK`]) so the whole lifecycle stays inside a window
//! a client's poll loop can traverse; a job whose `StoppingCondition.MaxRuntimeInSeconds`
//! falls inside the runtime that work took is ended on that cap instead, with the
//! `Stopped`/`MaxRuntimeExceeded` pair the model groups together.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateTrainingJob::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateTrainingJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")
//! ```
//!
//! Starts a model training job. After training completes, SageMaker saves the resulting
//! model artifacts to an Amazon S3 location that you specify. If you choose to host your
//! model using SageMaker hosting services, you can use the resulting model artifacts as
//! part of the model. You can also use the artifacts in a machine learning service other
//! than SageMaker, provided that you know how to use them for inference. In the request
//! body, you provide the following: AlgorithmSpecification - Identifies the training
//! algorithm to use. HyperParameters - Specify these algorithm-specific parameters to
//! enable the estimation of model parameters during training. Hyperparameters can be tuned
//! to optimize this learning process. For a list of hyperparameters for each training
//! algorithm provided by SageMaker, see Algorithms. Do not include any security-sensitive
//! information including account access IDs, secrets, or tokens in any hyperparameter
//! fields. As part of the shared responsibility model, you are responsible for any
//! potential exposure, unauthorized access, or compromise of your sensitive data if caused
//! by security-sensitive information included in the request hyperparameter variable or
//! plain text fields. InputDataConfig - Describes the input required by the training job
//! and the Amazon S3, EFS, or FSx location where it is stored. OutputDataConfig -
//! Identifies the Amazon S3 bucket where you want SageMaker to save the results of model
//! training. ResourceConfig - Identifies the resources, ML compute instances, and ML
//! storage volumes to deploy for model training. In distributed training, you specify more
//! than one instance. EnableManagedSpotTraining - Optimize the cost of training machine
//! learning models by up to 80% by using Amazon EC2 Spot instances. For more information,
//! see Managed Spot Training. RoleArn - The Amazon Resource Name (ARN) that SageMaker
//! assumes to perform tasks on your behalf during model training. You must grant this role
//! the necessary permissions so that SageMaker can successfully complete model training.
//! StoppingCondition - To help cap training costs, use MaxRuntimeInSeconds to set a time
//! limit for training. Use MaxWaitTimeInSeconds to specify how long a managed spot training
//! job has to complete. Environment - The environment variables to set in the Docker
//! container. Do not include any security-sensitive information including account access
//! IDs, secrets, or tokens in any environment fields. As part of the shared responsibility
//! model, you are responsible for any potential exposure, unauthorized access, or
//! compromise of your sensitive data if caused by security-sensitive information included
//! in the request environment variable or plain text fields. RetryStrategy - The number of
//! times to retry the job when the job fails due to an InternalServerError. For more
//! information about SageMaker, see How It Works.
//!
//! ## Input
//!
//! * `AlgorithmSpecification` (AlgorithmSpecification) — The registry path of the Docker
//!   image that contains the training algorithm and algorithm-specific metadata, including
//!   the input mode. For more information about algorithms provided by SageMaker, see
//!   Algorithms. For information about providing your own algorithms, see Using Your Own
//!   Algorithms with Amazon SageMaker.
//! * `CheckpointConfig` (CheckpointConfig) — Contains information about the output location
//!   for managed spot training checkpoint data.
//! * `DebugHookConfig` (DebugHookConfig) — Configuration information for the Amazon SageMaker
//!   Debugger hook parameters, metric and tensor collections, and storage paths. To learn
//!   more about how to configure the DebugHookConfig parameter, see Use the SageMaker and
//!   Debugger Configuration API Operations to Create, Update, and Debug Your Training Job.
//! * `DebugRuleConfigurations` (DebugRuleConfigurations, len 0..20) — Configuration
//!   information for Amazon SageMaker Debugger rules for debugging output tensors.
//! * `EnableInterContainerTrafficEncryption` (Boolean) — To encrypt all communications
//!   between ML compute instances in distributed training, choose True. Encryption provides
//!   greater security for distributed training, but training might take longer. How long it
//!   takes depends on the amount of communication between compute instances, especially if
//!   you use a deep learning algorithm in distributed training. For more information, see
//!   Protect Communications Between ML Compute Instances in a Distributed Training Job.
//! * `EnableManagedSpotTraining` (Boolean) — To train models using managed spot training,
//!   choose True. Managed spot training provides a fully managed and scalable infrastructure
//!   for training machine learning models. this option is useful when training jobs can be
//!   interrupted and when there is flexibility when the training job is run. The complete and
//!   intermediate results of jobs are stored in an Amazon S3 bucket, and can be used as a
//!   starting point to train models incrementally. Amazon SageMaker provides metrics and logs
//!   in CloudWatch. They can be used to see when managed spot training jobs are running,
//!   interrupted, resumed, or completed.
//! * `EnableNetworkIsolation` (Boolean) — Isolates the training container. No inbound or
//!   outbound network calls can be made, except for calls between peers within a training
//!   cluster for distributed training. If you enable network isolation for training jobs that
//!   are configured to use a VPC, SageMaker downloads and uploads customer data and model
//!   artifacts through the specified VPC, but the training container does not have network
//!   access.
//! * `Environment` (TrainingEnvironmentMap, len 0..100) — The environment variables to set in
//!   the Docker container. Do not include any security-sensitive information including
//!   account access IDs, secrets, or tokens in any environment fields. As part of the shared
//!   responsibility model, you are responsible for any potential exposure, unauthorized
//!   access, or compromise of your sensitive data if caused by security-sensitive information
//!   included in the request environment variable or plain text fields.
//! * `ExperimentConfig` (ExperimentConfig) — Associates a SageMaker job as a trial component
//!   with an experiment and trial. Specified when you call the following APIs:
//!   CreateProcessingJob CreateTrainingJob CreateTransformJob
//! * `HyperParameters` (HyperParameters, len 0..100) — Algorithm-specific parameters that
//!   influence the quality of the model. You set hyperparameters before you start the
//!   learning process. For a list of hyperparameters for each training algorithm provided by
//!   SageMaker, see Algorithms. You can specify a maximum of 100 hyperparameters. Each
//!   hyperparameter is a key-value pair. Each key and value is limited to 256 characters, as
//!   specified by the Length Constraint. Do not include any security-sensitive information
//!   including account access IDs, secrets, or tokens in any hyperparameter fields. As part
//!   of the shared responsibility model, you are responsible for any potential exposure,
//!   unauthorized access, or compromise of your sensitive data if caused …
//! * `InfraCheckConfig` (InfraCheckConfig) — Contains information about the infrastructure
//!   health check configuration for the training job.
//! * `InputDataConfig` (InputDataConfig, len 1..20) — An array of Channel objects. Each
//!   channel is a named input source. InputDataConfig describes the input data and its
//!   location. Algorithms can accept input data from one or more channels. For example, an
//!   algorithm might have two channels of input data, training_data and validation_data. The
//!   configuration for each channel provides the S3, EFS, or FSx location where the input
//!   data is stored. It also provides information about the stored data: the MIME type,
//!   compression method, and whether the data is wrapped in RecordIO format. Depending on the
//!   input mode that the algorithm supports, SageMaker either copies input data files from an
//!   S3 bucket to a local directory in the Docker container, or …
//! * `MlflowConfig` (MlflowConfig) — The MLflow configuration using SageMaker managed MLflow.
//! * `ModelPackageConfig` (ModelPackageConfig) — The configuration for the model package.
//! * `OutputDataConfig` (OutputDataConfig) **required** — Specifies the path to the S3
//!   location where you want to store model artifacts. SageMaker creates subfolders for the
//!   artifacts.
//! * `ProfilerConfig` (ProfilerConfig) — Configuration information for Amazon SageMaker
//!   Debugger system monitoring, framework profiling, and storage paths.
//! * `ProfilerRuleConfigurations` (ProfilerRuleConfigurations, len 0..20) — Configuration
//!   information for Amazon SageMaker Debugger rules for profiling system and framework
//!   metrics.
//! * `RemoteDebugConfig` (RemoteDebugConfig) — Configuration for remote debugging. To learn
//!   more about the remote debugging functionality of SageMaker, see Access a training
//!   container through Amazon Web Services Systems Manager (SSM) for remote debugging.
//! * `ResourceConfig` (ResourceConfig) — The resources, including the ML compute instances
//!   and ML storage volumes, to use for model training. ML storage volumes store model
//!   artifacts and incremental states. Training algorithms might also use ML storage volumes
//!   for scratch space. If you want SageMaker to use the ML storage volume to store the
//!   training data, choose File as the TrainingInputMode in the algorithm specification. For
//!   distributed training algorithms, specify an instance count greater than 1.
//! * `RetryStrategy` (RetryStrategy) — The number of times to retry the job when the job
//!   fails due to an InternalServerError.
//! * `RoleArn` (RoleArn, len 20..2048, pattern
//!   ^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) **required** — The Amazon
//!   Resource Name (ARN) of an IAM role that SageMaker can assume to perform tasks on your
//!   behalf. During model training, SageMaker needs your permission to read input data from
//!   an S3 bucket, download a Docker image that contains training code, write model artifacts
//!   to an S3 bucket, write logs to Amazon CloudWatch Logs, and publish metrics to Amazon
//!   CloudWatch. You grant permissions for all of these tasks to an IAM role. For more
//!   information, see SageMaker Roles. To be able to pass this role to SageMaker, the caller
//!   of this API must have the iam:PassRole permission.
//! * `ServerlessJobConfig` (ServerlessJobConfig) — The configuration for serverless training
//!   jobs.
//! * `SessionChainingConfig` (SessionChainingConfig) — Contains information about
//!   attribute-based access control (ABAC) for the training job.
//! * `StoppingCondition` (StoppingCondition) — Specifies a limit to how long a model training
//!   job can run. It also specifies how long a managed Spot training job has to complete.
//!   When the job reaches the time limit, SageMaker ends the training job. Use this API to
//!   cap model training costs. To stop a job, SageMaker sends the algorithm the SIGTERM
//!   signal, which delays job termination for 120 seconds. Algorithms can use this 120-second
//!   window to save the model artifacts, so the results of training are not lost.
//! * `Tags` (TagList, len 0..50) — An array of key-value pairs. You can use tags to
//!   categorize your Amazon Web Services resources in different ways, for example, by
//!   purpose, owner, or environment. For more information, see Tagging Amazon Web Services
//!   Resources. Do not include any security-sensitive information including account access
//!   IDs, secrets, or tokens in any tags. As part of the shared responsibility model, you are
//!   responsible for any potential exposure, unauthorized access, or compromise of your
//!   sensitive data if caused by any security-sensitive information included in the request
//!   tag variable or plain text fields.
//! * `TensorBoardOutputConfig` (TensorBoardOutputConfig) — Configuration of storage locations
//!   for the Amazon SageMaker Debugger TensorBoard output data.
//! * `TrainingJobName` (TrainingJobName, len 1..63, pattern
//!   ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$) **required** — The name of the training job. The
//!   name must be unique within an Amazon Web Services Region in an Amazon Web Services
//!   account.
//! * `VpcConfig` (VpcConfig) — A VpcConfig object that specifies the VPC that you want your
//!   training job to connect to. Control access to and from your training container by
//!   configuring the VPC. For more information, see Protect Training Jobs by Using an Amazon
//!   Virtual Private Cloud.
//!
//! ## Output
//!
//! * `TrainingJobArn` (TrainingJobArn, len 0..256, pattern
//!   ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:training-job/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$)
//!   **required** — The Amazon Resource Name (ARN) of the training job.
//!
//! ## Errors
//!
//! * `ResourceInUse` (HTTP 400) — Resource being accessed is in use.
//! * `ResourceLimitExceeded` (HTTP 400) — You have exceeded an SageMaker resource limit. For
//!   example, you might have too many training jobs created.
//! * `ResourceNotFound` (HTTP 400) — Resource being access is not found.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateTrainingJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemaker-sagemaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateTrainingJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.sagemaker#CreateTrainingJob")]
#[derive(Debug)]
pub enum CreateTrainingJobErrorModeled {
    ResourceInUse(ResourceInUse),
    ResourceLimitExceeded(ResourceLimitExceeded),
    ResourceNotFound(ResourceNotFound),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_training_job::CreateTrainingJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateTrainingJobError = crate::scaffolded::Scaffolded<CreateTrainingJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateTrainingJobErrorModeled> for CreateTrainingJobError {
    fn from(modeled: CreateTrainingJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateTrainingJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateTrainingJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateTrainingJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// A 400 `ValidationException`: the refusal of a request this operation models no error
/// for. Every rule it answers is one the model states in prose rather than in a
/// constraint trait, so the centralized validation walk cannot have refused it already.
fn refuse(message: impl Into<String>) -> CreateTrainingJobError {
    crate::scaffolded::Scaffolded::validation(message.into())
}

/// The `CompressionType` of a channel whose data is not compressed, as that modelled
/// enum spells it: "If training data is compressed, the compression type. The default
/// value is None." (`Channel$CompressionType`.)
const NOT_COMPRESSED: &str = "None";

/// The `RecordWrapperType` of a channel whose objects the service does not wrap, as that
/// modelled enum spells it (`smithy_query(query="shape",shape="RecordWrapper")`: `None` |
/// `RecordIO`).
const NOT_WRAPPED: &str = "None";

/// The `S3DataDistribution` a channel's S3 source gets when the request states none: the
/// whole dataset on each instance ("If you want SageMaker to replicate the entire dataset
/// on each ML compute instance that is launched for model training, specify
/// FullyReplicated" — `S3DataSource$S3DataDistributionType`).
const FULLY_REPLICATED: &str = "FullyReplicated";

/// The `OutputCompressionType` of a job's model output when the request states none:
/// "Defaults to gzip" (`OutputDataConfig$CompressionType`), spelled as that enum spells
/// it.
const OUTPUT_COMPRESSED_WITH_GZIP: &str = "GZIP";

/// Fill in what SageMaker itself supplies for an accepted training job, so the stored
/// request IS the job every reader replays rather than only the members the client
/// happened to send.
///
/// Each value below is the one the model documents for an absent member; a member the
/// request carried is left exactly as it arrived. `algorithm_metrics` is the metric set of
/// the ALGORITHM the job names, when it names a registered one.
fn materialise_job(
    input: &mut CreateTrainingJobInput,
    algorithm_metrics: Option<Vec<MetricDefinition>>,
) {
    if let Some(specification) = input.algorithm_specification.as_mut() {
        // "To generate and save time-series metrics during training, set to true. The
        // default is false." (`AlgorithmSpecification$EnableSageMakerMetricsTimeSeries`.)
        specification.enable_sage_maker_metrics_time_series =
            Some(specification.enable_sage_maker_metrics_time_series.unwrap_or(false));
        // "A list of metric definition objects. Each object specifies the metric name and
        // regular expressions used to parse algorithm logs"
        // (`AlgorithmSpecification$MetricDefinitions`): the metric set belongs to the
        // ALGORITHM, which is why a registered one declares its own
        // (`TrainingSpecification$MetricDefinitions`, "used for parsing metrics generated by
        // the algorithm") and why an algorithm AWS itself publishes brings the set it already
        // emits ("time-series metrics aren't generated except in the following cases: You use
        // one of the SageMaker built-in algorithms",
        // `AlgorithmSpecification$EnableSageMakerMetricsTimeSeries`). So a job that states no
        // metrics of its own runs — and is reported — with the set of the algorithm it names:
        // the registered algorithm's when it names one, and otherwise the one the built-in
        // algorithm its image names publishes ([`crate::state::builtin_algorithm_metrics`]). A
        // job that states its own keeps exactly those, and one running a container of the
        // caller's own is reported with none.
        if specification.metric_definitions.is_none() {
            let published = crate::state::builtin_specification_metric_definitions(specification);
            specification.metric_definitions = algorithm_metrics.or(published);
        }
    }
    for channel in input.input_data_config.iter_mut().flatten() {
        // "If training data is compressed, the compression type. The default value is
        // None." (`Channel$CompressionType`.)
        if channel.compression_type.is_none() {
            channel.compression_type = Some(NOT_COMPRESSED.to_string());
        }
        // How the channel's objects are WRAPPED for the algorithm, which a channel that
        // states none is not wrapped for: "If the input data is already in RecordIO format,
        // you don't need to set this attribute" and "In File mode, leave this field unset or
        // set it to None" (`Channel$RecordWrapperType`) — the `RecordWrapper` enum's own
        // `None`, which is the value the channel then runs under.
        if channel.record_wrapper_type.is_none() {
            channel.record_wrapper_type = Some(NOT_WRAPPED.to_string());
        }
        // `InputMode` is deliberately NOT defaulted: it is the channel's OVERRIDE of the
        // specification's mode ("If you don't set a value for InputMode, SageMaker uses the
        // value set for TrainingInputMode" — `Channel$InputMode`), so a channel that
        // overrides nothing carries none and a reader resolves the mode it runs in from the
        // specification.
        if let Some(source) = channel.data_source.s3_data_source.as_mut() {
            if source.s3_data_distribution_type.is_none() {
                source.s3_data_distribution_type = Some(FULLY_REPLICATED.to_string());
            }
        }
    }
    if input.output_data_config.compression_type.is_none() {
        input.output_data_config.compression_type = Some(OUTPUT_COMPRESSED_WITH_GZIP.to_string());
    }
    // "If you don't provide a KMS key ID, SageMaker uses the default KMS key for Amazon S3
    // for your role's account" (`OutputDataConfig$KmsKeyId`): the job names no key of its
    // own, which the member reports as the empty id (`len 0..2048`).
    if input.output_data_config.kms_key_id.is_none() {
        input.output_data_config.kms_key_id = Some(String::new());
    }
    // The three switches a job that asked for none of them runs without: no network
    // isolation, no inter-container traffic encryption, no managed spot training.
    input.enable_network_isolation = Some(input.enable_network_isolation.unwrap_or(false));
    input.enable_inter_container_traffic_encryption =
        Some(input.enable_inter_container_traffic_encryption.unwrap_or(false));
    input.enable_managed_spot_training = Some(input.enable_managed_spot_training.unwrap_or(false));
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateTrainingJob<Eng>
{
    type Input = CreateTrainingJobInput;
    type Output = CreateTrainingJobOutput;
    type Error = CreateTrainingJobError;

    const NAME: &'static str = "CreateTrainingJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_TRAINING_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_TRAINING_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        mut input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| crate::scaffolded::Scaffolded::validation(crate::state::NO_STATE))?;

        let region = crate::state::request_region(ctx);
        // The ML compute the job would RUN on is settled first, before the store is
        // locked: a `ResourceConfig` that names no instance type with a count greater than
        // zero (and no heterogeneous cluster of groups that do) names no compute to launch
        // at all, so the create is refused for it and nothing is persisted. The quotas its
        // capacity draws on are weighed below, where the account's own utilisation is.
        if let Some(config) = input.resource_config.as_ref() {
            if let Some(refusal) = crate::state::resource_config_refusal(config) {
                return Err(refuse(refusal));
            }
        }
        // The job RUNS AS the role the request names, so the role is settled BEFORE
        // anything else about the request is judged and before the store is locked: a role
        // SageMaker cannot assume backs no training run at all, so the create is refused
        // for the role whatever else the request gets wrong, and nothing is persisted. Only
        // what the emulator's OWN IAM ANSWERED is refused (see `crate::state::admission`),
        // so an emulator with no IAM to ask refuses nothing.
        if crate::state::execution_role_is_unassumable(ctx, &region, &input.role_arn).await {
            return Err(refuse(crate::state::could_not_assume_role(&input.role_arn)));
        }
        // The job READS its channels as that role, so the input data sources are settled
        // SYNCHRONOUSLY too: a location that answers no object the role may list backs no
        // run of this job, so the create is refused for it and nothing is persisted. Awaited
        // with no store guard held, like the role itself, and — for the same reason — only
        // what was ANSWERED is refused.
        //
        // Weighed HERE, before the per-name and per-store rules below, for the same reason
        // the role is: both are facts about the identity and the data this job would run
        // with, settled before the store is locked at all. So a request that gets BOTH its
        // data source and its name wrong is refused for the data source — what AWS orders
        // these two refusals in is not recorded, and this keeps the whole admission in one
        // readable order rather than splitting it across the store guard.
        if let Some(refusal) = crate::state::absent_input_objects(ctx, &region, &input).await {
            return Err(refuse(refusal));
        }
        let arn = crate::state::resource_arn(&region, "training-job", &input.training_job_name);
        let now = ::std::time::SystemTime::now();

        // `AlgorithmSpecification` "identifies the training algorithm to use" (this
        // operation's own doc), so without it there is no job to run. The model leaves the
        // member optional, so this is the service's own rule. It is judged before the
        // store is locked: nothing about it needs state.
        let Some(specification) = input.algorithm_specification.as_ref() else {
            return Err(refuse(
                "AlgorithmSpecification is required to identify the training algorithm.",
            ));
        };
        // The algorithm is named EITHER by a registered `AlgorithmName` or by a
        // `TrainingImage`; the two are independent optional members the model cannot make
        // exclusive, so "exactly one" is this operation's own rule.
        //
        // The component the job's `ExperimentConfig` brings into being, set inside the
        // store block below and recorded once the guard is released.
        let tracked_component: Option<String>;
        // The container image the job RUNS, likewise set inside the store block and resolved
        // once the guard is released: the image the request names, and for a job naming a
        // registered algorithm the image that algorithm trains with.
        let job_image: Option<String>;
        // The identity this request speaks for, read from STS's and IAM's own records
        // BEFORE this service's store is reached: those are other services' stores on the
        // same engine, and no store guard of this service is held while one is asked.
        let created_by = crate::state::caller_user_context(&*self.engine, ctx);
        // The store guard lives in a BLOCK of its own: the job's run is awaited below, and
        // no state guard is ever held across an await.
        {
            let mut store = state.lock();

            // The `AlgorithmName` reference is RESOLVED FIRST, before the exclusion is
            // weighed: the service has to find the algorithm to know what the request asks
            // for, so a name this account holds no algorithm under is refused for THAT — a
            // request that also carries a `TrainingImage` included — rather than being
            // classified as a conflict between two members. `AlgorithmName` is modelled
            // `ArnOrName`, so either form resolves to the one stored record and the refusal
            // reports the ARN it resolved to.
            //
            // The resolved ARN is what the JOB records: a job keeps the identity of the
            // algorithm it ran, not the spelling the request happened to use, so the
            // reference survives the algorithm's deletion and a reader can still re-resolve
            // it. Applied below, once the borrow of the specification has ended.
            let mut resolved_algorithm_arn = None;
            // The metric set the algorithm the job names declares, resolved with the
            // reference itself so the stored job carries the metrics it RUNS with (see
            // [`materialise_job`]).
            let mut algorithm_metrics = None;
            // The image that algorithm trains with (`TrainingSpecification$TrainingImage`),
            // which is the image a job naming the algorithm rather than an image of its own
            // runs.
            let mut algorithm_image = None;
            if let Some(named) = specification.algorithm_name.as_deref() {
                let key = crate::state::name_from_arn_or_name(named);
                let Some(algorithm) = store.algorithms.get(key) else {
                    let algorithm_arn = crate::state::resource_arn(
                        &region,
                        crate::state::ALGORITHM_ARN_KIND,
                        key,
                    );
                    return Err(refuse(crate::state::entity_does_not_exist(
                        crate::state::ALGORITHM_ENTITY,
                        &algorithm_arn,
                    )));
                };
                resolved_algorithm_arn = Some(algorithm.arn.clone());
                algorithm_metrics = algorithm.training_specification.metric_definitions.clone();
                algorithm_image = Some(algorithm.training_specification.training_image.clone());
            }
            match (
                specification.algorithm_name.is_some(),
                specification.training_image.is_some(),
            ) {
                (true, true) => {
                    return Err(refuse(
                        "AlgorithmSpecification may name only one of AlgorithmName and \
                         TrainingImage.",
                    ));
                }
                (false, false) => {
                    return Err(refuse(
                        "AlgorithmSpecification must name either AlgorithmName or \
                         TrainingImage.",
                    ));
                }
                _ => {}
            }

            // `TrainingJobName`'s doc: "The name must be unique within an Amazon Web
            // Services Region in an Amazon Web Services account." A name already in use is
            // refused with the modelled `ResourceInUse`, never overwritten: otherwise the
            // caller reads a 200 for a job it did not create and loses the one it did.
            // Checked before the store's bound, so the per-name rule answers the same
            // whatever the store holds.
            if store.training_jobs.contains_key(&input.training_job_name) {
                return Err(CreateTrainingJobErrorModeled::ResourceInUse(ResourceInUse {
                    message: Some(format!(
                        "Training job names must be unique within an Amazon Web Services \
                         account and Region, and \"{}\" already exists.",
                        input.training_job_name
                    )),
                })
                .into());
            }
            // The store is bounded, so a caller varying `TrainingJobName` cannot grow it
            // without end. `ResourceLimitExceeded` is the limit failure this operation
            // models. The bound is the emulator's OWN (`crate::state::MODEL_QUOTA`) rather
            // than a published account quota, so the refusal states it in its own words
            // instead of pointing at Service Quotas — the form every other store bound in
            // this crate uses.
            if !store.training_jobs_below_quota() {
                return Err(CreateTrainingJobErrorModeled::ResourceLimitExceeded(
                    ResourceLimitExceeded {
                        message: Some(format!(
                            "The account-level service limit for training jobs is {}, with \
                             current utilization of {} and a request delta of 1.",
                            crate::state::TRAINING_JOB_QUOTA,
                            store.training_jobs.len()
                        )),
                    },
                )
                .into());
            }

            // The capacity the job asks for is weighed against the account's published
            // quotas — the ML storage volume one training job instance may be given, and
            // how many instances of that type the account may run for training jobs,
            // counted over the jobs it is already running — so a request past either is the
            // modelled `ResourceLimitExceeded`, the limit failure AWS answers a request for
            // capacity it will not provision with.
            if let Some(config) = input.resource_config.as_ref() {
                if let Some(refusal) = store.training_quota_refusal(config) {
                    return Err(CreateTrainingJobErrorModeled::ResourceLimitExceeded(
                        ResourceLimitExceeded {
                            message: Some(refusal),
                        },
                    )
                    .into());
                }
            }

            // An empty-valued hyperparameter is not kept: the stored map holds only the
            // entries a reader can tell from an absent key. See
            // `crate::state::retained_hyper_parameters`.
            input.hyper_parameters =
                crate::state::retained_hyper_parameters(input.hyper_parameters);
            // The algorithm reference as the service resolved it, so the stored job — and
            // every reader of it — identifies the algorithm by its ARN.
            if let (Some(algorithm_arn), Some(specification)) = (
                resolved_algorithm_arn,
                input.algorithm_specification.as_mut(),
            ) {
                specification.algorithm_name = Some(algorithm_arn);
            }
            // The service's own defaulting step, applied ONCE here so the record is the
            // materialised job rather than the bare request.
            materialise_job(&mut input, algorithm_metrics);
            // The image the job runs: its own when the request names one, and otherwise the
            // registered algorithm's.
            job_image = input
                .algorithm_specification
                .as_ref()
                .and_then(|specification| specification.training_image.clone())
                .or(algorithm_image);

            store.training_jobs.insert(
                input.training_job_name.clone(),
                // The ONE constructor of a stored training job, so a job admitted here and
                // one a hyperparameter tuning job launches are the same record. A caller's
                // own create has no parent tuning job.
                crate::state::TrainingJob::accepted(arn.clone(), &input, None, now),
            );

            // "Associates a SageMaker job as a trial component with an experiment and
            // trial" (`ExperimentConfig`'s doc): the configuration does not merely travel
            // with the job — the service creates the component for it, so a
            // source-keyed listing of the job's ARN reports one and the named trial holds
            // it. Recorded through the ONE writer of a job's component, so every
            // `ExperimentConfig`-carrying create records it alike.
            tracked_component = input.experiment_config.as_ref().and_then(|config| {
                store.record_job_trial_component(
                    &region,
                    crate::state::TrackedJob {
                        kind: crate::state::TRAINING_JOB_TRACKING,
                        name: &input.training_job_name,
                        arn: &arn,
                    },
                    config,
                    created_by,
                    now,
                )
            });
        }

        // The image detail every read of the job reports, resolved ONCE here: the digest
        // comes from Amazon ECR's own store on the same engine, so it is asked for with no
        // guard of this service's store held, and it is recorded onto the job rather than
        // re-resolved per read — an image pushed to the same tag afterwards does not change
        // what an already-accepted job reports.
        crate::state::record_training_job_image(
            &*self.engine,
            ctx,
            &state,
            &input.training_job_name,
            job_image.as_deref(),
            now,
        );

        // The component the configuration brought into being is a resource of its own, so
        // it leaves the same record the job does.
        if let Some(component_arn) = tracked_component {
            crate::state::record_state_change(Self::NAME, &component_arn, ctx);
        }
        // From here on the job exists, so whatever its run does — including this handler
        // never getting to report it — is recorded against the record by the recorder's
        // `Drop`. The outcome is RECORDED, not revealed: the terminal status is owed by the
        // clock, which `TrainingJob::settle_status` materialises once the run has reported
        // one and the span is over.
        let mut recorder = RunOutcomeRecorder {
            store: state.clone(),
            job_name: input.training_job_name.clone(),
            reported: false,
        };
        let outcome = crate::state::run_training_job(
            &*self.engine,
            ctx,
            &region,
            &input.training_job_name,
            &input,
            // The container runs with the credentials of the job's execution role, so an AWS
            // call the algorithm's own program makes is made AS that role (see
            // `crate::state::ContainerAwsAccess`).
            crate::state::container_aws_access(&*self.engine, ctx, &region, &input.role_arn),
        )
        .await;
        recorder.report(outcome);
        drop(recorder);

        // What the run produced appears in Amazon S3 at the stage of the work that uploads it
        // ("Uploading - Training is complete and the model artifacts are being uploaded to the
        // S3 location"), which the clock owes later than this call. A client that creates a
        // job and then watches the output prefix never calls this service again, so that
        // instant is armed here rather than left to a read that may never come; it claims the
        // same held archive a read claims, so whichever gets there first writes it once.
        let due_at = {
            let store = state.lock();
            let job = store.training_jobs.get(&input.training_job_name);
            [
                job.and_then(crate::state::TrainingJob::checkpoints_due_at),
                job.and_then(crate::state::TrainingJob::artifact_due_at),
            ]
        };
        for due_at in due_at.into_iter().flatten() {
            crate::state::publish_when_due(
                ::std::sync::Arc::clone(&self.engine),
                ctx.callback().clone(),
                region.clone(),
                input.training_job_name.clone(),
                due_at,
            );
        }

        crate::state::record_state_change(Self::NAME, &arn, ctx);
        Ok(CreateTrainingJobOutput {
            training_job_arn: arn,
        })
    }
}

/// Records what the job's RUN reported onto the stored job, whatever happens to the request
/// that started it.
///
/// The run belongs to the JOB, not to the create: once the record exists, the outcome has to
/// reach it even if this handler is dropped mid-run (a cancelled request, a panic), because a
/// job whose run never reported has no terminal status for the clock to materialise at all
/// (`crate::state::TrainingJob::settle_status`) and would stay `InProgress` for the life of
/// the process. A run nothing reported for is the server's own failure
/// ([`crate::state::INTERNAL_FAILURE_REASON`]), written through this guard's `Drop` rather
/// than by a line the unwinding path could skip.
struct RunOutcomeRecorder {
    store: ::engine::ServiceState<crate::state::ComAmazonawsSagemakerSagemakerState>,
    job_name: String,
    reported: bool,
}

impl RunOutcomeRecorder {
    /// Write `outcome` onto the stored job, and remember that it has been written.
    fn report(&mut self, outcome: crate::state::TrainingRunOutcome) {
        self.reported = true;
        self.record(outcome);
    }

    /// Write one outcome onto the stored job, at the instant it is reported, so the clock owes
    /// the job's terminal status no earlier than the end of its own work.
    fn record(&self, outcome: crate::state::TrainingRunOutcome) {
        if let Some(job) = self.store.lock().training_jobs.get_mut(&self.job_name) {
            job.record_run(outcome, ::std::time::SystemTime::now());
        }
    }
}

impl ::std::ops::Drop for RunOutcomeRecorder {
    fn drop(&mut self) {
        if self.reported {
            return;
        }
        self.record(crate::state::TrainingRunOutcome::failed(
            crate::state::INTERNAL_FAILURE_REASON,
        ));
    }
}
