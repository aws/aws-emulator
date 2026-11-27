// SPDX-License-Identifier: CC0-1.0

//! The SERVER-SIDE RUN of an accepted AutoML job V2: the dataset it analyses, the model
//! candidates it fits on it, the metrics it measures each candidate by, the candidate it
//! picks as best, and the notebooks and model artifacts it writes under the job's output
//! location.
//!
//! # What an AutoML job DOES
//!
//! "Creates an Autopilot job ... Find the best-performing model after you run an Autopilot
//! job by calling `DescribeAutoMLJobV2`" — an accepted job progresses on its own, with no
//! further call from the client, and the reader then reports `AutoMLJobStatus` `Completed`
//! with a `BestCandidate`, the `AutoMLJobArtifacts` notebooks and the
//! `AutoMLResolvedAttributes` the analysis resolved. Nothing in the API says HOW the work is
//! performed.
//!
//! This module performs it IN PROCESS, over the caller's own data, on the same terms the
//! training-job run does ([`super::training_run`], whose readings it reuses rather than
//! repeating):
//!
//! * every channel's `AutoMLS3DataSource` is read out of this emulator's own Amazon S3 over
//!   the in-process callback, under the job's execution role, bounded by the same per-object,
//!   per-channel and per-run ceilings;
//! * the records are read as comma-separated values, the default `ContentType` of a tabular
//!   AutoML channel (`text/csv;header=present`), with the target variable in the first
//!   column;
//! * the dataset is SPLIT into the records the candidates are fitted on and the records they
//!   are measured over — the `validation` channel when the job declares one, otherwise the
//!   fraction `DataSplitConfig$ValidationFraction` asks for, otherwise
//!   [`DEFAULT_VALIDATION_FRACTION`];
//! * the problem type is RESOLVED from the data when the request named none ("Autopilot
//!   infers the problem type"): a target taking two values is a binary classification, a
//!   small number of whole values a multiclass one, anything else a regression;
//! * [`CANDIDATE_EPOCH_BUDGETS`] candidates are fitted, each a least-squares linear model
//!   over a different budget of descent passes — a genuinely different fit, so the candidates
//!   genuinely differ — and each is MEASURED over the validation records by every metric the
//!   resolved problem type lists;
//! * the best candidate is the one whose objective metric is best, minimised or maximised as
//!   that metric's own direction says;
//! * each candidate's fitted model is written as the gzip-compressed tar `model.tar.gz` under
//!   the job's output location, and the data-exploration and candidate-definition notebooks
//!   are written beside them, so every location the describe reports is an object Amazon S3
//!   itself holds.
//!
//! Every number this run reports is therefore MEASURED over the caller's data. Nothing here
//! runs Autopilot's own preprocessing pipelines or its ensembling and hyperparameter-tuning
//! modes, so the candidates are not the ones AWS would generate and the `CandidateSteps` of
//! one are the empty list: the work was done by the job itself rather than by sub-jobs a
//! caller could describe, and naming a step ARN nothing answers would be a dangling
//! reference. What a candidate IS deployable as is reported in full — the
//! `InferenceContainers` definition naming the inference container of its problem type and
//! the `ModelDataUrl` of the model it fitted, which is a real object — because that pair is
//! the documented way a candidate is promoted to a model.
//!
//! The run is the JOB's own work, so what it could not do is the job's modelled `Failed`
//! status with the `FailureReason` naming the condition that failed, never a refusal of the
//! `CreateAutoMLJobV2` request that submitted it.

use super::*;

use crate::ops::types::{
    AutoMLJobChannel, AutoMLJobObjective, AutoMLProblemTypeConfig, CreateAutoMLJobV2Input,
    MetricDatum,
};

/// The `AutoMLChannelType` whose records the candidates are MEASURED over: "defines whether
/// the data are used for training or validation" (`AutoMLJobChannel$ChannelType`).
pub const VALIDATION_CHANNEL_TYPE: &str = "validation";

/// What fraction of a single-channel dataset is held back to measure the candidates over when
/// the request names no `DataSplitConfig$ValidationFraction` and declares no `validation`
/// channel.
///
/// `ValidationFraction`'s own doc gives the service's default: "the validation fraction (optional)
/// ... The default value is 0.2, and values must be greater than 0 and less than 1."
pub const DEFAULT_VALIDATION_FRACTION: f64 = 0.2;

/// The descent-pass budgets the run fits its candidates over, as a FRACTION of the budget
/// the dataset's own size admits ([`super::fit_epochs`]): a candidate's budget is that budget
/// scaled by its own weight over the largest weight here.
///
/// Three candidates, so the leaderboard the best is picked from holds more than one entry and
/// the pick is a real comparison; each weight is a genuinely different fit, so the candidates
/// differ in what they measured rather than in name only. Scaling DOWN from the largest weight
/// rather than up from the smallest is what keeps the whole run inside the single-fit ceiling
/// [`super::MAX_FIT_OPERATIONS`] states: the widest candidate costs one fit and the others
/// cost fractions of it.
pub const CANDIDATE_EPOCH_BUDGETS: [usize; 3] = [1, 2, 4];

/// The folder the run writes a job's own artifacts under, inside the job's output location —
/// the prefix Autopilot keeps a job's candidates and notebooks under.
const CANDIDATES_FOLDER: &str = "sagemaker-automl-candidates";

/// The object the data-exploration notebook is written as, which
/// `AutoMLJobArtifacts$DataExplorationNotebookLocation` reports.
const DATA_EXPLORATION_NOTEBOOK: &str = "SageMakerAutopilotDataExplorationNotebook.ipynb";

/// The object the candidate-definition notebook is written as, which
/// `AutoMLJobArtifacts$CandidateDefinitionNotebookLocation` reports.
const CANDIDATE_DEFINITION_NOTEBOOK: &str = "SageMakerAutopilotCandidateDefinitionNotebook.ipynb";

/// The Amazon ECR repository AWS publishes the inference container a TABULAR Autopilot
/// candidate is served by: Autopilot's tabular modes build their candidates on the AutoGluon
/// framework, whose inference container AWS publishes as `autogluon-inference` (the SageMaker
/// Developer Guide's registry-path tables list it among the Deep Learning Containers).
///
/// It is the repository a candidate's container definition names, and nothing is pulled from
/// it: the model behind the candidate is the artifact the run wrote, which this emulator
/// serves itself.
const CANDIDATE_INFERENCE_REPOSITORY: &str = "autogluon-inference";

/// The Amazon ECR registry account AWS publishes the Deep Learning Containers from — the
/// registry [`CANDIDATE_INFERENCE_REPOSITORY`] is published in, which the Deep Learning
/// Containers' registry table gives as one account across the Regions it publishes them in.
const DEEP_LEARNING_REGISTRY_ACCOUNT: &str = "763104351884";

/// The Amazon ECR path of the inference container a candidate of this run is served by, in the
/// Region the job ran in.
///
/// A container image is addressed `<account>.dkr.ecr.<region>.<domain>/<repository>`, and a
/// Region AWS publishes no such registry in is left to the only registry this emulator serves
/// there, which is this account's own ([`super::aws_publishes_from`] is the one reading of
/// which registry AWS publishes a SageMaker container from in a Region). No TAG is named: no
/// image was built or pulled for the candidate, so no version of one is claimed — what the
/// candidate is served from is its own `ModelDataUrl`.
#[must_use]
fn candidate_inference_image(region: &str) -> String {
    let registry = if aws_publishes_from(region, DEEP_LEARNING_REGISTRY_ACCOUNT) {
        DEEP_LEARNING_REGISTRY_ACCOUNT
    } else {
        ::emulator_server::EMULATOR_ACCOUNT_ID
    };
    format!("{registry}.dkr.ecr.{region}.amazonaws.com/{CANDIDATE_INFERENCE_REPOSITORY}")
}

/// The most distinct target values a dataset may take for the run to resolve its problem type
/// as a classification rather than a regression.
///
/// An EMULATOR reading of "Autopilot infers the problem type", bounded so a column of
/// continuous measurements is never mistaken for a label set.
const MAX_INFERRED_CLASSES: usize = 20;

/// `ProblemType`'s members (`smithy_query(query="shape",shape="ProblemType")`), which are what
/// `TabularJobConfig$ProblemType` carries and `TabularResolvedAttributes$ProblemType` reports.
pub const PROBLEM_TYPE_REGRESSION: &str = "Regression";
/// See [`PROBLEM_TYPE_REGRESSION`].
pub const PROBLEM_TYPE_BINARY_CLASSIFICATION: &str = "BinaryClassification";
/// See [`PROBLEM_TYPE_REGRESSION`].
pub const PROBLEM_TYPE_MULTICLASS_CLASSIFICATION: &str = "MulticlassClassification";

/// `AutoMLJobObjectiveType`'s members
/// (`smithy_query(query="shape",shape="AutoMLJobObjectiveType")`): which way a metric is better.
pub const OBJECTIVE_MINIMIZE: &str = "Minimize";
/// See [`OBJECTIVE_MINIMIZE`].
pub const OBJECTIVE_MAXIMIZE: &str = "Maximize";

/// `ObjectiveStatus`'s member a candidate whose objective metric was measured carries
/// (`smithy_query(query="shape",shape="ObjectiveStatus")`: `Succeeded` | `Pending` | `Failed`).
pub const OBJECTIVE_STATUS_SUCCEEDED: &str = "Succeeded";

/// `CandidateStatus`'s member a candidate whose fit finished carries
/// (`smithy_query(query="shape",shape="CandidateStatus")`).
pub const CANDIDATE_STATUS_COMPLETED: &str = "Completed";

/// `MetricDatum$Set`'s member the candidate metrics were measured over: the records held back
/// from the fit (`smithy_query(query="shape",shape="MetricSetSource")` answers `Train`,
/// `Validation` and `Test`).
pub const METRIC_SET_VALIDATION: &str = "Validation";

/// Why an AutoML run could not train a candidate: the dataset the channels named held no
/// record of the form a tabular AutoML channel carries, so there was nothing to fit.
///
/// The `ClientError:` class a failure of the caller's own input carries, the same class the
/// training run words its data failures with, then the condition that failed.
#[must_use]
pub fn no_records_to_fit(uri: &str) -> String {
    format!(
        "ClientError: Unable to parse the dataset at {uri}: no record of the tabular form \
         (a numeric target variable in the first column and at least one numeric feature \
         column) was read from it, so no candidate could be trained"
    )
}

/// One metric an AutoML candidate was measured by, and which way it is better.
#[derive(Debug, Clone)]
pub struct CandidateMetric {
    /// An `AutoMLMetricEnum` member.
    pub name: &'static str,
    /// An `AutoMLJobObjectiveType` member: whether a bigger or a smaller value is better.
    pub direction: &'static str,
    /// The value measured over the validation records.
    pub value: f64,
}

/// One candidate an AutoML run fitted, as `DescribeAutoMLJobV2` reports it.
#[derive(Debug, Clone)]
pub struct AutoMLCandidateRun {
    /// `AutoMLCandidate$CandidateName`.
    pub name: String,
    /// Every metric the resolved problem type lists, measured over the validation records.
    pub metrics: Vec<CandidateMetric>,
    /// The object this candidate's fitted model was written to.
    pub model_artifacts: String,
    /// The Amazon ECR path of the inference container the candidate is served by, resolved in
    /// the Region the job ran in ([`candidate_inference_image`]) — the `Image` of the
    /// container definition `AutoMLCandidate$InferenceContainers` reports beside the artifact.
    pub inference_image: String,
    /// How many descent passes this candidate's fit took — what makes it a different
    /// candidate from its siblings.
    pub epochs: usize,
}

impl AutoMLCandidateRun {
    /// This candidate's value for `metric`, when it was measured.
    #[must_use]
    pub fn metric(&self, metric: &str) -> Option<&CandidateMetric> {
        self.metrics.iter().find(|measured| measured.name == metric)
    }
}

/// What an AutoML job's run produced, recorded when the run reported and revealed by the
/// clock ([`super::AutoMLJobV2::settle_status`]).
#[derive(Debug, Clone)]
pub enum AutoMLRunOutcome {
    /// The run finished: the candidates it fitted, which of them is best, the notebooks it
    /// wrote and the attributes its analysis resolved.
    Completed(Box<AutoMLRunResult>),
    /// The run did not finish, and this is the condition that stopped it — the job's
    /// `FailureReason`.
    Failed {
        reason: String,
        /// The phases the run had got through when the condition stopped it.
        phases: Vec<String>,
    },
}

/// What a FINISHED AutoML run produced.
#[derive(Debug, Clone)]
pub struct AutoMLRunResult {
    /// Every candidate the run fitted, in the order it fitted them.
    pub candidates: Vec<AutoMLCandidateRun>,
    /// Which of [`Self::candidates`] the objective metric picked as best.
    pub best: usize,
    /// The `AutoMLMetricEnum` member the candidates were ranked by.
    pub objective_metric: &'static str,
    /// The `ProblemType` the analysis resolved, for a tabular job.
    pub resolved_problem_type: String,
    /// `AutoMLJobArtifacts$CandidateDefinitionNotebookLocation`.
    pub candidate_definition_notebook: String,
    /// `AutoMLJobArtifacts$DataExplorationNotebookLocation`.
    pub data_exploration_notebook: String,
    /// The `AutoMLJobSecondaryStatus` phases the run got through, in order.
    pub phases: Vec<String>,
}

impl AutoMLRunOutcome {
    /// A run stopped by `reason` once it had got through `phases`, bounded to the modelled
    /// `AutoMLFailureReason` length (0..1024).
    #[must_use]
    pub fn failed_in(phases: Vec<String>, reason: impl Into<String>) -> Self {
        let reason: String = reason.into();
        let bounded = match reason.char_indices().nth(1024) {
            Some((at, _)) => reason[..at].to_string(),
            None => reason,
        };
        Self::Failed {
            reason: bounded,
            phases,
        }
    }

    /// The `AutoMLJobSecondaryStatus` phases of its own work this run went through, after the
    /// `Starting` the job is accepted in.
    #[must_use]
    pub fn phases(&self) -> &[String] {
        match self {
            Self::Completed(result) => &result.phases,
            Self::Failed { phases, .. } => phases,
        }
    }
}

/// Run one accepted AutoML job V2: read its channels, resolve its problem type and objective,
/// fit and measure its candidates, write their artifacts and the job's notebooks, and report
/// what was produced.
///
/// Answers an [`AutoMLRunOutcome`] rather than an error channel, for the reason the module
/// docs give: the run is the job's own work.
///
/// `engine` is the shared store every service of this emulator keeps its own records in, which
/// the KMS half of each read's authorization is decided over ([`super::object_key_access`]).
pub async fn run_auto_ml_job_v2<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    job_name: &str,
    request: &CreateAutoMLJobV2Input,
) -> AutoMLRunOutcome {
    // "AnalyzingData - The data is being analyzed" (`AutoMLJobSecondaryStatus`): the stage the
    // run reads the dataset in.
    let mut phases = vec![AUTO_ML_SECONDARY_STATUS_ANALYZING_DATA.to_string()];
    let Some(output) = S3Location::parse(&request.output_data_config.s3_output_path) else {
        return AutoMLRunOutcome::failed_in(
            phases,
            model_upload_failed(
                &request.output_data_config.s3_output_path,
                "the output location names no Amazon S3 bucket",
            ),
        );
    };
    let partition = arn_partition(&request.role_arn);
    let client = s3_client(ctx, region);

    let mut remaining_bytes = MAX_RUN_INPUT_BYTES;
    let mut fitted_on: Vec<Record> = Vec::new();
    let mut measured_over: Vec<Record> = Vec::new();
    let mut reads: Vec<ChannelData> = Vec::new();
    for (index, channel) in request.auto_ml_job_input_data_config.iter().enumerate() {
        let Some(source) = channel.data_source.as_ref() else {
            continue;
        };
        let name = channel_name(channel, index);
        match read_channel(
            engine,
            ctx,
            &client,
            region,
            &request.role_arn,
            &partition,
            &name,
            source.s3_data_source.s3_data_type.as_str(),
            source.s3_data_source.s3_uri.as_str(),
            // `AutoMLS3DataSource` carries no `AttributeNames`, so a channel of this job that
            // names an augmented manifest takes every attribute its JSON lines hold
            // ([`super::augmented_manifest`]).
            &[],
            &mut remaining_bytes,
        )
        .await
        {
            Ok(read) => {
                if channel.channel_type.as_deref() == Some(VALIDATION_CHANNEL_TYPE) {
                    measured_over.extend(read.records.iter().cloned());
                } else {
                    fitted_on.extend(read.records.iter().cloned());
                }
                reads.push(read);
            }
            Err(reason) => return AutoMLRunOutcome::failed_in(phases, reason),
        }
    }

    // Only one shape of record can be fitted at once: the width of the first record read, so
    // a channel of another width takes no part rather than being read against the wrong
    // columns (the training run narrows its fit the same way).
    let columns = fitted_on.first().map_or(0, |record| record.features.len());
    fitted_on.retain(|record| record.features.len() == columns);
    measured_over.retain(|record| record.features.len() == columns);
    // A job that declared no `validation` channel is split here instead: the fraction the
    // request asked for, or the service's own default.
    if measured_over.is_empty() {
        let fraction = request
            .data_split_config
            .as_ref()
            .and_then(|config| config.validation_fraction)
            .map_or(DEFAULT_VALIDATION_FRACTION, f64::from)
            .clamp(0.0, 1.0);
        let held_back = ((fitted_on.len() as f64) * fraction).round() as usize;
        let keep = fitted_on.len().saturating_sub(held_back);
        measured_over = fitted_on.split_off(keep.max(1).min(fitted_on.len()));
    }
    if fitted_on.is_empty() {
        let named = reads
            .first()
            .map_or_else(|| output.uri(), |read| read.uri_read.clone());
        return AutoMLRunOutcome::failed_in(phases, no_records_to_fit(&named));
    }
    // A split that held nothing back leaves the fit's own records to measure over, which is
    // what the notebook records: a measurement over real predictions either way.
    if measured_over.is_empty() {
        measured_over = fitted_on.clone();
    }

    let resolved_problem_type = resolve_problem_type(&request.auto_ml_problem_type_config, &fitted_on);
    let objective_metric = resolve_objective_metric(
        request.auto_ml_job_objective.as_ref(),
        &resolved_problem_type,
    );

    // "FeatureEngineering - The data is being transformed" then "ModelTuning - The model is
    // being tuned" (`AutoMLJobSecondaryStatus`): the stages the candidates are fitted and
    // measured in.
    phases.push(AUTO_ML_SECONDARY_STATUS_FEATURE_ENGINEERING.to_string());
    phases.push(AUTO_ML_SECONDARY_STATUS_MODEL_TUNING.to_string());

    // Fitting the candidates and measuring them is synchronous, CPU-bound work with no await
    // of its own, so it goes to the blocking pool and the worker serving this request is
    // released while it runs. The work is bounded: the widest candidate's budget is the ONE
    // fit `MAX_FIT_OPERATIONS` admits for this dataset and every other candidate is a fraction
    // of it, so the whole run costs under twice a single fit however large the data is.
    let base_epochs = fit_epochs(fitted_on.len(), columns);
    let widest = CANDIDATE_EPOCH_BUDGETS.iter().copied().max().unwrap_or(1);
    let problem_type = resolved_problem_type.clone();
    // What a candidate ANSWERS follows the problem type the analysis resolved: a binary
    // classification is the probability of the positive class, so its candidates are fitted
    // and served through the logistic link ([`super::ObjectiveLink`]), while a regression and
    // a multiclass problem answer in the target's own space.
    let link = candidate_link(&resolved_problem_type);
    let fitting = ::tokio::task::spawn_blocking(move || {
        CANDIDATE_EPOCH_BUDGETS
            .iter()
            .map(|budget| {
                let epochs = (base_epochs.saturating_mul(*budget) / widest.max(1)).max(1);
                let model = fit_under(&fitted_on, epochs, link);
                let metrics = measure(&model, &measured_over, &problem_type);
                (model, epochs, metrics)
            })
            .collect::<Vec<_>>()
    })
    .await;
    let Ok(fits) = fitting else {
        return AutoMLRunOutcome::failed_in(phases, crate::state::INTERNAL_FAILURE_REASON);
    };

    let mut candidates: Vec<AutoMLCandidateRun> = Vec::new();
    for (index, (model, epochs, metrics)) in fits.into_iter().enumerate() {
        let name = candidate_name(job_name, index);
        let artifact = candidate_artifact(&output, job_name, &name);
        let document = candidate_document(job_name, &name, request, &model, &reads, &metrics);
        let Ok(body) = ::serde_json::to_vec_pretty(&document) else {
            return AutoMLRunOutcome::failed_in(
                phases,
                model_upload_failed(&artifact.uri(), "the model archive could not be built"),
            );
        };
        let Ok(archive) = model_archive(&[(MODEL_ARCHIVE_ENTRY.to_string(), body)]) else {
            return AutoMLRunOutcome::failed_in(
                phases,
                model_upload_failed(&artifact.uri(), "the model archive could not be built"),
            );
        };
        if let Err(reason) = put_artifact(
            ctx,
            &client,
            region,
            &request.role_arn,
            &partition,
            &artifact,
            archive,
            request.output_data_config.kms_key_id.as_deref(),
        )
        .await
        {
            return AutoMLRunOutcome::failed_in(phases, reason);
        }
        candidates.push(AutoMLCandidateRun {
            name,
            metrics,
            model_artifacts: artifact.uri(),
            inference_image: candidate_inference_image(region),
            epochs,
        });
    }
    let best = best_candidate(&candidates, objective_metric);

    // "CandidateDefinitionsGenerated" (`AutoMLJobSecondaryStatus`): the notebooks the job
    // reports as its own artifacts, written as real objects before the status claims them.
    let notebooks = [
        (
            DATA_EXPLORATION_NOTEBOOK,
            data_exploration_notebook(job_name, &reads, &resolved_problem_type, columns),
        ),
        (
            CANDIDATE_DEFINITION_NOTEBOOK,
            candidate_definition_notebook(job_name, &candidates, objective_metric, best),
        ),
    ];
    let mut written: Vec<String> = Vec::new();
    for (object, document) in notebooks {
        let location = notebook_location(&output, job_name, object);
        let Ok(body) = ::serde_json::to_vec_pretty(&document) else {
            return AutoMLRunOutcome::failed_in(
                phases,
                model_upload_failed(&location.uri(), "the notebook could not be built"),
            );
        };
        if let Err(reason) = put_artifact(
            ctx,
            &client,
            region,
            &request.role_arn,
            &partition,
            &location,
            body,
            request.output_data_config.kms_key_id.as_deref(),
        )
        .await
        {
            return AutoMLRunOutcome::failed_in(phases, reason);
        }
        written.push(location.uri());
    }
    phases.push(AUTO_ML_SECONDARY_STATUS_CANDIDATE_DEFINITIONS_GENERATED.to_string());

    let mut written = written.into_iter();
    let data_exploration_notebook = written.next().unwrap_or_default();
    let candidate_definition_notebook = written.next().unwrap_or_default();
    AutoMLRunOutcome::Completed(Box::new(AutoMLRunResult {
        candidates,
        best,
        objective_metric,
        resolved_problem_type,
        candidate_definition_notebook,
        data_exploration_notebook,
        phases,
    }))
}

/// Write one object of the run's own output under the execution role's grant, answering the
/// job's `FailureReason` for a write the role may not make or Amazon S3 would not take.
///
/// `output_kms_key_id` is the key the job's output configuration declared
/// (`AutoMLOutputDataConfig$KmsKeyId`), which every object of that output is encrypted with.
#[allow(clippy::too_many_arguments)]
async fn put_artifact(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    object: &S3Location,
    body: Vec<u8>,
    output_kms_key_id: Option<&str>,
) -> Result<(), String> {
    if ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        role_arn,
        PUT_OBJECT_ACTION,
        &object.object_arn(partition),
    )
    .await
    {
        return Err(model_upload_failed(&object.uri(), &access_denied_detail()));
    }
    write_object(client, object, body, output_kms_key_id)
        .await
        .map_err(|detail| model_upload_failed(&object.uri(), &detail))
}

/// The name a channel's read is recorded under: the `AutoMLChannelType` it declares, which is
/// what the channel's role in the job is, and otherwise its position in the list — an AutoML
/// channel carries no name of its own.
fn channel_name(channel: &AutoMLJobChannel, index: usize) -> String {
    match channel.channel_type.as_deref() {
        Some(kind) => kind.to_string(),
        None => format!("channel-{index}"),
    }
}

/// One candidate's name: the job's own name and the candidate's position, so the name is
/// stable for one job and unique inside it, inside `CandidateName`'s modelled length (1..64).
fn candidate_name(job_name: &str, index: usize) -> String {
    format!("{job_name}-candidate-{}", index + 1)
}

/// Where a candidate's fitted model is written, under the job's output location.
fn candidate_artifact(output: &S3Location, job_name: &str, candidate: &str) -> S3Location {
    job_object(
        output,
        job_name,
        &format!("{candidate}/output/{MODEL_ARCHIVE_OBJECT}"),
    )
}

/// Where one of the job's notebooks is written, under the job's output location.
fn notebook_location(output: &S3Location, job_name: &str, object: &str) -> S3Location {
    job_object(output, job_name, &format!("notebooks/{object}"))
}

/// One object of a job's own output: `<S3OutputPath>/<AutoMLJobName>/<folder>/<suffix>`.
fn job_object(output: &S3Location, job_name: &str, suffix: &str) -> S3Location {
    let prefix = output.key.trim_end_matches('/');
    let tail = format!("{job_name}/{CANDIDATES_FOLDER}/{suffix}");
    S3Location {
        bucket: output.bucket.clone(),
        key: if prefix.is_empty() {
            tail
        } else {
            format!("{prefix}/{tail}")
        },
    }
}

/// The function the candidates of `problem_type` answer their score through: a BINARY
/// classification's candidate answers the probability of the positive class, which is what the
/// logistic link gives, and a regression or a multiclass problem answers in the target's own
/// space.
#[must_use]
fn candidate_link(problem_type: &str) -> ObjectiveLink {
    if problem_type == PROBLEM_TYPE_BINARY_CLASSIFICATION {
        ObjectiveLink::Logistic
    } else {
        ObjectiveLink::Identity
    }
}

/// The `ProblemType` the run's candidates solve: the one the request named, and otherwise the
/// one the TARGET column's own values resolve to — "Autopilot infers the problem type".
///
/// A target taking two distinct values is a binary classification; a small number of whole
/// values is a multiclass one ([`MAX_INFERRED_CLASSES`]); anything else is a regression. A
/// problem type config that is not tabular carries no `ProblemType` member at all, so its
/// candidates are resolved from the data the same way.
fn resolve_problem_type(config: &AutoMLProblemTypeConfig, records: &[Record]) -> String {
    if let AutoMLProblemTypeConfig::TabularJobConfig(tabular) = config {
        if let Some(named) = tabular.problem_type.as_deref() {
            return named.to_string();
        }
    }
    let labels = class_labels(records);
    match labels.len() {
        2 => PROBLEM_TYPE_BINARY_CLASSIFICATION.to_string(),
        3..=MAX_INFERRED_CLASSES => PROBLEM_TYPE_MULTICLASS_CLASSIFICATION.to_string(),
        _ => PROBLEM_TYPE_REGRESSION.to_string(),
    }
}

/// The distinct WHOLE target values `records` take, in order, and the empty set when any
/// target is not a whole number or there are more of them than a label set can hold.
fn class_labels(records: &[Record]) -> Vec<i64> {
    let mut labels: Vec<i64> = Vec::new();
    for record in records {
        if record.target.fract().abs() > f64::EPSILON {
            return Vec::new();
        }
        let label = record.target as i64;
        if !labels.contains(&label) {
            if labels.len() == MAX_INFERRED_CLASSES {
                return Vec::new();
            }
            labels.push(label);
        }
    }
    labels.sort_unstable();
    labels
}

/// The `AutoMLMetricEnum` member the candidates are ranked by: the one the request's
/// `AutoMLJobObjective` named when this run measures it, and otherwise the DEFAULT objective
/// metric the problem type documents.
///
/// `AutoMLJobObjective$MetricName`'s own doc states both lists: for tabular problem types the
/// available metrics are `MAE`, `MSE`, `R2`, `RMSE` for regression, `Accuracy`, `AUC`,
/// `BalancedAccuracy`, `F1`, `Precision`, `Recall` for binary classification and `Accuracy`,
/// `BalancedAccuracy`, `F1macro`, `PrecisionMacro`, `RecallMacro` for multiclass; the default
/// objective metrics are `MSE` for regression, `F1` for binary classification and `Accuracy`
/// for multiclass classification.
fn resolve_objective_metric(
    objective: Option<&AutoMLJobObjective>,
    problem_type: &str,
) -> &'static str {
    let measured = metric_names(problem_type);
    let named = objective
        .map(|objective| objective.metric_name.as_str())
        .and_then(|name| measured.iter().copied().find(|metric| *metric == name));
    named.unwrap_or(match problem_type {
        PROBLEM_TYPE_BINARY_CLASSIFICATION => METRIC_F1,
        PROBLEM_TYPE_MULTICLASS_CLASSIFICATION => METRIC_ACCURACY,
        _ => METRIC_MSE,
    })
}

/// `AutoMLMetricEnum` members this run measures (`smithy_query(query="shape",
/// shape="AutoMLMetricEnum")` lists every member the enum carries).
pub const METRIC_MAE: &str = "MAE";
/// See [`METRIC_MAE`].
pub const METRIC_MSE: &str = "MSE";
/// See [`METRIC_MAE`].
pub const METRIC_RMSE: &str = "RMSE";
/// See [`METRIC_MAE`].
pub const METRIC_R2: &str = "R2";
/// See [`METRIC_MAE`].
pub const METRIC_ACCURACY: &str = "Accuracy";
/// See [`METRIC_MAE`].
pub const METRIC_BALANCED_ACCURACY: &str = "BalancedAccuracy";
/// See [`METRIC_MAE`].
pub const METRIC_F1: &str = "F1";
/// See [`METRIC_MAE`].
pub const METRIC_PRECISION: &str = "Precision";
/// See [`METRIC_MAE`].
pub const METRIC_RECALL: &str = "Recall";
/// See [`METRIC_MAE`].
pub const METRIC_F1_MACRO: &str = "F1macro";
/// See [`METRIC_MAE`].
pub const METRIC_PRECISION_MACRO: &str = "PrecisionMacro";
/// See [`METRIC_MAE`].
pub const METRIC_RECALL_MACRO: &str = "RecallMacro";

/// The metrics a problem type's candidates are measured by, as
/// `AutoMLJobObjective$MetricName`'s own doc lists them for that type — minus the ones this
/// run does not measure (`AUC` needs a scored probability, which a least-squares fit does not
/// produce).
fn metric_names(problem_type: &str) -> &'static [&'static str] {
    match problem_type {
        PROBLEM_TYPE_BINARY_CLASSIFICATION => &[
            METRIC_ACCURACY,
            METRIC_BALANCED_ACCURACY,
            METRIC_F1,
            METRIC_PRECISION,
            METRIC_RECALL,
        ],
        PROBLEM_TYPE_MULTICLASS_CLASSIFICATION => &[
            METRIC_ACCURACY,
            METRIC_BALANCED_ACCURACY,
            METRIC_F1_MACRO,
            METRIC_PRECISION_MACRO,
            METRIC_RECALL_MACRO,
        ],
        _ => &[METRIC_MAE, METRIC_MSE, METRIC_RMSE, METRIC_R2],
    }
}

/// Which way an `AutoMLMetricEnum` member is better: an error is minimised, a score is
/// maximised.
#[must_use]
pub fn metric_direction(metric: &str) -> &'static str {
    match metric {
        METRIC_MAE | METRIC_MSE | METRIC_RMSE => OBJECTIVE_MINIMIZE,
        _ => OBJECTIVE_MAXIMIZE,
    }
}

/// Measure `model` over `records` by every metric `problem_type` lists.
///
/// Every value is computed from real predictions the model made over the held-back records.
/// A metric this run cannot measure over them is not reported at all rather than reported as a
/// number nothing measured: the classification metrics need a LABEL SET, and a target column
/// holding more distinct values than [`MAX_INFERRED_CLASSES`] is no label set, so a job that
/// named a classification problem type over such a column reports only what was measurable.
fn measure(model: &LinearModel, records: &[Record], problem_type: &str) -> Vec<CandidateMetric> {
    let predictions: Vec<f64> = records
        .iter()
        .map(|record| model.predict(&record.features))
        .collect();
    let targets: Vec<f64> = records.iter().map(|record| record.target).collect();
    // The per-class counts every classification metric is read off, built ONCE for the
    // candidate rather than once per metric.
    let counted = confusion(&predictions, &targets);
    metric_names(problem_type)
        .iter()
        .filter_map(|metric| {
            Some(CandidateMetric {
                name: metric,
                direction: metric_direction(metric),
                value: metric_value(metric, &predictions, &targets, counted.as_ref())?,
            })
        })
        .collect()
}

/// One metric's value over the predictions and the targets they were made for, and `None`
/// where this run could not measure it.
fn metric_value(
    metric: &str,
    predictions: &[f64],
    targets: &[f64],
    counted: Option<&Confusion>,
) -> Option<f64> {
    let count = predictions.len().max(1) as f64;
    Some(match metric {
        METRIC_MAE => {
            predictions
                .iter()
                .zip(targets)
                .map(|(predicted, target)| (predicted - target).abs())
                .sum::<f64>()
                / count
        }
        METRIC_MSE => mean_squared_error(predictions, targets),
        METRIC_RMSE => mean_squared_error(predictions, targets).sqrt(),
        METRIC_R2 => r_squared(predictions, targets),
        _ => classification_metric(metric, counted?)?,
    })
}

/// The mean of the squared differences between the predictions and their targets.
fn mean_squared_error(predictions: &[f64], targets: &[f64]) -> f64 {
    let count = predictions.len().max(1) as f64;
    predictions
        .iter()
        .zip(targets)
        .map(|(predicted, target)| (predicted - target).powi(2))
        .sum::<f64>()
        / count
}

/// The coefficient of determination: how much of the targets' own variance the predictions
/// account for. Targets that do not vary leave nothing to account for, which is `0`.
fn r_squared(predictions: &[f64], targets: &[f64]) -> f64 {
    let count = targets.len().max(1) as f64;
    let mean = targets.iter().sum::<f64>() / count;
    let total: f64 = targets.iter().map(|target| (target - mean).powi(2)).sum();
    if total <= f64::EPSILON {
        return 0.0;
    }
    let residual: f64 = predictions
        .iter()
        .zip(targets)
        .map(|(predicted, target)| (predicted - target).powi(2))
        .sum();
    1.0 - residual / total
}

/// The per-class counts every classification metric of one candidate is read off.
///
/// Built ONCE per candidate ([`confusion`]) and then read in time proportional to the number
/// of CLASSES, which is bounded: no metric walks the records again.
#[derive(Debug, Clone)]
struct Confusion {
    /// One entry per class the targets take, in ascending label order.
    classes: Vec<ClassCounts>,
    /// How many records were assigned the class they really carry.
    correct: usize,
    /// How many records were measured.
    records: usize,
}

/// What one class contributed to a [`Confusion`].
#[derive(Debug, Clone, Default)]
struct ClassCounts {
    /// How many records were assigned to this class.
    assigned: usize,
    /// How many records really carry it.
    actual: usize,
    /// How many records both carry it and were assigned to it.
    hit: usize,
}

/// The per-class counts of one candidate's predictions, and `None` when the targets are no
/// LABEL SET at all: a column taking more than [`MAX_INFERRED_CLASSES`] distinct values is a
/// continuous measurement rather than a set of classes, so no classification metric is
/// measured over it — and the bound is what keeps this reading linear in the record count
/// however many distinct values the caller's data holds.
fn confusion(predictions: &[f64], targets: &[f64]) -> Option<Confusion> {
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for target in targets {
        seen.insert(target.round() as i64);
        if seen.len() > MAX_INFERRED_CLASSES {
            return None;
        }
    }
    let labels: Vec<i64> = seen.into_iter().collect();
    if labels.is_empty() {
        return None;
    }
    let mut classes = vec![ClassCounts::default(); labels.len()];
    let mut correct = 0;
    for (predicted, target) in predictions.iter().zip(targets) {
        let actual = target.round() as i64;
        let assigned = nearest_label(*predicted, &labels);
        // The label list is bounded and ascending, so both lookups are a search over at most
        // `MAX_INFERRED_CLASSES` entries.
        let Ok(actual_at) = labels.binary_search(&actual) else {
            continue;
        };
        let Ok(assigned_at) = labels.binary_search(&assigned) else {
            continue;
        };
        if let Some(counts) = classes.get_mut(actual_at) {
            counts.actual += 1;
        }
        if let Some(counts) = classes.get_mut(assigned_at) {
            counts.assigned += 1;
        }
        if actual_at == assigned_at {
            correct += 1;
            if let Some(counts) = classes.get_mut(actual_at) {
                counts.hit += 1;
            }
        }
    }
    Some(Confusion {
        classes,
        correct,
        records: targets.len(),
    })
}

impl Confusion {
    /// How many of the records assigned to the class at `index` really carry it.
    fn precision(&self, index: usize) -> f64 {
        self.classes.get(index).map_or(0.0, |counts| {
            if counts.assigned == 0 {
                0.0
            } else {
                counts.hit as f64 / counts.assigned as f64
            }
        })
    }

    /// How many of the records that really carry the class at `index` were assigned to it.
    fn recall(&self, index: usize) -> f64 {
        self.classes.get(index).map_or(0.0, |counts| {
            if counts.actual == 0 {
                0.0
            } else {
                counts.hit as f64 / counts.actual as f64
            }
        })
    }

    /// The mean of `of` over every class, which is what a macro-averaged metric reports.
    fn mean_over_classes(&self, of: impl Fn(usize) -> f64) -> f64 {
        (0..self.classes.len()).map(of).sum::<f64>() / self.classes.len().max(1) as f64
    }

    /// The POSITIVE class of a binary problem: the greatest label, which is the convention
    /// `Precision`, `Recall` and `F1` are measured under. The classes are in ascending label
    /// order, so that is the last of them.
    fn positive(&self) -> usize {
        self.classes.len().saturating_sub(1)
    }
}

/// One classification metric, read off the per-class counts.
///
/// `Precision`, `Recall` and `F1` are the binary metrics, measured over the greatest label as
/// the positive class; the `*Macro` ones average the per-class values over every class, which
/// is what "macro" names. `None` for a metric this reading does not measure.
fn classification_metric(metric: &str, counted: &Confusion) -> Option<f64> {
    Some(match metric {
        METRIC_ACCURACY => counted.correct as f64 / counted.records.max(1) as f64,
        METRIC_BALANCED_ACCURACY | METRIC_RECALL_MACRO => {
            counted.mean_over_classes(|class| counted.recall(class))
        }
        METRIC_PRECISION_MACRO => counted.mean_over_classes(|class| counted.precision(class)),
        METRIC_F1_MACRO => counted.mean_over_classes(|class| {
            harmonic_mean(counted.precision(class), counted.recall(class))
        }),
        METRIC_PRECISION => counted.precision(counted.positive()),
        METRIC_RECALL => counted.recall(counted.positive()),
        METRIC_F1 => {
            let positive = counted.positive();
            harmonic_mean(counted.precision(positive), counted.recall(positive))
        }
        _ => return None,
    })
}

/// The class a prediction is assigned to: the label it is nearest to. The label list is
/// bounded by [`MAX_INFERRED_CLASSES`], so this is a bounded walk per prediction.
fn nearest_label(predicted: f64, labels: &[i64]) -> i64 {
    labels
        .iter()
        .copied()
        .min_by(|left, right| {
            (predicted - *left as f64)
                .abs()
                .total_cmp(&(predicted - *right as f64).abs())
        })
        .unwrap_or_default()
}

/// The harmonic mean of a precision and a recall, which is what an F1 score is.
fn harmonic_mean(precision: f64, recall: f64) -> f64 {
    if precision + recall <= f64::EPSILON {
        return 0.0;
    }
    2.0 * precision * recall / (precision + recall)
}

/// Which candidate the objective metric picks: the best value of it, minimised or maximised as
/// the metric's own direction says. A tie keeps the first candidate, so the pick is stable.
fn best_candidate(candidates: &[AutoMLCandidateRun], objective: &str) -> usize {
    let minimise = metric_direction(objective) == OBJECTIVE_MINIMIZE;
    candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            candidate.metric(objective).map(|metric| (index, metric.value))
        })
        .reduce(|best, next| {
            let better = if minimise {
                next.1 < best.1
            } else {
                next.1 > best.1
            };
            if better {
                next
            } else {
                best
            }
        })
        .map_or(0, |(index, _)| index)
}

/// The model one candidate's archive holds: the parameters its fit measured, how to compute an
/// inference from them, and what the run read and measured to produce them.
///
/// The same layout the training run writes ([`super::training_run`]), so one reading of a
/// model artifact serves both.
fn candidate_document(
    job_name: &str,
    candidate: &str,
    request: &CreateAutoMLJobV2Input,
    model: &LinearModel,
    reads: &[ChannelData],
    metrics: &[CandidateMetric],
) -> ::serde_json::Value {
    ::serde_json::json!({
        "autoMLJobName": job_name,
        "candidateName": candidate,
        "problemTypeConfigName": auto_ml_problem_type_config_name(&request.auto_ml_problem_type_config),
        "model": {
            "type": "linear",
            "inference": "prediction = link(intercept + sum(weights[i] * feature[i]))",
            "intercept": model.intercept,
            "weights": model.weights,
            "epochs": model.epochs,
            "link": model.link.label(),
        },
        "channels": channel_summaries(reads),
        "metrics": metrics
            .iter()
            .map(|metric| {
                ::serde_json::json!({
                    "name": metric.name,
                    "value": metric.value,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// What each channel contributed to the run, as the artifacts and notebooks report it.
fn channel_summaries(reads: &[ChannelData]) -> Vec<::serde_json::Value> {
    reads
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
        .collect()
}

/// The data-exploration notebook the run writes: what it read, and what the analysis resolved
/// from it.
fn data_exploration_notebook(
    job_name: &str,
    reads: &[ChannelData],
    problem_type: &str,
    columns: usize,
) -> ::serde_json::Value {
    let read_rows: usize = reads.iter().map(|read| read.records.len()).sum();
    notebook(vec![
        format!("# Data exploration for AutoML job {job_name}"),
        format!(
            "The dataset holds {read_rows} record(s) of {columns} feature column(s), read \
             from {} channel(s).",
            reads.len()
        ),
        format!("The problem type resolved from the target column is {problem_type}."),
        format!(
            "Channels: {}",
            ::serde_json::Value::Array(channel_summaries(reads))
        ),
    ])
}

/// The candidate-definition notebook the run writes: every candidate it fitted, how it
/// differs from its siblings, what it measured and which one was picked.
fn candidate_definition_notebook(
    job_name: &str,
    candidates: &[AutoMLCandidateRun],
    objective: &str,
    best: usize,
) -> ::serde_json::Value {
    let mut cells = vec![
        format!("# Candidate definitions for AutoML job {job_name}"),
        format!(
            "{} candidate(s) were fitted and ranked by {objective} ({}).",
            candidates.len(),
            metric_direction(objective)
        ),
    ];
    for (index, candidate) in candidates.iter().enumerate() {
        cells.push(format!(
            "{}{}: a linear model fitted over {} descent pass(es); artifact {}; measured {}",
            candidate.name,
            if index == best { " (best)" } else { "" },
            candidate.epochs,
            candidate.model_artifacts,
            candidate
                .metrics
                .iter()
                .map(|metric| format!("{}={}", metric.name, metric.value))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    notebook(cells)
}

/// A notebook document holding one markdown cell per line, in the `nbformat` 4 layout a
/// Jupyter notebook object carries, so the object the describe names opens as a notebook.
fn notebook(cells: Vec<String>) -> ::serde_json::Value {
    ::serde_json::json!({
        "cells": cells
            .into_iter()
            .map(|source| {
                ::serde_json::json!({
                    "cell_type": "markdown",
                    "metadata": {},
                    "source": [source],
                })
            })
            .collect::<Vec<_>>(),
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5,
    })
}

/// The `MetricDatum` list `CandidateProperties$CandidateMetrics` reports for one candidate:
/// every metric the run measured over the records it held back.
#[must_use]
pub fn candidate_metric_data(candidate: &AutoMLCandidateRun) -> Vec<MetricDatum> {
    candidate
        .metrics
        .iter()
        .map(|metric| MetricDatum {
            metric_name: Some(metric.name.to_string()),
            standard_metric_name: Some(metric.name.to_string()),
            value: Some(metric.value as f32),
            set: Some(METRIC_SET_VALIDATION.to_string()),
        })
        .collect()
}

/// `DescribeAutoMLJobV2$BestCandidate` for a job whose run finished: the candidate the
/// objective metric picked, with the metric it was picked on and every metric it was measured
/// by.
///
/// `InferenceContainers` carries the ONE container definition the candidate is deployable as:
/// the `Image` of the inference container that serves this problem type's candidates and the
/// `ModelDataUrl` of the model this candidate's own fit wrote under the job's output location,
/// which is the pair `CreateModel` takes — so the documented Autopilot deploy path (describe
/// the job, read the best candidate's container, register it as a model) needs nothing
/// re-derived. The artifact it names is an object Amazon S3 really holds: the run writes it
/// before the job's status claims the candidate.
///
/// `CandidateSteps` is the EMPTY list: the candidate's work was performed by the job's own run
/// rather than by sub-jobs a caller could describe, so naming a step ARN nothing answers would
/// be a dangling reference.
#[must_use]
pub fn best_candidate_report(
    job: &super::AutoMLJobV2,
    result: &AutoMLRunResult,
) -> Option<crate::ops::types::AutoMLCandidate> {
    // The candidate the pick named, read rather than indexed: a result holding no candidate
    // at all reports none instead of failing the read.
    let candidate = result.candidates.get(result.best)?;
    Some(crate::ops::types::AutoMLCandidate {
        candidate_name: candidate.name.clone(),
        final_auto_ml_job_objective_metric: candidate.metric(result.objective_metric).map(
            |metric| crate::ops::types::FinalAutoMLJobObjectiveMetric {
                r#type: Some(metric.direction.to_string()),
                metric_name: metric.name.to_string(),
                value: metric.value as f32,
                // "The name of the standard metric": the metric this run measured is one of
                // the standard ones, so the two name the same thing.
                standard_metric_name: Some(metric.name.to_string()),
            },
        ),
        objective_status: OBJECTIVE_STATUS_SUCCEEDED.to_string(),
        candidate_steps: Vec::new(),
        candidate_status: CANDIDATE_STATUS_COMPLETED.to_string(),
        inference_containers: Some(vec![crate::ops::types::AutoMLContainerDefinition {
            image: candidate.inference_image.clone(),
            model_data_url: candidate.model_artifacts.clone(),
            // "The environment variables to set in the container": the candidate's model is
            // read out of the artifact itself, so the container is told nothing beside it.
            environment: None,
        }]),
        creation_time: timestamp(job.creation_time),
        end_time: job.end_time.map(timestamp),
        last_modified_time: timestamp(job.last_modified_time),
        failure_reason: None,
        candidate_properties: Some(crate::ops::types::CandidateProperties {
            // "The Amazon S3 prefix to the explainability artifacts generated for the AutoML
            // candidate" is this structure's REQUIRED member, and this run generates no
            // explainability artifact, so the member stays absent rather than naming a prefix
            // Amazon S3 holds nothing under.
            candidate_artifact_locations: None,
            candidate_metrics: Some(candidate_metric_data(candidate)),
        }),
        // "Populated for the AutoML jobs V2 ... related to image or text classification
        // problem types only" (`InferenceContainerDefinitions`), which this run's candidates
        // are not.
        inference_container_definitions: None,
    })
}

/// `DescribeAutoMLJobV2$ResolvedAttributes` for a job whose run finished: the objective metric
/// the run ranked its candidates by, the completion criteria it ran under and the problem type
/// its analysis resolved.
#[must_use]
pub fn resolved_attributes_report(
    job: &super::AutoMLJobV2,
    result: &AutoMLRunResult,
) -> crate::ops::types::AutoMLResolvedAttributes {
    let requested = problem_type_completion_criteria(&job.problem_type_config);
    crate::ops::types::AutoMLResolvedAttributes {
        // The metric the candidates were actually ranked by, which is the request's when it
        // named one this run measures and the problem type's documented default otherwise.
        auto_ml_job_objective: Some(AutoMLJobObjective {
            metric_name: result.objective_metric.to_string(),
        }),
        completion_criteria: Some(crate::ops::types::AutoMLJobCompletionCriteria {
            // "The maximum number of times a training job is allowed to run": the cap the
            // request asked for, and otherwise the number of candidates this run generates,
            // which is the cap it ran under.
            max_candidates: requested
                .and_then(|criteria| criteria.max_candidates)
                .or_else(|| i32::try_from(result.candidates.len()).ok()),
            max_runtime_per_training_job_in_seconds: requested
                .and_then(|criteria| criteria.max_runtime_per_training_job_in_seconds),
            max_auto_ml_job_runtime_in_seconds: requested
                .and_then(|criteria| criteria.max_auto_ml_job_runtime_in_seconds),
        }),
        auto_ml_problem_type_resolved_attributes: resolved_problem_type_attributes(
            &job.problem_type_config,
            &result.resolved_problem_type,
        ),
    }
}

/// The `AutoMLJobCompletionCriteria` the request's problem-type configuration named, whichever
/// member of the union it is.
fn problem_type_completion_criteria(
    config: &AutoMLProblemTypeConfig,
) -> Option<&crate::ops::types::AutoMLJobCompletionCriteria> {
    match config {
        AutoMLProblemTypeConfig::ImageClassificationJobConfig(config) => {
            config.completion_criteria.as_ref()
        }
        AutoMLProblemTypeConfig::TextClassificationJobConfig(config) => {
            config.completion_criteria.as_ref()
        }
        AutoMLProblemTypeConfig::TimeSeriesForecastingJobConfig(config) => {
            config.completion_criteria.as_ref()
        }
        AutoMLProblemTypeConfig::TabularJobConfig(config) => config.completion_criteria.as_ref(),
        AutoMLProblemTypeConfig::TextGenerationJobConfig(config) => {
            config.completion_criteria.as_ref()
        }
    }
}

/// `AutoMLResolvedAttributes$AutoMLProblemTypeResolvedAttributes`: "the resolved attributes
/// specific to a problem type". The union carries a member for the tabular and the
/// text-generation problem types only, so a job of any other type resolves none.
fn resolved_problem_type_attributes(
    config: &AutoMLProblemTypeConfig,
    resolved: &str,
) -> Option<crate::ops::types::AutoMLProblemTypeResolvedAttributes> {
    match config {
        AutoMLProblemTypeConfig::TabularJobConfig(_) => Some(
            crate::ops::types::AutoMLProblemTypeResolvedAttributes::TabularResolvedAttributes(
                crate::ops::types::TabularResolvedAttributes {
                    problem_type: Some(resolved.to_string()),
                },
            ),
        ),
        AutoMLProblemTypeConfig::TextGenerationJobConfig(config) => Some(
            crate::ops::types::AutoMLProblemTypeResolvedAttributes::TextGenerationResolvedAttributes(
                crate::ops::types::TextGenerationResolvedAttributes {
                    base_model_name: config.base_model_name.clone(),
                },
            ),
        ),
        _ => None,
    }
}
