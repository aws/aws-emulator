// SPDX-License-Identifier: CC0-1.0

//! The SERVER-SIDE RUN of an accepted training job: the training data it reads out of the
//! channels the request named, the model it fits on them, the model artifact it writes
//! under the job's output location, and the metrics it measured.
//!
//! # What a training job DOES
//!
//! "After training completes, SageMaker saves the resulting model artifacts to an Amazon S3
//! location that you specify" (`CreateTrainingJob`'s own doc), and `DescribeTrainingJob`
//! reports that object as `ModelArtifacts$S3ModelArtifacts` — "the path of the S3 object
//! that contains the model artifacts". So an accepted job's work is: read the channels,
//! produce a model, write it where the request asked, and report the object. Nothing in the
//! API says HOW the work is performed.
//!
//! This module performs it IN PROCESS, over the caller's own data:
//!
//! * every channel's `S3DataSource` is read out of this emulator's own Amazon S3 over the
//!   in-process callback, bounded by [`MAX_OBJECT_BYTES`] per object,
//!   [`MAX_CHANNEL_OBJECTS`] objects per channel and [`MAX_RUN_INPUT_BYTES`] over the run;
//! * the records are read as comma-separated values — the form a training channel carries
//!   for SageMaker's built-in algorithms, which document "the target variable in the first
//!   column and the input data in the remaining columns" and no header record — by the
//!   maintained CSV reader rather than by splitting text here;
//! * a least-squares LINEAR model is fitted on them by batch gradient descent, bounded by
//!   [`MAX_FIT_OPERATIONS`]: every number the run reports is measured from the caller's
//!   data;
//! * the model artifact is the gzip-compressed tar of what the job's CONTAINER left in
//!   `/opt/ml/model` ("a SageMaker container stores your trained model artifacts in the
//!   /opt/ml/model directory. After training has completed, by default, these artifacts are
//!   uploaded to your Amazon S3 bucket as compressed files" — `ModelArtifacts`): the files a
//!   declared program wrote there, and otherwise the fitted model under the file name the
//!   ALGORITHM's container writes its model as ([`super::training_algorithm`]). It is
//!   PUBLISHED under `<S3OutputPath>/<TrainingJobName>/output/` once the stage of the work
//!   that uploads it is over ([`super::training_artifact`]), so the object the describe names
//!   is an object Amazon S3 itself holds from then on;
//! * the checkpoints a job that asked for them gets are the files the container left in its
//!   checkpoint directory, written to `CheckpointConfig.S3Uri`, and otherwise the state the
//!   fit reached;
//! * the error of the fitted model is measured over each channel and reported as the run's
//!   final metrics, named per channel and per the metric the algorithm's learning task is
//!   evaluated by ([`super::TrainingAlgorithm`]).
//!
//! The metric a run reports is therefore the one THIS fit measured, named the way the
//! algorithm the job's `AlgorithmSpecification` identifies names it
//! ([`super::training_algorithm`]). Nothing here runs the algorithm IMAGE, so the
//! `MetricDefinitions` a custom image's log lines would be scraped for are not scraped, and
//! the `HyperParameters` the request carried steer only which metric the run is evaluated by
//! (and are recorded in the artifact) rather than the fit itself. A caller that reads
//! `FinalMetricDataList` finds the names its algorithm emits, carrying values that were
//! genuinely measured over the data it supplied.
//!
//! The run is the JOB's own work, so what it could not do is the job's modelled `Failed`
//! status with the `FailureReason` naming the condition that failed — never a refusal of the
//! `CreateTrainingJob` request that submitted it, and never a `Completed` status for a run
//! that wrote no artifact.
//!
//! What fails a run is a LEG of its work that failed: an object it could not download under
//! the execution role, an artifact it could not write, or the container PROGRAM the request
//! declared for the algorithm exiting non-zero — `AlgorithmSpecification$ContainerEntrypoint`
//! "takes precedence over the default train processing instructions", so when a request
//! declares one it is performed ([`crate::state::run_container_program`]) and the status it
//! exits with is the job's own outcome, reported in AWS's `AlgorithmError` form
//! ([`crate::state::algorithm_error`]). The SHAPE of the caller's data is not one of them —
//! nothing in the API models what a channel's bytes mean, `Channel$ContentType`
//! being the caller's own MIME type and the algorithm, not SageMaker, reading them — so data
//! that holds no record of the form above leaves a run that still fits, writes its artifact
//! and completes, with how many rows held no record reported in that artifact.
//!
//! The stages of the work are reported in the order the run got through them: the run records
//! the `SecondaryStatus` phases it reached ([`TrainingRunOutcome::phases`]) and the clock
//! reveals them from the instant the run reported ([`super::TrainingJob::record_run`],
//! [`super::TrainingJob::settle_status`]), so a caller polling the job sees the data come
//! down, the fit run and the artifact go up before the terminal status — and the artifact
//! appears in Amazon S3 once the stage that uploads it is over and never before
//! ([`super::training_artifact`]).
//!
//! # The job runs AS the execution role
//!
//! "RoleArn - The Amazon Resource Name (ARN) that SageMaker assumes to perform tasks on your
//! behalf during model training. You must grant this role the necessary permissions so that
//! SageMaker can successfully complete model training" (`CreateTrainingJob`), and
//! `S3DataSource$S3Uri`'s own doc: "the object that each S3Uri points to must be readable by
//! the IAM role that SageMaker uses to perform tasks on your behalf". So each object the run
//! reads and the artifact it writes is a call made UNDER that role: the decision is IAM's,
//! asked of IAM through the shared reading
//! ([`::service_common::iam_role::role_withholds_action`]), and an action IAM withholds from
//! the role is one the run does not perform — the job fails with the reason naming that
//! condition. Where IAM answers nothing (no IAM mounted, a role this emulator holds no
//! record of) nothing is withheld, for the reason that module gives.
//!
//! The create asks the same question of the INPUT DATA SOURCES before it accepts the job at
//! all ([`absent_input_objects`]): a location that answers no object the role may list backs
//! no run, so it is refused synchronously instead of settling a job that could only fail.

use super::*;

use crate::ops::types::{CreateTrainingJobInput, MetricData};

/// The most bytes the run reads out of ONE object.
///
/// An EMULATOR ceiling, not an AWS limit: the run holds what it reads in memory, so the read
/// is bounded in every direction the caller's data can grow.
pub const MAX_OBJECT_BYTES: usize = 1_048_576;

/// The most objects the run reads out of ONE channel's location.
pub const MAX_CHANNEL_OBJECTS: usize = 16;

/// The most bytes the run reads across every channel of one job.
pub const MAX_RUN_INPUT_BYTES: usize = 4_194_304;

/// The most records the fit is given, out of those the channels hold.
pub const MAX_RECORDS: usize = 5_000;

/// The most feature columns one record contributes; a record wider than this is fitted on
/// its first [`MAX_FEATURES`] features, which is what the artifact then reports.
pub const MAX_FEATURES: usize = 32;

/// The work one fit may do, counted in (record, feature) updates: it is what decides how
/// many epochs the descent runs ([`fit_epochs`]), so the fit is bounded whatever the data's
/// shape.
pub const MAX_FIT_OPERATIONS: usize = 2_000_000;

/// The most epochs the descent runs, however small the data is.
pub const MAX_EPOCHS: usize = 200;

/// The step the descent takes along the gradient of the mean squared error. Fixed, so one
/// dataset fits to one model.
const LEARNING_RATE: f64 = 0.5;

/// The channel SageMaker's built-in algorithms take their training data from ("specify the
/// training data in the `train` channel"). The run fits on this channel's records when the
/// job declares one.
pub const TRAIN_CHANNEL: &str = "train";

/// The entry the model archive holds: the fitted model, as this run serialized it.
pub const MODEL_ARCHIVE_ENTRY: &str = "model.json";

/// The object a finished run writes under `<S3OutputPath>/<TrainingJobName>/output/`
/// (`ModelArtifacts$S3ModelArtifacts`: "for example,
/// `s3://bucket-name/keynameprefix/model.tar.gz`").
pub const MODEL_ARCHIVE_OBJECT: &str = "model.tar.gz";

/// The OTHER object a training job's output location holds: the archive of everything the
/// job's own run wrote beside the model it fitted — what the SageMaker Developer Guide calls
/// the output data, written to the same `<S3OutputPath>/<TrainingJobName>/output/` prefix as
/// the model archive. It is the object the recorded service's `TrainingJobOutput` names.
pub const OUTPUT_ARCHIVE_OBJECT: &str = "output.tar.gz";

/// The `S3DataType` whose `S3Uri` "identifies an object that is a manifest file containing a
/// list of object keys that you want SageMaker to use for model training"
/// (`S3DataSource$S3DataType`). Every other value identifies a key name prefix, which is how
/// the run reads it.
pub const MANIFEST_FILE: &str = "ManifestFile";

// The `S3DataType` whose `S3Uri` identifies an augmented manifest — the object the records
// themselves live in — and the reading of those records live in [`super::augmented_manifest`],
// reached here through `use super::*`.

// The IAM actions an Amazon S3 read or write is authorized under, the partition an Amazon S3
// ARN carries and the location an `s3://` URI names are this service's ONE reading of an
// Amazon S3 location ([`super::s3_access`]), reached here through `use super::*`.

/// What a training job's run produced, recorded when the run reported and revealed by the
/// clock ([`super::TrainingJob::settle_status`]).
#[derive(Debug, Clone)]
pub enum TrainingRunOutcome {
    /// The run finished: the model artifact object it wrote, and the metrics it measured.
    Completed {
        /// The URI of the object Amazon S3 now holds, which is what
        /// `ModelArtifacts$S3ModelArtifacts` reports.
        model_artifacts: String,
        /// "The name, value, and date and time of a metric that was emitted"
        /// (`MetricData`), one per channel the run measured the fitted model over.
        final_metrics: Vec<MetricData>,
        /// The stages of its work the run GOT THROUGH, in the order it did (see
        /// [`TrainingRunOutcome::phases`]).
        phases: Vec<String>,
        /// The archive the run built, waiting for the stage of the work that PUBLISHES it
        /// (see [`super::training_artifact`]). Taken out of the outcome by the record that
        /// holds it ([`super::TrainingJob::record_run`]), so an outcome stored on a job
        /// carries none and nothing holds the bytes twice.
        artifact: Option<super::PendingArtifact>,
        /// The checkpoint objects the run produced for `CheckpointConfig.S3Uri`, waiting for
        /// the TRAINING stage that publishes them, and empty for a job that asked for none.
        /// Taken out of the outcome the same way the artifact is.
        checkpoints: Vec<super::PendingArtifact>,
        /// How long the job's CONTAINER work took, on the run's own clock: the time the
        /// declared container program stated its own work took
        /// ([`crate::state::ContainerOutcome::took`]), and zero for a run that performed no
        /// container program or one that asked for no time of its own. It is what the job's
        /// runtime is measured against ([`super::TrainingJob::record_run`]).
        took: Duration,
    },
    /// The run did not finish, and this is the condition that stopped it — the job's
    /// `FailureReason`.
    Failed {
        reason: String,
        /// The stages the run had got through when the condition stopped it, so the job's
        /// history says WHERE its work failed (see [`TrainingRunOutcome::phases`]).
        phases: Vec<String>,
        /// How long the container's work took before the condition stopped the run, on the
        /// same clock the completed variant's own member is read on.
        took: Duration,
    },
}

impl TrainingRunOutcome {
    /// A run stopped by `reason` before it got through any stage of its work.
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::failed_in(Vec::new(), reason)
    }

    /// A run stopped by `reason` once it had got through `phases`, bounded to the modelled
    /// `FailureReason` length (0..1024).
    #[must_use]
    pub fn failed_in(phases: Vec<String>, reason: impl Into<String>) -> Self {
        Self::failed_in_after(phases, reason, Duration::ZERO)
    }

    /// [`Self::failed_in`] for a run whose CONTAINER had already spent `took` of its own work
    /// when the condition stopped it, so the job's runtime still counts the work its container
    /// did ([`TrainingRunOutcome::Completed`]'s own member).
    #[must_use]
    pub fn failed_in_after(
        phases: Vec<String>,
        reason: impl Into<String>,
        took: Duration,
    ) -> Self {
        let reason: String = reason.into();
        let bounded = match reason.char_indices().nth(1024) {
            Some((at, _)) => reason[..at].to_string(),
            None => reason,
        };
        Self::Failed {
            reason: bounded,
            phases,
            took,
        }
    }

    /// The `SecondaryStatus` stages of its own work this run went through, after the
    /// `Starting` the job is accepted in: the data it downloaded to the ML storage volumes,
    /// the fit it performed and the artifact it uploaded, each named by the member
    /// `DescribeTrainingJob$SecondaryStatus`'s doc gives it.
    ///
    /// They are what the job ENTERED, so a run that stopped on one of its legs reports the
    /// stages before that leg and not the ones it never reached.
    #[must_use]
    pub fn phases(&self) -> &[String] {
        match self {
            Self::Completed { phases, .. } | Self::Failed { phases, .. } => phases,
        }
    }

    /// How long the job's CONTAINER work took, on the run's own clock (see the `took` member of
    /// either variant).
    #[must_use]
    pub fn took(&self) -> Duration {
        match self {
            Self::Completed { took, .. } | Self::Failed { took, .. } => *took,
        }
    }
}

/// How SageMaker words a training job that could not read a channel's data: the
/// `ClientError:` class a failure of the caller's own input carries, then the leg that
/// failed (the data download) and what failed on it.
#[must_use]
pub fn data_download_failed(channel: &str, uri: &str, detail: &str) -> String {
    format!("ClientError: Data download failed:{detail} for channel {channel} at {uri}")
}

/// What [`data_download_failed`] carries for a call the execution role is not allowed to
/// make: the Amazon S3 answer such a call gets, as the SDK inside the job's container reports
/// one — the MESSAGE Amazon S3 answered with FIRST, then the envelope naming the service, the
/// HTTP status and the error code.
///
/// The leading field is the error's own MESSAGE and not its code repeated: Amazon S3's list of
/// error responses pairs the code `AccessDenied` with the message "Access Denied", and that
/// message is what says WHY the call was refused — an operator reading a job's
/// `FailureReason` for an authorization failure is looking for exactly that sentence, which a
/// reason carrying the code twice never states.
#[must_use]
pub fn access_denied_detail() -> String {
    s3_refusal_detail(ACCESS_DENIED_MESSAGE)
}

/// The ENVELOPE a refused Amazon S3 call's answer arrives in, around the sentence Amazon S3
/// answered with: the service, the HTTP status and the error code, as the SDK inside a job's
/// container reports one.
///
/// Stated once because a job's work reads TWO sentences in this one envelope: Amazon S3's own
/// "Access Denied" for a call its own authorities refused ([`access_denied_detail`]), and the
/// sentence KMS worded for a read it could not open the object's data key for
/// ([`super::object_key_access`]) — a read Amazon S3 answers with the same code and status,
/// relaying what KMS said.
#[must_use]
pub fn s3_refusal_detail(message: &str) -> String {
    format!("{message} (Service: Amazon S3; Status Code: 403; Error Code: AccessDenied)")
}

/// The message Amazon S3 answers a refused call with, from its own list of error responses:
/// the code `AccessDenied` and the message "Access Denied", HTTP 403.
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// What [`data_download_failed`] carries for a location holding no object at all.
const NO_OBJECT_DETAIL: &str = "no object was found at the S3 location";

/// What [`data_download_failed`] carries for a read Amazon S3 refused for anything else: the
/// error code it answered with, and otherwise that nothing could be read — never a code
/// Amazon S3 did not answer.
#[must_use]
fn s3_answer_detail(code: Option<&str>, unanswered: &str) -> String {
    match code {
        Some(code) => format!("{code} (Service: Amazon S3)"),
        None => unanswered.to_string(),
    }
}

/// [`s3_answer_detail`]'s wording for a read Amazon S3 answered nothing readable to.
const UNREAD_OBJECT_DETAIL: &str = "the object could not be read from Amazon S3";

/// [`s3_answer_detail`]'s wording for a write Amazon S3 answered nothing readable to.
const UNWRITTEN_OBJECT_DETAIL: &str = "the object could not be written to Amazon S3";

/// How SageMaker words a run whose model artifacts could not be written: the upload leg that
/// failed and what failed on it.
#[must_use]
pub fn model_upload_failed(uri: &str, detail: &str) -> String {
    format!("ClientError: Model artifact upload failed:{detail} for {uri}")
}

/// One record of a training channel: the target variable the first column carries, and the
/// features the remaining columns carry.
#[derive(Debug, Clone)]
pub struct Record {
    pub target: f64,
    pub features: Vec<f64>,
}

/// What one channel contributed to the run.
#[derive(Debug, Clone)]
pub struct ChannelData {
    /// The channel's own name, which the metric it is measured over is named after.
    pub name: String,
    /// The location its records were read from, which a failure about it names.
    pub uri_read: String,
    /// How many objects of its location were read.
    pub objects: usize,
    /// The records read from them.
    pub records: Vec<Record>,
    /// The rows that held no record of the channel's form, which the artifact reports.
    pub skipped: usize,
    /// The objects THEMSELVES, as `(name under the channel's directory, contents)`: what the
    /// service places in the container before its entrypoint is invoked, under
    /// `/opt/ml/input/data/<ChannelName>/` ([`container_inputs`]).
    ///
    /// The records above are the same bytes READ as the form the built-in algorithms document;
    /// these are the bytes, which is what a container's own program opens.
    pub files: Vec<(String, String)>,
}

impl ChannelData {
    /// One channel's read, before anything has been read into it.
    pub fn new(name: &str, uri_read: String) -> Self {
        Self {
            name: name.to_string(),
            uri_read,
            objects: 0,
            records: Vec::new(),
            skipped: 0,
            files: Vec::new(),
        }
    }
}

/// The directory a training channel's data is placed in for the container: "SageMaker makes
/// the data available in the `/opt/ml/input/data/<channel_name>` directory" — the path a
/// container's own program opens its channel data at.
pub const INPUT_DATA_DIRECTORY: &str = "/opt/ml/input/data";

/// The data the service places in the container's storage before its entrypoint is invoked:
/// every object of every channel it downloaded, as `(absolute path, contents)` under that
/// channel's own directory below [`INPUT_DATA_DIRECTORY`].
///
/// This is what makes a declared container program — a shell program reading its channel, a
/// Python program counting its records — work over the job's OWN data rather than over an empty
/// container ([`crate::state::run_container_program`]).
///
/// Every path answered here is INSIDE that directory: the channel's own name is the one the
/// request named (`ChannelName` is `@pattern ^[A-Za-z0-9\.\-_]+$`, so it names one directory
/// below it) and the file's name is a relative name of ordinary segments
/// ([`object_name_under`]) — and the join is checked against the directory itself, so nothing a
/// request carries can place a file outside the container's own storage.
#[must_use]
pub fn container_inputs(channels: &[ChannelData]) -> Vec<(String, String)> {
    let root = format!("{INPUT_DATA_DIRECTORY}/");
    channels
        .iter()
        .flat_map(|channel| {
            channel.files.iter().map(|(name, contents)| {
                (
                    format!("{INPUT_DATA_DIRECTORY}/{}/{name}", channel.name),
                    contents.clone(),
                )
            })
        })
        .filter(|(path, _)| path.starts_with(&root) && !path.split('/').any(|part| part == ".."))
        .collect()
}

/// The directories the SERVICE creates in a training job's container before its entrypoint is
/// invoked, so each exists for the program whether or not any object was placed in it.
///
/// "SageMaker makes the data available in the `/opt/ml/input/data/<channel_name>` directory"
/// (so every channel the request declares has its own directory, and an empty channel has an
/// empty one), "a SageMaker container stores your trained model artifacts in the
/// /opt/ml/model directory" (so that directory is there for the container to write into), and
/// `CheckpointConfig$LocalPath` is "the local directory where checkpoints are written. The
/// default directory is /opt/ml/checkpoints/" — the same promise for a job that asked for
/// checkpoints. A program that lists its channel, or writes its model without creating the
/// directory first, therefore finds the directory SageMaker prepared for it.
#[must_use]
pub fn container_directories(request: &CreateTrainingJobInput) -> Vec<String> {
    let channels = request
        .input_data_config
        .iter()
        .flatten()
        .map(|channel| format!("{INPUT_DATA_DIRECTORY}/{}", channel.channel_name));
    let checkpoints = request.checkpoint_config.as_ref().map(|config| {
        config
            .local_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .unwrap_or(super::DEFAULT_CHECKPOINT_DIRECTORY)
            .to_string()
    });
    let mut prepared: Vec<String> = channels
        .chain([super::MODEL_DIRECTORY.to_string()])
        .chain(checkpoints)
        .map(|path| path.trim_end_matches('/').to_string())
        // A path that walks out of the directory it appears to be under is no directory of the
        // container's storage, the way a placed file's is not ([`container_inputs`]).
        .filter(|path| path.starts_with('/') && !path.split('/').any(|part| part == ".."))
        .collect();
    prepared.sort();
    prepared.dedup();
    prepared
}

/// A fitted linear model: `prediction = link(intercept + weights · features)`, in the
/// caller's own scale.
///
/// `link` is what the LEARNING TASK the job asked for answers through
/// ([`super::ObjectiveLink`]): a task whose output is a probability answers the logistic of
/// the linear score and a task whose output is a count a positive quantity, so the value this
/// model serves lies in the range its own task is defined on rather than being the unbounded
/// score. A job that asked for no such task answers the score itself
/// ([`super::ObjectiveLink::Identity`]).
#[derive(Debug, Clone)]
pub struct LinearModel {
    pub intercept: f64,
    pub weights: Vec<f64>,
    /// How many passes over the data the descent took, which the artifact reports.
    pub epochs: usize,
    /// The function the linear score is answered through, which the job's own learning task
    /// decides.
    pub link: ObjectiveLink,
}

impl LinearModel {
    /// What this model predicts for one record's features: its linear score through the
    /// learning task's own link.
    pub fn predict(&self, features: &[f64]) -> f64 {
        self.link.apply(self.margin(features))
    }

    /// The LINEAR SCORE this model gives one record's features, before the link: the space the
    /// fit's own descent works in.
    pub fn margin(&self, features: &[f64]) -> f64 {
        self.intercept
            + self
                .weights
                .iter()
                .zip(features)
                .map(|(weight, value)| weight * value)
                .sum::<f64>()
    }

    /// The root-mean-square error of this model over `records` — a MEASUREMENT over real
    /// predictions, which is what the run reports as its final metric.
    pub fn rmse(&self, records: &[Record]) -> f64 {
        if records.is_empty() {
            return 0.0;
        }
        let squared: f64 = records
            .iter()
            .map(|record| (self.predict(&record.features) - record.target).powi(2))
            .sum();
        (squared / records.len() as f64).sqrt()
    }
}

// The location an `s3://` URI names — `S3Location`, with its `uri`, `object_arn` and
// `bucket_arn` readings — lives in [`super::s3_access`], the one place this service reads an
// Amazon S3 location, and is reached here through `use super::*`.

/// How many epochs a dataset of `records` rows and `columns` features is fitted for: as many
/// as [`MAX_FIT_OPERATIONS`] admits, never more than [`MAX_EPOCHS`] and never none.
#[must_use]
pub fn fit_epochs(records: usize, columns: usize) -> usize {
    let per_epoch = records.saturating_mul(columns.max(1)).max(1);
    (MAX_FIT_OPERATIONS / per_epoch).clamp(1, MAX_EPOCHS)
}

/// Why SageMaker will not accept this training job: an input data source whose Amazon S3
/// location names no object the execution role can list — and `None` when every channel's
/// location answers at least one, which is also the answer wherever nothing was LEARNED.
///
/// The input data sources are settled SYNCHRONOUSLY, before the job is created: "the object
/// that each S3Uri points to must be readable by the IAM role that SageMaker uses to perform
/// tasks on your behalf" (`S3DataSource$S3Uri`), so a location that answers no object backs
/// no run of this job and the create is refused for it rather than accepted into a job that
/// could only fail. The listing is made with the ROLE's grant on the bucket decided by IAM
/// ([`::service_common::iam_role::role_withholds_action`]), which is the permission the
/// refusal names.
///
/// Only what was ANSWERED refuses: a listing that could not be made at all — no Amazon S3 to
/// ask — teaches nothing about the location, so it refuses nothing and the run reports what
/// it finds. A role nothing is known about is refused nothing, for the reason the shared
/// reading gives.
///
/// The RUN authorizes again, per OBJECT, for the objects only it can know: a prefix resolves
/// to concrete keys when it is read, which this admission cannot name in advance.
pub async fn absent_input_objects(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    request: &CreateTrainingJobInput,
) -> Option<String> {
    for channel in request.input_data_config.iter().flatten() {
        let Some(source) = channel.data_source.s3_data_source.as_ref() else {
            continue;
        };
        if let Some(refusal) =
            unlistable_input_location(ctx, region, &request.role_arn, &source.s3_uri).await
        {
            return Some(refusal);
        }
    }
    None
}

/// Why SageMaker will not accept a create whose resource reads the input data source at
/// `s3_uri` AS `role_arn`: that location names no object the role can list — and `None` when it
/// answers at least one, which is also the answer wherever nothing was LEARNED.
///
/// The ONE reading every create that resolves an input data source takes of ONE of its
/// locations, so no two of them refuse the same fact differently: a training job's channels
/// ([`absent_input_objects`]), and a processing job's SageMaker-managed `S3Input`s, each ask
/// this per location and refuse with the sentence it answers
/// ([`crate::state::no_s3_objects_found`]).
///
/// Two conditions produce it and the sentence states both, because the service itself cannot
/// tell them apart: the prefix holds nothing, or the role may not list the bucket. Which it was
/// is carried by the clause naming what Amazon S3 ANSWERED — the identity-based refusal for a
/// listing authorization withheld, and the sentence Amazon S3 wrote for a location whose bucket
/// or key it answered is not there ([`super::s3_access::NoObjectAnswer`]); a listing that
/// answered and merely held nothing refused nothing, so it carries no clause at all.
///
/// Only what was ANSWERED refuses: a listing that could not be made at all — no Amazon S3 to
/// ask — teaches nothing about the location, so it refuses nothing. A value that names no
/// Amazon S3 bucket names no location to list either, so it is left to the member's own
/// constraint rather than refused here.
pub async fn unlistable_input_location(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    s3_uri: &str,
) -> Option<String> {
    let location = S3Location::parse(s3_uri)?;
    let partition = arn_partition(role_arn);
    let refusal = |s3_error: Option<&str>| {
        crate::state::no_s3_objects_found(s3_uri, region, role_arn, &location.bucket, s3_error)
    };
    let bucket_arn = location.bucket_arn(&partition);
    if ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        role_arn,
        LIST_BUCKET_ACTION,
        &bucket_arn,
    )
    .await
    {
        // The listing was refused by AUTHORIZATION, so what Amazon S3 ANSWERED about it is
        // part of what the caller is told: the two conditions this one sentence covers are
        // told apart by that clause (see [`crate::state::no_s3_objects_found`]). WHICH of
        // IAM's rules withheld it is part of the answer too — a statement that denies the
        // listing is a different fact from no statement allowing it — so the narrower
        // reading is asked only once the wider one has answered, and the clause follows it.
        let reason = if ::service_common::iam_role::role_denies_action(
            ctx,
            region,
            role_arn,
            LIST_BUCKET_ACTION,
            &bucket_arn,
        )
        .await
        {
            ::service_common::iam_authorization::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
        } else {
            ::service_common::iam_authorization::no_identity_policy_allows(LIST_BUCKET_ACTION)
        };
        return Some(refusal(Some(&crate::state::s3_listing_refusal(
            role_arn,
            &bucket_arn,
            &partition,
            &reason,
        ))));
    }
    // A listing that ANSWERED, with no object under the prefix, is the refusal, and so is
    // an answer that NAMES the absence of the bucket or the key itself; one that could not
    // be made at all says nothing (see the doc above). The reading is the one this service
    // takes of every Amazon S3 location ([`super::location_no_object`]), and it carries
    // WHICH answer Amazon S3 gave: the clause reporting what Amazon S3 itself said carries
    // that sentence for an answer that named the absence, and nothing for a listing that
    // answered and merely held nothing, which Amazon S3 refused nothing about.
    let client = s3_client(ctx, region);
    super::s3_access::location_no_object(&client, &location)
        .await
        .map(|answer| refusal(answer.s3_error()))
}

/// Run one accepted training job: read its channels, fit a model on them, write the model
/// artifact under the output location the request named, and report what was produced.
///
/// Answers a [`TrainingRunOutcome`] rather than an error channel, for the reason the module
/// docs give: the run is the job's own work.
///
/// `container_access` is the seam an AWS SDK call the job's own CONTAINER PROGRAM makes travels
/// on, which is the session of the job's execution role
/// ([`crate::state::ContainerAwsAccess`]): a training container runs with that role's
/// credentials, so code inside it that reads or writes Amazon S3 itself makes that call as
/// that role.
pub async fn run_training_job<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    job_name: &str,
    request: &CreateTrainingJobInput,
    container_access: Option<crate::state::ContainerAwsAccess>,
) -> TrainingRunOutcome {
    let Some(output) = S3Location::parse(&request.output_data_config.s3_output_path) else {
        return TrainingRunOutcome::failed(model_upload_failed(
            &request.output_data_config.s3_output_path,
            "the output location names no Amazon S3 bucket",
        ));
    };
    let partition = arn_partition(&request.role_arn);
    let client = s3_client(ctx, region);

    // The stages of its own work this run gets through, recorded as it enters each one so a
    // run that stops on a leg reports the ones before it (see [`TrainingRunOutcome::phases`]).
    let mut phases: Vec<String> = Vec::new();
    // "Downloading - An optional stage for algorithms that support File training input mode.
    // It indicates that data is being downloaded to the ML storage volumes"
    // (`DescribeTrainingJob$SecondaryStatus`): the stage belongs to a run that HAS data to
    // bring down, so a job whose channels name no Amazon S3 location never enters it.
    if request
        .input_data_config
        .iter()
        .flatten()
        .any(|channel| channel.data_source.s3_data_source.is_some())
    {
        phases.push(TRAINING_SECONDARY_STATUS_DOWNLOADING.to_string());
    }

    let mut remaining_bytes = MAX_RUN_INPUT_BYTES;
    let mut channels: Vec<ChannelData> = Vec::new();
    for channel in request.input_data_config.iter().flatten() {
        let Some(source) = channel.data_source.s3_data_source.as_ref() else {
            // A channel whose data lives in a file system rather than in Amazon S3 takes no
            // part in the fit; a job whose channels hold no record at all fails below.
            continue;
        };
        match read_channel(
            engine,
            ctx,
            &client,
            region,
            &request.role_arn,
            &partition,
            channel.channel_name.as_str(),
            source.s3_data_type.as_str(),
            source.s3_uri.as_str(),
            // "A list of one or more attribute names to use that are found in a specified
            // augmented manifest file" (`S3DataSource$AttributeNames`): which attributes of
            // that manifest's JSON lines this channel's records are built from.
            source.attribute_names.as_deref().unwrap_or_default(),
            &mut remaining_bytes,
        )
        .await
        {
            Ok(read) => channels.push(read),
            Err(reason) => return TrainingRunOutcome::failed_in(phases, reason),
        }
    }

    // The records the model is FITTED on: the training channel's when the job declares one,
    // and otherwise every record the job's channels hold, narrowed to the ONE shape a model
    // can be fitted on — the feature count of the first record — so a channel of another
    // width takes no part rather than being read against the wrong columns.
    //
    // Data that holds no record of that form is not a failure of the JOB: nothing in the API
    // models what a channel's bytes mean — `Channel$ContentType` is the caller's own MIME
    // type and it is the algorithm, not SageMaker, that reads them — so a run whose download
    // and upload legs both succeeded completes, fitted on whatever records it could read and
    // reporting a metric only for the channels it measured one over. How many rows held no
    // record is reported in the artifact the run writes.
    let mut fitted_on: Vec<Record> = match channels.iter().find(|read| read.name == TRAIN_CHANNEL)
    {
        Some(train) => train.records.clone(),
        None => channels
            .iter()
            .flat_map(|read| read.records.iter().cloned())
            .collect(),
    };
    let columns = fitted_on.first().map_or(0, |record| record.features.len());
    fitted_on.retain(|record| record.features.len() == columns);

    // "Training - Training is in progress" (`DescribeTrainingJob$SecondaryStatus`): the fit
    // the run performs over the records it read.
    phases.push(TRAINING_SECONDARY_STATUS_TRAINING.to_string());

    // The job's ALGORITHM CONTAINER starts here, and SageMaker publishes a training job's
    // container output to CloudWatch Logs — so the group and the stream it writes to are
    // CloudWatch Logs' own resources from this instant on, and a client that lists, reads or
    // deletes the streams of a job that has trained finds them (see [`super::job_logs`]). A
    // run that stopped on an earlier leg never started a container, so it publishes none.
    //
    // The stream is opened with no event in it, for the reason that module states: the lines a
    // container printed are not a record this emulator holds, and none is invented for them.
    crate::state::publish_training_job_logs(ctx, region, job_name, SystemTime::now()).await;

    // The ALGORITHM the job names, which decides the file its container writes the model into
    // and the metric it emits per channel (see [`super::training_algorithm`]).
    let algorithm = super::TrainingAlgorithm::of(request);

    // The program the request DECLARED for the algorithm container, when it declared one:
    // `AlgorithmSpecification$ContainerEntrypoint` "takes precedence over the default train
    // processing instructions", so that argument vector is what the container runs and the
    // status it exits with is the job's own outcome — a non-zero status is a failure inside
    // the algorithm, which AWS reports as the `AlgorithmError` class carrying the container's
    // standard-error tail and that status ([`super::algorithm_error`]). A program that exits
    // zero leaves the default train instructions — performed below — to produce the artifact
    // `ModelArtifacts` promises, and a request declaring no program runs those instructions
    // alone.
    let mut container: Option<crate::state::ContainerOutcome> = None;
    if let Some(specification) = request.algorithm_specification.as_ref() {
        let program = crate::state::container_program(
            specification.container_entrypoint.as_deref(),
            specification.container_arguments.as_deref(),
        );
        // A job that declares NO program runs the default train instructions, which are
        // SageMaker's own and which this run performs below; only a DECLARED program takes
        // precedence over them.
        if !program.is_empty() {
            // Performed off the request worker, by the engine the program's own name chooses:
            // the bounded shell session, or the contained Python interpreter for a vector
            // naming the interpreter the SageMaker images provide (see
            // `crate::state::container_run`). Neither has a host process to start, a host
            // filesystem to open, a host environment to read or a socket. A program neither
            // engine performs is not performed at all, and no exit status is claimed for it;
            // a run this process could not carry out is the server's own failure.
            //
            // The container's storage holds the job's own data from its first line: every
            // object of every channel the run downloaded, under
            // `/opt/ml/input/data/<ChannelName>/` ([`container_inputs`]), which is where
            // SageMaker makes a channel's data available.
            let image = specification.training_image.clone().unwrap_or_default();
            let outcome = crate::state::run_container_program_off_worker(
                &program,
                &image,
                request.environment.as_ref(),
                &container_inputs(&channels),
                // The directories SageMaker prepares in the container before the entrypoint
                // is invoked: each channel's own, the model directory and the checkpoint one.
                &container_directories(request),
                container_access,
            )
            .await;
            match outcome {
                Some(outcome) => {
                    if let Some(reason) = outcome.failure_reason() {
                        return TrainingRunOutcome::failed_in_after(phases, reason, outcome.took());
                    }
                    container = Some(outcome);
                }
                None => {
                    return TrainingRunOutcome::failed_in(
                        phases,
                        crate::state::INTERNAL_FAILURE_REASON,
                    )
                }
            }
        }
    }

    // How long the container's own work took, which every outcome from here on reports: the
    // work was performed whatever the legs after it come to, and the job's runtime is measured
    // against it ([`super::TrainingJob::record_run`]).
    let took = container
        .as_ref()
        .map(crate::state::ContainerOutcome::took)
        .unwrap_or_default();

    // Fitting a model and measuring it are synchronous, CPU-bound work with no await of
    // their own, so they go to the blocking pool and the worker serving this request is
    // released while they run. The work itself is bounded ([`MAX_FIT_OPERATIONS`]), so the
    // pool is never occupied for long.
    let measured_at = timestamp(SystemTime::now());
    let measured: Vec<(String, Vec<Record>)> = channels
        .iter()
        .filter(|read| !read.records.is_empty())
        .map(|read| (read.name.clone(), read.records.clone()))
        .collect();
    let fit_and_measure = ::tokio::task::spawn_blocking(move || {
        let model = fit_under(
            &fitted_on,
            fit_epochs(fitted_on.len(), columns),
            algorithm.link,
        );
        let metrics: Vec<MetricData> = measured
            .iter()
            .filter_map(|(name, records)| {
                // The metric the ALGORITHM emits for this channel: the channel's own name,
                // then the metric its learning task is evaluated by, with the value MEASURED
                // over that channel's own records (see [`super::training_algorithm`]). A job
                // whose requested metric nothing here measures emits none, rather than a
                // number no measurement produced.
                let metric = algorithm.metric?;
                Some(MetricData {
                    metric_name: algorithm.channel_metric_name(name),
                    value: Some(metric.measure(&model, records) as f32),
                    timestamp: Some(measured_at),
                })
            })
            .collect();
        (model, metrics)
    })
    .await;
    let Ok((model, final_metrics)) = fit_and_measure else {
        return TrainingRunOutcome::failed_in_after(
            phases,
            crate::state::INTERNAL_FAILURE_REASON,
            took,
        );
    };

    // "CheckpointConfig - Contains information about the output location for managed spot
    // training checkpoint data": the checkpoints the container wrote to its checkpoint
    // directory go to that location, and they go there at the TRAINING stage rather than with
    // the model artifacts. A job that asked for none has none to write.
    let checkpoints = match checkpoint_objects(request, &model, &algorithm, container.as_ref()) {
        Ok(checkpoints) => checkpoints,
        Err(reason) => return TrainingRunOutcome::failed_in_after(phases, reason, took),
    };
    for checkpoint in &checkpoints {
        // The write is authorized the way every write a job's work makes is: the destination
        // bucket's own policy refuses what it EXPLICITLY DENIES, and otherwise the grant in
        // force on the execution role decides (see [`super::s3_access`]).
        if object_write_is_unauthorized(
            ctx,
            &client,
            region,
            &request.role_arn,
            &partition,
            &checkpoint.location,
        )
        .await
        {
            return TrainingRunOutcome::failed_in_after(
                phases,
                checkpoint_upload_failed(&checkpoint.location.uri(), &access_denied_detail()),
                took,
            );
        }
    }

    // "Uploading - Training is complete and the model artifacts are being uploaded to the S3
    // location" (`DescribeTrainingJob$SecondaryStatus`).
    phases.push(TRAINING_SECONDARY_STATUS_UPLOADING.to_string());
    let artifact = artifact_location(&output, job_name);
    let document = model_document(job_name, request, &model, &channels, &final_metrics);
    // What the archive holds is what the container left in `/opt/ml/model`: the files the
    // job's own program wrote when it wrote any, and otherwise the model the default train
    // instructions fitted, under the file name the algorithm's container writes it as (see
    // [`super::training_algorithm`]).
    let written = container
        .as_ref()
        .map(|outcome| outcome.files_under(super::MODEL_DIRECTORY))
        .unwrap_or_default();
    let entries: Vec<(String, Vec<u8>)> = if written.is_empty() {
        let Ok(body) = ::serde_json::to_vec_pretty(&document) else {
            return TrainingRunOutcome::failed_in_after(
                phases,
                model_upload_failed(&artifact.uri(), ARCHIVE_NOT_BUILT),
                took,
            );
        };
        vec![(algorithm.model_file().to_string(), body)]
    } else {
        written
            .into_iter()
            .map(|(name, contents)| (name, contents.into_bytes()))
            .collect()
    };
    let Ok(archive) = model_archive(&entries) else {
        return TrainingRunOutcome::failed_in_after(
            phases,
            model_upload_failed(&artifact.uri(), ARCHIVE_NOT_BUILT),
            took,
        );
    };
    if object_write_is_unauthorized(
        ctx,
        &client,
        region,
        &request.role_arn,
        &partition,
        &artifact,
    )
    .await
    {
        return TrainingRunOutcome::failed_in_after(
            phases,
            model_upload_failed(&artifact.uri(), &access_denied_detail()),
            took,
        );
    }
    // The archive is PRODUCED here and PUBLISHED at the stage that uploads it, which the
    // clock owes later than this call ([`super::training_artifact`]): the upload leg is a
    // leg of the run, and the object it leaves must not be readable before the job reports
    // that stage. A write Amazon S3 then refuses fails the job the same way a write refused
    // here would.
    TrainingRunOutcome::Completed {
        model_artifacts: artifact.uri(),
        final_metrics,
        phases,
        artifact: Some(super::PendingArtifact {
            location: artifact,
            archive,
        }),
        checkpoints,
        // The time the job's own container program stated its work took, which is the runtime
        // the job's `StoppingCondition` is measured against.
        took,
    }
}

/// What [`model_upload_failed`] carries for an archive this run could not build at all.
const ARCHIVE_NOT_BUILT: &str = " the model archive could not be built";

/// How SageMaker words a run whose checkpoints could not be written: the leg that failed and
/// what failed on it, in the `ClientError:` class a failure of the caller's own configuration
/// carries.
#[must_use]
pub fn checkpoint_upload_failed(uri: &str, detail: &str) -> String {
    format!("ClientError: Checkpoint upload failed:{detail} for {uri}")
}

/// The objects this job's checkpoints go to, and none at all for a job that asked for none.
///
/// "CheckpointConfig$S3Uri - Identifies the S3 path where you want SageMaker to store
/// checkpoints", written from `LocalPath` ("the local directory where checkpoints are
/// written. The default directory is /opt/ml/checkpoints/"). So the objects are the files the
/// container left in that directory; a job whose own program wrote none — the default train
/// instructions — checkpoints the model the fit reached, under the same file name the
/// algorithm's container writes its model as, because a checkpoint is the model's own state at
/// that point of the run.
///
/// `Err` is the job's `FailureReason`: a checkpoint location that names no bucket is a
/// configuration nothing can be written to.
fn checkpoint_objects(
    request: &CreateTrainingJobInput,
    model: &LinearModel,
    algorithm: &super::TrainingAlgorithm,
    container: Option<&crate::state::ContainerOutcome>,
) -> Result<Vec<super::PendingArtifact>, String> {
    let Some(config) = request.checkpoint_config.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(location) = S3Location::parse(&config.s3_uri) else {
        return Err(checkpoint_upload_failed(
            &config.s3_uri,
            " the checkpoint location names no Amazon S3 bucket",
        ));
    };
    let directory = config
        .local_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .unwrap_or(super::DEFAULT_CHECKPOINT_DIRECTORY);
    let mut written: Vec<(String, Vec<u8>)> = container
        .map(|outcome| outcome.files_under(directory))
        .unwrap_or_default()
        .into_iter()
        .map(|(name, contents)| (name, contents.into_bytes()))
        .collect();
    if written.is_empty() {
        let document = ::serde_json::json!({
            "model": {
                "type": "linear",
                "intercept": model.intercept,
                "weights": model.weights,
                "epochs": model.epochs,
            },
        });
        let body = ::serde_json::to_vec_pretty(&document).map_err(|_| {
            checkpoint_upload_failed(&location.uri(), " the checkpoint could not be built")
        })?;
        written.push((algorithm.model_file().to_string(), body));
    }
    let prefix = location.key.trim_end_matches('/');
    Ok(written
        .into_iter()
        .map(|(name, archive)| super::PendingArtifact {
            location: S3Location {
                bucket: location.bucket.clone(),
                key: if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                },
            },
            archive,
        })
        .collect())
}

/// Where a finished run writes the model artifact: under the output location the request
/// named, in the job's own folder — the same path [`super::TrainingJob::model_artifacts_path`]
/// reports while the job is still running.
fn artifact_location(output: &S3Location, job_name: &str) -> S3Location {
    let prefix = output.key.trim_end_matches('/');
    let key = if prefix.is_empty() {
        format!("{job_name}/output/{MODEL_ARCHIVE_OBJECT}")
    } else {
        format!("{prefix}/{job_name}/output/{MODEL_ARCHIVE_OBJECT}")
    };
    S3Location {
        bucket: output.bucket.clone(),
        key,
    }
}

/// Read one channel's records out of its Amazon S3 location.
///
/// The FORM the records are read in follows the channel's own `S3DataType`: an augmented
/// manifest carries the records inside the object itself, as the JSON lines the attributes
/// `attribute_names` names are taken out of ([`super::augmented_manifest`]), and every other
/// data type names objects of the comma-separated form the built-in algorithms document
/// ([`read_records`]).
///
/// `Err` is the job's `FailureReason`: the download leg failed, which is the job's own
/// failure rather than the request's.
///
/// `engine` is the shared store every service of this emulator keeps its own records in, which
/// the KMS half of each read's authorization is decided over ([`super::object_key_access`]).
#[allow(clippy::too_many_arguments)]
pub async fn read_channel<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    name: &str,
    s3_data_type: &str,
    s3_uri: &str,
    attribute_names: &[String],
    remaining_bytes: &mut usize,
) -> Result<ChannelData, String> {
    let Some(location) = S3Location::parse(s3_uri) else {
        return Err(data_download_failed(
            name,
            s3_uri,
            "the channel names no Amazon S3 bucket",
        ));
    };
    let mut read = ChannelData::new(name, location.uri());
    let objects = channel_objects(
        engine,
        ctx,
        client,
        region,
        role_arn,
        partition,
        name,
        s3_data_type,
        &location,
    )
    .await?;
    if objects.is_empty() {
        return Err(data_download_failed(name, &location.uri(), NO_OBJECT_DETAIL));
    }
    for object in objects {
        // The read is authorized against BOTH halves in force on it: the policy on the object's
        // bucket together with the grant in force on the execution role — a bucket policy that
        // explicitly denies the role is what stops the read whatever the role's own policies
        // allow (see [`super::s3_access`]) — and the KMS key the object is SEALED under, whose
        // `kms:Decrypt` the read performs as that same role ([`super::object_key_access`]). So a
        // job whose input its bucket fences the role off from, or whose data key that role may
        // not open, reads nothing and fails here rather than completing over data it never saw.
        if let Some(detail) =
            object_read_refusal(engine, ctx, client, region, role_arn, partition, &object).await
        {
            return Err(data_download_failed(name, &object.uri(), &detail));
        }
        let ceiling = MAX_OBJECT_BYTES.min(*remaining_bytes);
        if ceiling == 0 {
            break;
        }
        let bytes = read_object(client, &object, ceiling)
            .await
            .map_err(|detail| data_download_failed(name, &object.uri(), &detail))?;
        *remaining_bytes = remaining_bytes.saturating_sub(bytes.len());
        read.objects += 1;
        // The object itself, as the container's own storage holds it: the file the service
        // places under this channel's directory ([`container_inputs`]). Its name is the key
        // below the prefix that named it, of ordinary segments only, so a prefix holding a tree
        // is placed as that tree and a key that names no file of a tree places none.
        if let Some(file) = object_name_under(&location.key, &object.key) {
            read.files
                .push((file, String::from_utf8_lossy(&bytes).to_string()));
        }
        if s3_data_type == AUGMENTED_MANIFEST_FILE {
            read_augmented_records(&bytes, attribute_names, &mut read);
        } else {
            read_records(&bytes, &mut read);
        }
        if read.records.len() >= MAX_RECORDS {
            break;
        }
    }
    Ok(read)
}

/// The objects one channel's data lives in.
///
/// A location whose `S3DataType` is [`MANIFEST_FILE`] names a manifest, whose first element
/// is a prefix and whose remaining elements are suffixes of it (`S3DataSource$S3Uri`) — the
/// prefix element is an `s3://` URI, so the objects it names are objects of ITS bucket, which
/// is carried alongside each key rather than assumed to be the manifest's own. A location whose
/// data type is [`AUGMENTED_MANIFEST_FILE`] "identifies an object that is an augmented manifest
/// file in JSON lines format" holding "the data you want to use for model training"
/// (`S3DataSource$S3DataType`), so the one object the channel reads is that object itself.
/// Every other value names a key name prefix, so the objects are the ones Amazon S3 lists under
/// it — which is the object itself when the prefix names one.
#[allow(clippy::too_many_arguments)]
async fn channel_objects<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    channel: &str,
    s3_data_type: &str,
    location: &S3Location,
) -> Result<Vec<S3Location>, String> {
    if s3_data_type == AUGMENTED_MANIFEST_FILE {
        return Ok(vec![location.clone()]);
    }
    if s3_data_type == MANIFEST_FILE {
        if let Some(detail) =
            object_read_refusal(engine, ctx, client, region, role_arn, partition, location).await
        {
            return Err(data_download_failed(channel, &location.uri(), &detail));
        }
        let bytes = read_object(client, location, MAX_OBJECT_BYTES)
            .await
            .map_err(|detail| data_download_failed(channel, &location.uri(), &detail))?;
        return Ok(manifest_objects(&bytes, location));
    }
    let listed = client
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&location.key)
        .max_keys(i32::try_from(MAX_CHANNEL_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await;
    let Ok(listed) = listed else {
        // The listing is not a grant the run depends on: a location the role may read
        // object by object is read that way, so a listing that answered nothing leaves the
        // object the channel named as the one to read.
        return Ok(vec![location.clone()]);
    };
    let objects: Vec<S3Location> = listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        // A key ending in the delimiter is a folder marker rather than training data.
        .filter(|key| !key.ends_with('/'))
        .take(MAX_CHANNEL_OBJECTS)
        .map(|key| S3Location {
            bucket: location.bucket.clone(),
            key,
        })
        .collect();
    Ok(objects)
}

/// The objects a manifest names: "the first element is a prefix which is followed by one or
/// more suffixes… SageMaker appends the suffix elements to the prefix"
/// (`S3DataSource$S3Uri`).
///
/// The prefix element is itself an `s3://` URI, so each object answered here is an object of
/// THAT URI's bucket; a manifest that names no prefix leaves the objects in the manifest's own
/// bucket (`held_in`), which is where a suffix with nothing in front of it resolves.
pub fn manifest_objects(bytes: &[u8], held_in: &S3Location) -> Vec<S3Location> {
    let Ok(::serde_json::Value::Array(elements)) = ::serde_json::from_slice(bytes) else {
        return Vec::new();
    };
    let mut prefix = S3Location {
        bucket: held_in.bucket.clone(),
        key: String::new(),
    };
    let mut objects = Vec::new();
    for element in elements.iter().take(MAX_CHANNEL_OBJECTS + 1) {
        match element {
            ::serde_json::Value::Object(map) => {
                if let Some(named) = map
                    .get("prefix")
                    .and_then(::serde_json::Value::as_str)
                    .and_then(S3Location::parse)
                {
                    prefix = named;
                }
            }
            ::serde_json::Value::String(suffix) => objects.push(S3Location {
                bucket: prefix.bucket.clone(),
                key: format!("{}{suffix}", prefix.key),
            }),
            _ => {}
        }
    }
    objects
}

/// The bytes of one object, up to `ceiling`.
///
/// `Err` is what Amazon S3 answered, as a failure reason's detail names it.
pub async fn read_object(
    client: &::aws_sdk_s3::Client,
    object: &S3Location,
    ceiling: usize,
) -> Result<Vec<u8>, String> {
    let read = client
        .get_object()
        .bucket(&object.bucket)
        .key(&object.key)
        // The ceiling is enforced by the REQUEST as well as by the bytes that arrive, so an
        // answer declaring no length is bounded too. An HTTP byte range is INCLUSIVE, so the
        // last byte asked for is `ceiling - 1`: the range states exactly the bound the
        // constant documents.
        .range(format!("bytes=0-{}", ceiling.saturating_sub(1)))
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                bucket = ?object.bucket,
                key = ?object.key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a SageMaker training run could not read a training object"
            );
            s3_answer_detail(
                error
                    .as_service_error()
                    .and_then(::aws_sdk_s3::error::ProvideErrorMetadata::code),
                UNREAD_OBJECT_DETAIL,
            )
        })?;
    let body = read
        .body
        .collect()
        .await
        .map_err(|_| s3_answer_detail(None, UNREAD_OBJECT_DETAIL))?
        .into_bytes();
    Ok(body.into_iter().take(ceiling).collect())
}

/// Write one object of this service's own output, or answer what Amazon S3 answered.
///
/// `output_kms_key_id` is the `KmsKeyId` the output configuration this object belongs to
/// declared, which decides the server-side encryption the object is stored under
/// ([`super::with_output_encryption`]): a declared key makes it an SSE-KMS write under that
/// key, so a later `HeadObject` or `GetObject` reports `aws:kms` and that key, and a
/// configuration declaring none leaves the destination bucket's own default to apply.
pub async fn write_object(
    client: &::aws_sdk_s3::Client,
    object: &S3Location,
    document: Vec<u8>,
    output_kms_key_id: Option<&str>,
) -> Result<(), String> {
    super::with_output_encryption(
        client
            .put_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(document)),
        output_kms_key_id,
    )
    .send()
    .await
    .map_err(|error| {
        ::tracing::debug!(
            bucket = ?object.bucket,
            key = ?object.key,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "a SageMaker run could not write one of its output objects"
        );
        s3_answer_detail(
            error
                .as_service_error()
                .and_then(::aws_sdk_s3::error::ProvideErrorMetadata::code),
            UNWRITTEN_OBJECT_DETAIL,
        )
    })?;
    Ok(())
}

/// Read the records one object holds into `read`, as comma-separated values.
///
/// A row whose first field is not a number, that holds no feature at all, or whose feature
/// count differs from the first record's, is COUNTED as skipped rather than guessed at: the
/// channel's form is the one the built-in algorithms document, and a row that is not of that
/// form is not a record of it.
pub fn read_records(bytes: &[u8], read: &mut ChannelData) {
    let expected = read.records.first().map(|record| record.features.len());
    let mut reader = ::csv::ReaderBuilder::new()
        // "SageMaker requires that a CSV file doesn't have a header record"
        // (`Channel$ContentType`'s `text/csv` form), so every row is a record.
        .has_headers(false)
        // A ragged row is the caller's data rather than a parse failure: it is counted as
        // skipped below.
        .flexible(true)
        .from_reader(bytes);
    let mut columns = expected;
    for row in reader.records() {
        if read.records.len() >= MAX_RECORDS {
            return;
        }
        let Ok(row) = row else {
            read.skipped += 1;
            continue;
        };
        let values: Option<Vec<f64>> = row
            .iter()
            .take(MAX_FEATURES + 1)
            .map(|field| field.trim().parse::<f64>().ok())
            .collect();
        let Some((target, features)) = values
            .as_deref()
            .and_then(<[f64]>::split_first)
            .filter(|(_, features)| !features.is_empty())
        else {
            read.skipped += 1;
            continue;
        };
        if columns.is_some_and(|held| held != features.len()) {
            read.skipped += 1;
            continue;
        }
        columns = Some(features.len());
        read.records.push(Record {
            target: *target,
            features: features.to_vec(),
        });
    }
}

/// Fit a least-squares linear model on `records` by batch gradient descent over `epochs`
/// passes.
///
/// The fit of a learning task that answers the linear score itself; a task that answers
/// through a link of its own is fitted in that task's own space ([`fit_under`]).
pub fn fit(records: &[Record], epochs: usize) -> LinearModel {
    fit_under(records, epochs, ObjectiveLink::Identity)
}

/// Fit a linear model on `records` by batch gradient descent over `epochs` passes, answering
/// through `link`.
///
/// The features are standardised for the descent — each column by its own mean and standard
/// deviation, so one step is comparable across columns whatever units they carry — and the
/// fitted weights are returned in the CALLER's scale, so the model predicts from the data as
/// it was given. A column that does not vary has no scale to divide by and takes no part.
///
/// The descent is over the task the model ANSWERS, in the space where its step is bounded by
/// the caller's own data:
///
/// * a link whose output is BOUNDED — the logistic one, whose value lies in the unit interval
///   — is fitted by the canonical gradient of its own loss, the difference between the value
///   it answers for a record and that record's target, which can never exceed the width of
///   that interval. So a probability task is fitted as a probability task rather than fitted
///   as a line and squashed afterwards.
/// * every other link is fitted by least squares on the target taken BACK through the link
///   ([`super::ObjectiveLink::margin_of`]), which for the identity link is the target itself
///   and for a growing one is the target in the score's own space. The residual is then a
///   difference between numbers the caller's data states, so a link that grows with the score
///   cannot grow the step with it.
///
/// Either way the value served for a record is the link applied to the fitted score, so it
/// lies in the range the task's output is defined on.
pub fn fit_under(records: &[Record], epochs: usize, link: ObjectiveLink) -> LinearModel {
    let columns = records.first().map_or(0, |record| record.features.len());
    let count = records.len().max(1) as f64;
    // The column sums and the squared deviations are accumulated by WALKING each record, so
    // nothing indexes into a record and a record of another width contributes only the
    // columns it has.
    let mut sums = vec![0.0_f64; columns];
    for record in records {
        for (slot, value) in sums.iter_mut().zip(&record.features) {
            *slot += value;
        }
    }
    let means: Vec<f64> = sums.iter().map(|sum| sum / count).collect();
    let mut squares = vec![0.0_f64; columns];
    for record in records {
        for ((slot, value), mean) in squares.iter_mut().zip(&record.features).zip(&means) {
            *slot += (value - mean).powi(2);
        }
    }
    let deviations: Vec<f64> = squares
        .iter()
        .map(|square| {
            let deviation = (square / count).sqrt();
            if deviation > f64::EPSILON {
                deviation
            } else {
                1.0
            }
        })
        .collect();
    // The standardised matrix is built ONCE, so the descent's passes cost arithmetic only.
    let standardised: Vec<Vec<f64>> = records
        .iter()
        .map(|record| {
            record
                .features
                .iter()
                .zip(&means)
                .zip(&deviations)
                .map(|((value, mean), deviation)| (value - mean) / deviation)
                .collect()
        })
        .collect();

    let mut weights = vec![0.0_f64; columns];
    let mut intercept = 0.0_f64;
    for _ in 0..epochs {
        let mut gradient = vec![0.0_f64; columns];
        let mut bias_gradient = 0.0_f64;
        for (row, record) in standardised.iter().zip(records) {
            let score = intercept
                + weights
                    .iter()
                    .zip(row)
                    .map(|(weight, value)| weight * value)
                    .sum::<f64>();
            let error = match link {
                ObjectiveLink::Logistic => link.apply(score) - record.target,
                _ => score - link.margin_of(record.target),
            };
            bias_gradient += error;
            for (slot, value) in gradient.iter_mut().zip(row) {
                *slot += error * value;
            }
        }
        // A pass whose step leaves the numbers a model can carry is not taken: the descent
        // stops at the last pass that was finite, so the model served is one fitted on the
        // caller's data rather than one carrying values no prediction can be computed from.
        // Only a link that grows without bound can reach this at all.
        let stepped_intercept = intercept - LEARNING_RATE * bias_gradient / count;
        let stepped: Vec<f64> = weights
            .iter()
            .zip(&gradient)
            .map(|(weight, slot)| weight - LEARNING_RATE * slot / count)
            .collect();
        if !stepped_intercept.is_finite() || stepped.iter().any(|weight| !weight.is_finite()) {
            break;
        }
        intercept = stepped_intercept;
        weights = stepped;
    }

    // Back to the caller's own scale: `w·(x - m)/s + b` is `(w/s)·x + (b - Σ w·m/s)`.
    let mut offset = intercept;
    let scaled: Vec<f64> = weights
        .iter()
        .zip(&means)
        .zip(&deviations)
        .map(|((weight, mean), deviation)| {
            offset -= weight * mean / deviation;
            weight / deviation
        })
        .collect();
    LinearModel {
        intercept: offset,
        weights: scaled,
        epochs,
        link,
    }
}

/// The model the archive holds: the parameters the fit measured, how to compute an inference
/// from them, and what the run read to produce them.
fn model_document(
    job_name: &str,
    request: &CreateTrainingJobInput,
    model: &LinearModel,
    channels: &[ChannelData],
    metrics: &[MetricData],
) -> ::serde_json::Value {
    let specification = request.algorithm_specification.as_ref();
    ::serde_json::json!({
        "trainingJobName": job_name,
        "algorithm": {
            "trainingImage": specification.and_then(|spec| spec.training_image.clone()),
            "algorithmName": specification.and_then(|spec| spec.algorithm_name.clone()),
        },
        "hyperParameters": request.hyper_parameters.clone().unwrap_or_default(),
        "model": {
            "type": "linear",
            "inference": "prediction = link(intercept + sum(weights[i] * feature[i]))",
            "intercept": model.intercept,
            "weights": model.weights,
            "epochs": model.epochs,
            // The function the learning task answers its score through, so whatever serves
            // this artifact answers in the same space the fit measured in.
            "link": model.link.label(),
        },
        "channels": channels
            .iter()
            .map(|read| {
                ::serde_json::json!({
                    "name": read.name,
                    "s3Uri": read.uri_read,
                    "objects": read.objects,
                    "records": read.records.len(),
                    "skippedRows": read.skipped,
                })
            })
            .collect::<Vec<_>>(),
        "metrics": metrics
            .iter()
            .map(|metric| {
                ::serde_json::json!({
                    "name": metric.metric_name,
                    "value": metric.value,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// The `model.tar.gz` bytes for `entries`: a gzip-compressed tar holding one file per entry,
/// which is the form `ModelArtifacts` documents ("uploaded to your Amazon S3 bucket as
/// compressed files").
///
/// The entries are the files the container left in its model directory, so the archive holds
/// exactly what the job produced and nothing beside it.
pub fn model_archive(entries: &[(String, Vec<u8>)]) -> ::std::io::Result<Vec<u8>> {
    use ::std::io::Write;

    let encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    let mut archive = ::tar::Builder::new(encoder);
    for (name, body) in entries {
        let mut header = ::tar::Header::new_gnu();
        header.set_path(name)?;
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        // The archive is a function of what the run produced: nothing about the machine or the
        // moment it was built takes part, so one set of files serializes to one archive.
        header.set_mtime(0);
        header.set_cksum();
        archive.append(&header, body.as_slice())?;
    }
    let mut encoder = archive.into_inner()?;
    encoder.flush()?;
    encoder.finish()
}

// An Amazon S3 client over the in-process callback into this same emulator lives in
// `super::s3_access`, the one place this service builds one, and is reached here through
// `use super::*`.

/// The most bytes [`model_from_archive`] reads out of a model artifact AFTER decompression.
///
/// The ceilings on a read bound the COMPRESSED bytes only ([`MAX_OBJECT_BYTES`]), and a gzip
/// member expands by orders of magnitude, so the inflated side is bounded here too: the
/// decompressing stream and the entry read from it are both cut off at this many bytes, and an
/// archive that does not fit answers "no model" rather than growing the process. The budget is
/// generous against a model document this emulator writes — which is a few numbers per feature
/// column — and far below what the host would notice.
pub const MAX_INFLATED_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024;

/// The fitted model a `model.tar.gz` this emulator wrote holds, read back out of the archive.
///
/// The inverse of [`model_archive`]: the one reading of a model artifact, so whatever serves
/// an inference from a trained model reads the same parameters the fit measured instead of
/// deciding again how the archive is laid out. `None` for bytes that are not an archive this
/// emulator's own run produced — nothing is guessed for them.
///
/// Every entry is examined rather than one name assumed: the file an algorithm's container
/// writes its model as is the ALGORITHM's ([`super::training_algorithm`]), so the model is
/// found by what the entry CARRIES and not by what it is called. The walk is bounded by
/// [`super::MAX_ARTIFACT_ENTRIES`], the same ceiling every other reading of a caller's archive
/// takes.
///
/// The bytes are a caller's own object, so the DECOMPRESSED side is bounded as explicitly as
/// the download was: see [`MAX_INFLATED_ARCHIVE_BYTES`].
#[must_use]
pub fn model_from_archive(bytes: &[u8]) -> Option<LinearModel> {
    use ::std::io::Read;

    // The whole inflated stream is cut off at the budget, so no archive — however many
    // entries it declares and however large each says it is — can inflate past it.
    let decoder = ::flate2::read::GzDecoder::new(bytes).take(MAX_INFLATED_ARCHIVE_BYTES);
    let mut archive = ::tar::Archive::new(decoder);
    for entry in archive.entries().ok()?.take(super::MAX_ARTIFACT_ENTRIES) {
        let Ok(entry) = entry else { continue };
        // And the one entry the reading wants is cut off at the same budget, so the parse
        // that consumes it cannot be fed more than that either.
        let document: Option<::serde_json::Value> =
            ::serde_json::from_reader(entry.take(MAX_INFLATED_ARCHIVE_BYTES)).ok();
        if let Some(model) = document.and_then(fitted_model_in) {
            return Some(model);
        }
    }
    None
}

/// The fitted model the bytes of ONE file carry, and `None` for bytes that carry none.
///
/// The same reading [`model_from_archive`] applies to an archive's entries, over a file the
/// caller handed this service directly: a model directory's files are not always an archive's
/// entries — model data a container declares as an uncompressed object, or as the objects under a
/// key name prefix, is read file by file ([`super::model_from_entries`]) — so the reading of ONE
/// file lives here, beside the fit that wrote it.
#[must_use]
pub fn fitted_model_from_bytes(bytes: &[u8]) -> Option<LinearModel> {
    use ::std::io::Read;

    // Bounded like every other read of a caller's own bytes: the parse is fed at most the budget
    // the inflated side of an archive is bounded to.
    let document: ::serde_json::Value =
        ::serde_json::from_reader(bytes.take(MAX_INFLATED_ARCHIVE_BYTES)).ok()?;
    fitted_model_in(document)
}

/// The fitted model `document` carries, and `None` for a document that carries none.
fn fitted_model_in(document: ::serde_json::Value) -> Option<LinearModel> {
    let model = document.get("model")?;
    let weights: Vec<f64> = model
        .get("weights")?
        .as_array()?
        .iter()
        .map(::serde_json::Value::as_f64)
        .collect::<Option<Vec<f64>>>()?;
    Some(LinearModel {
        intercept: model.get("intercept")?.as_f64()?,
        weights,
        epochs: model
            .get("epochs")
            .and_then(::serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
        // A document that states no link states a model answering its own score, which is
        // what an artifact written before the link was recorded carries.
        link: model
            .get("link")
            .and_then(::serde_json::Value::as_str)
            .and_then(ObjectiveLink::named)
            .unwrap_or(ObjectiveLink::Identity),
    })
}

#[cfg(test)]
mod tests {
    use super::{fit, fit_epochs, read_records, ChannelData, MAX_FEATURES, MAX_RECORDS};

    /// One channel's read, as the rows below drive it.
    fn read_of(bytes: &str) -> ChannelData {
        let mut read = ChannelData::new("train", "s3://bucket/train/".to_string());
        read_records(bytes.as_bytes(), &mut read);
        read
    }

    /// RULE: a training channel's records are the comma-separated rows whose first column is
    /// the target and whose remaining columns are the features; a row that is not of that
    /// form is counted, never guessed at.
    ///
    /// The rows differ along the record count, the feature count, the number form, the
    /// raggedness of the data and whether the row holds numbers at all.
    #[test]
    fn a_channels_records_are_its_comma_separated_rows() {
        let rows: [(&str, &str, usize, usize); 6] = [
            ("one record of one feature", "1,2\n", 1, 0),
            ("three records of two features", "1,2,3\n4,5,6\n7,8,9\n", 3, 0),
            ("a decimal and a negative", "1.5,-2.25\n-3,4e2\n", 2, 0),
            ("a header record", "target,feature\n1,2\n", 1, 1),
            ("a ragged row", "1,2\n3,4,5\n6,7\n", 2, 1),
            ("a row with no feature", "1\n2,3\n", 1, 1),
        ];
        for (case, body, records, skipped) in rows {
            let read = read_of(body);
            assert_eq!(
                (read.records.len(), read.skipped),
                (records, skipped),
                "case {case:?}: expected {records} record(s) and {skipped} skipped, got \
                 {:?}",
                read
            );
        }
    }

    /// RULE: a record wider than [`MAX_FEATURES`] is fitted on its first [`MAX_FEATURES`]
    /// features, and no more than [`MAX_RECORDS`] records are read.
    #[test]
    fn the_read_is_bounded_in_width_and_in_length() {
        let wide: String = (0..MAX_FEATURES + 8)
            .map(|column| column.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let read = read_of(&format!("{wide}\n"));
        let features = read.records.first().map_or(0, |record| record.features.len());
        assert_eq!(
            features, MAX_FEATURES,
            "case a record wider than the ceiling: expected {MAX_FEATURES} features, got \
             {features}"
        );

        let long: String = (0..MAX_RECORDS + 16)
            .map(|row| format!("{row},1\n"))
            .collect();
        let read = read_of(&long);
        assert_eq!(
            read.records.len(),
            MAX_RECORDS,
            "case more rows than the ceiling: expected {MAX_RECORDS} records, got {}",
            read.records.len()
        );
    }

    /// RULE: the fit MEASURES the model from the data — a linear relation in the records is
    /// recovered, so the error it reports falls as the fit explains the data.
    ///
    /// The rows differ along the relation the data carries: one feature, two features, a
    /// constant target and a column that does not vary at all.
    #[test]
    fn the_fit_recovers_a_linear_relation_in_the_records() {
        // Each row: the case, and the records whose relation the fit must recover.
        let rows: [(&str, &str); 4] = [
            ("one feature, y = 2x + 1", "3,1\n5,2\n7,3\n9,4\n"),
            ("two features, y = x1 + x2", "2,1,1\n4,1,3\n6,2,4\n8,3,5\n"),
            ("a constant target", "5,1\n5,2\n5,3\n"),
            ("a column that does not vary", "1,7\n3,7\n5,7\n"),
        ];
        for (case, body) in rows {
            let read = read_of(body);
            let columns = read
                .records
                .first()
                .map_or(0, |record| record.features.len());
            let model = fit(&read.records, fit_epochs(read.records.len(), columns));
            let error = model.rmse(&read.records);
            let spread = {
                let mean = read.records.iter().map(|record| record.target).sum::<f64>()
                    / read.records.len() as f64;
                (read.records
                    .iter()
                    .map(|record| (record.target - mean).powi(2))
                    .sum::<f64>()
                    / read.records.len() as f64)
                    .sqrt()
            };
            assert!(
                error <= spread + 1e-9,
                "case {case:?}: the fitted model's error {error} must not exceed the data's \
                 own spread {spread}"
            );
            assert!(
                model.weights.len() == columns && model.intercept.is_finite(),
                "case {case:?}: expected {columns} finite weights, got {model:?}"
            );
        }
    }

    /// RULE: the fit is bounded — the epochs it runs are at least one pass and never cost
    /// more than the operation budget admits.
    #[test]
    fn the_epoch_count_is_bounded_by_the_operation_budget() {
        for (case, records, columns) in [
            ("a tiny dataset", 4_usize, 1_usize),
            ("a wide dataset", MAX_RECORDS, MAX_FEATURES),
            ("no record at all", 0, 0),
        ] {
            let epochs = fit_epochs(records, columns);
            assert!(
                (1..=super::MAX_EPOCHS).contains(&epochs),
                "case {case:?}: expected between 1 and {} epochs, got {epochs}",
                super::MAX_EPOCHS
            );
            let cost = epochs * records * columns;
            assert!(
                cost <= super::MAX_FIT_OPERATIONS,
                "case {case:?}: expected at most {} (record, feature) updates, got {cost}",
                super::MAX_FIT_OPERATIONS
            );
        }
    }
}
