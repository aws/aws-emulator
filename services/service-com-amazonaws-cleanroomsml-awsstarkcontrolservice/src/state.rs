// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cleanroomsml-awsstarkcontrolservice service.
//!
//! The training dataset and the audience model are the first link of Clean Rooms
//! ML's resource chain: `CreateTrainingDataset` mints a training dataset,
//! `CreateAudienceModel` trains a model from one, the Get operations report them,
//! the List operation pages over them and the Delete operations release them.
//! The records below are the one copy of each resource every one of those
//! handlers reads and writes, plus the summary projections the list operations
//! return.
//!
//! The configured model algorithm is the custom-model link: `CreateConfiguredModelAlgorithm`
//! persists the container configuration a trained model is built from, and
//! `GetConfiguredModelAlgorithm` reports it back from that one record. The
//! association of such an algorithm is held the same way: it is created under a
//! membership and listed through the collaboration that membership belongs to.
//!
//! The ML input channel is the collaboration-scoped link of the same chain: a
//! channel is created under a membership, and every member of the collaboration
//! that membership belongs to can list the channels it holds. A trained model is
//! scoped the same way: it is created under a membership and listed through the
//! collaboration that membership belongs to, and so is a trained model inference
//! job — the run of one trained model over a data source, held here beside the
//! model it ran.
//!
//! Tags are part of the record of the resource they are assigned to, whichever
//! kind of resource that is: `TagResource` and `UntagResource` change that one
//! map and `ListTagsForResource` reports it, so a resource's tags never live in
//! a second place.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

// Clean Rooms' own store, which is where a membership lives: Clean Rooms ML
// resources are created under a membership, and a membership is a Clean Rooms
// resource, so this service reads one from the service that owns it rather than
// keeping a second copy of it here.
use ::service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms;

use crate::ops::types::{
    CollaborationConfiguredModelAlgorithmAssociationSummary, CollaborationMLInputChannelSummary,
    CollaborationTrainedModelInferenceJobSummary, CollaborationTrainedModelSummary,
    ConfiguredModelAlgorithmAssociationSummary, ConflictException, ContainerConfig, Dataset,
    IncrementalTrainingDataChannelOutput, InferenceContainerConfig, InferenceOutputConfiguration,
    PayerConfiguration, PrivacyConfiguration, ResourceNotFoundException, TrainingDatasetSummary,
};

/// The region a minted ARN names when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service segment every Clean Rooms ML resource carries, as
/// `OPERATIONS.md` records it ("ARN namespace: `cleanrooms-ml`") and as
/// `TrainingDatasetArn`'s own pattern requires
/// (`^arn:aws[-a-z]*:cleanrooms-ml:…`).
pub const ARN_SERVICE: &str = "cleanrooms-ml";

/// The largest page `ListTrainingDatasets` serves, and the page size it uses
/// when the request names no `maxResults`:
/// `ListTrainingDatasetsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const TRAINING_DATASET_PAGE_LIMIT: i32 = 100;

/// How many training datasets the store holds before `CreateTrainingDataset`
/// refuses, and how many audience models before `CreateAudienceModel` does.
///
/// Clean Rooms ML publishes adjustable per-account quotas on both, and this
/// emulator models neither Service Quotas nor an account's adjustments — so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is
/// released by that resource's delete, which is what frees room for the next
/// create.
pub const RESOURCE_QUOTA: usize = 1_000;

/// How long an audience model stays in the status `CreateAudienceModel`
/// returned before the clock owes it `ACTIVE`.
///
/// Clean Rooms ML trains the model asynchronously, so `AudienceModelStatus`
/// carries `CREATE_PENDING` / `CREATE_IN_PROGRESS` before `ACTIVE`; real
/// training takes far longer, and the emulator chooses this span. It is
/// deliberately NOT zero, so a caller reading the model straight after the
/// create still observes the transient status. The instant is stored on the
/// record and materialised by whichever later call reads it — nothing sleeps.
pub const AUDIENCE_MODEL_TRAINING: Duration = Duration::from_millis(500);

/// The status an audience model carries from the moment `CreateAudienceModel`
/// accepted it until [`AUDIENCE_MODEL_TRAINING`] has passed.
pub const AUDIENCE_MODEL_CREATING: &str = "CREATE_PENDING";

/// The status an audience model carries once its training window has passed,
/// and the only status `TrainingDatasetStatus` models at all.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The largest page `ListCollaborationMLInputChannels` serves, and the page size
/// it uses when the request names no `maxResults`:
/// `ListCollaborationMLInputChannelsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const ML_INPUT_CHANNEL_PAGE_LIMIT: i32 = 100;

/// The largest page `ListCollaborationTrainedModels` serves, and the page size
/// it uses when the request names no `maxResults`:
/// `ListCollaborationTrainedModelsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const TRAINED_MODEL_PAGE_LIMIT: i32 = 100;

/// The largest page `ListCollaborationTrainedModelInferenceJobs` serves, and the
/// page size it uses when the request names no `maxResults`:
/// `ListCollaborationTrainedModelInferenceJobsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const TRAINED_MODEL_INFERENCE_JOB_PAGE_LIMIT: i32 = 100;

/// The largest page `ListCollaborationConfiguredModelAlgorithmAssociations`
/// serves, and the page size it uses when the request names no `maxResults`:
/// `ListCollaborationConfiguredModelAlgorithmAssociationsRequest$maxResults` is
/// modelled `@range(min: 1, max: 100)`.
pub const CONFIGURED_MODEL_ALGORITHM_ASSOCIATION_PAGE_LIMIT: i32 = 100;

/// A status change the clock owes a resource: the instant it falls due, and the
/// status the resource carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// Materialise the status change `pending` owes `status`, if `now` is past its
/// due instant — the one place a clock-owed transition is settled, whichever
/// record holds it.
///
/// Only the status moves: a resource's `updateTime` reports the last call that
/// changed it, and the work this settles was started by the call that set the
/// transition, which already set that time.
fn settle_pending(status: &mut String, pending: &mut Option<PendingTransition>, now: SystemTime) {
    let Some(transition) = pending.as_ref() else {
        return;
    };
    if now < transition.due {
        return;
    }
    *status = transition.status.clone();
    *pending = None;
}

/// One Clean Rooms ML training dataset, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct TrainingDataset {
    /// The `TrainingDatasetArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. It is the scope
    /// `CreateTrainingDatasetRequest$name`'s documented uniqueness is checked in
    /// ("unique in your account and region").
    pub region: String,
    pub name: String,
    pub role_arn: String,
    pub training_data: Vec<Dataset>,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// One of `TrainingDatasetStatus`'s values, which is `ACTIVE` and nothing
    /// else: a training dataset is metadata pointing at a Glue table, so the
    /// create has nothing to do asynchronously and the resource is ready when
    /// it returns.
    pub status: String,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

/// The stored form of a submitted `DatasetList`: the dataset metadata as the service
/// holds it once the create has resolved what the request left to the service.
///
/// Two things are resolved here, so every later read of the record reports them and no
/// read has to compute anything:
///
/// * `GlueDataSource$catalogId` names the Glue Data Catalog the training data is read
///   from. The member is optional on the wire and the Glue Data Catalog a request leaves
///   unnamed is the one of the account making the call, so the resolved catalog is that
///   account and that is what the record carries.
/// * A dataset's input `schema` is a column set keyed by column name, so it is held in
///   column-name order rather than in the order one request happened to list it in: two
///   creates naming the same columns in different orders describe the same dataset and
///   are reported identically. `sort_by` is stable, so entries that share a name keep
///   their submitted order relative to each other. The model fixes no relation between
///   the submitted and the reported order, so this order is taken from the recorded
///   read-back of a create whose columns were submitted unordered; a recording that
///   showed an unordered submission reported back unchanged would retire this sort
///   rather than refine it.
pub fn stored_training_data(training_data: Vec<Dataset>) -> Vec<Dataset> {
    training_data
        .into_iter()
        .map(|mut dataset| {
            let input_config = &mut dataset.input_config;
            input_config
                .schema
                .sort_by(|left, right| left.column_name.cmp(&right.column_name));
            input_config
                .data_source
                .glue_data_source
                .catalog_id
                .get_or_insert_with(|| ::engine::emulator_account_id().to_string());
            dataset
        })
        .collect()
}

impl TrainingDataset {
    /// The `TrainingDatasetSummary` projection `ListTrainingDatasets` returns.
    pub fn to_summary(&self) -> TrainingDatasetSummary {
        TrainingDatasetSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            training_dataset_arn: self.arn.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
        }
    }
}

/// One Clean Rooms ML audience model, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct AudienceModel {
    /// The `AudienceModelArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    pub name: String,
    /// The training dataset this model was trained from. `CreateAudienceModel`
    /// only accepts one that exists, so this always named a persisted training
    /// dataset at create time.
    pub training_dataset_arn: String,
    /// The training window the create named, held as the modelled timestamp the
    /// request carried so the get reports exactly what was stored.
    pub training_data_start_time: Option<::aws_smithy_types::DateTime>,
    pub training_data_end_time: Option<::aws_smithy_types::DateTime>,
    pub kms_key_arn: Option<String>,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// One of `AudienceModelStatus`'s values.
    pub status: String,
    /// The status change the clock owes this model, if any.
    pub pending: Option<PendingTransition>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl AudienceModel {
    /// Materialise the status change the clock owes this model, if `now` is past
    /// its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending(&mut self.status, &mut self.pending, now);
    }
}

/// One Clean Rooms ML input channel, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct MlInputChannel {
    /// The `MLInputChannelArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    /// The membership the channel was created under, which its ARN names.
    pub membership_identifier: String,
    /// The collaboration that membership belongs to. It is the scope
    /// `ListCollaborationMLInputChannels` reports in: a channel is visible to
    /// that call only through the collaboration it was created in.
    pub collaboration_identifier: String,
    pub name: String,
    /// The configured model algorithm associations the channel was created
    /// from, which `ConfiguredModelAlgorithmAssociationArnList` bounds to one
    /// entry.
    pub configured_model_algorithm_associations: Vec<String>,
    pub description: Option<String>,
    pub payer_configuration: Option<PayerConfiguration>,
    /// The account of the member that created the channel.
    pub creator_account_id: String,
    /// The tags assigned to this channel: `TagResource` writes them,
    /// `UntagResource` removes them and `ListTagsForResource` reports them.
    pub tags: BTreeMap<String, String>,
    /// One of `MLInputChannelStatus`'s values.
    pub status: String,
    /// The status change the clock owes this channel, if any.
    pub pending: Option<PendingTransition>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl MlInputChannel {
    /// Materialise the status change the clock owes this channel, if `now` is
    /// past its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending(&mut self.status, &mut self.pending, now);
    }

    /// The `CollaborationMLInputChannelSummary` projection
    /// `ListCollaborationMLInputChannels` returns.
    pub fn to_collaboration_summary(&self) -> CollaborationMLInputChannelSummary {
        CollaborationMLInputChannelSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            membership_identifier: self.membership_identifier.clone(),
            collaboration_identifier: self.collaboration_identifier.clone(),
            name: self.name.clone(),
            configured_model_algorithm_associations: self
                .configured_model_algorithm_associations
                .clone(),
            ml_input_channel_arn: self.arn.clone(),
            status: self.status.clone(),
            creator_account_id: self.creator_account_id.clone(),
            description: self.description.clone(),
            payer_configuration: self.payer_configuration.clone(),
        }
    }
}

/// One Clean Rooms ML trained model version, as this emulator persists it.
///
/// A trained model is created under a membership, and every member of the
/// collaboration that membership belongs to sees it through the
/// collaboration-scoped reads — which is why the collaboration is held on the
/// record beside the membership its ARN names.
#[derive(Debug, Clone)]
pub struct TrainedModel {
    /// The `TrainedModelArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    /// The membership the model was created under, which its ARN names.
    pub membership_identifier: String,
    /// The collaboration that membership belongs to. It is the scope
    /// `ListCollaborationTrainedModels` reports in: a model is visible to that
    /// call only through the collaboration it was trained in.
    pub collaboration_identifier: String,
    pub name: String,
    /// The version of the trained model this record is, when the model carries
    /// versions: `CollaborationTrainedModelSummary$versionIdentifier` is
    /// optional, so a model with no version reports none.
    pub version_identifier: Option<String>,
    /// The incremental training data channels this version was trained from, as
    /// `IncrementalTrainingDataChannelsOutput` reports them.
    pub incremental_training_data_channels: Option<Vec<IncrementalTrainingDataChannelOutput>>,
    /// The configured model algorithm association the model was trained through.
    pub configured_model_algorithm_association_arn: String,
    pub description: Option<String>,
    /// The account of the member that created the model.
    pub creator_account_id: String,
    /// The account that pays for the training, when the create named one.
    pub ml_model_training_payer_account_id: Option<String>,
    /// The tags assigned to this model, held the same way every other taggable
    /// record in this store holds them.
    pub tags: BTreeMap<String, String>,
    /// One of `TrainedModelStatus`'s values.
    pub status: String,
    /// The status change the clock owes this model, if any.
    pub pending: Option<PendingTransition>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl TrainedModel {
    /// Materialise the status change the clock owes this model, if `now` is past
    /// its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending(&mut self.status, &mut self.pending, now);
    }

    /// The `CollaborationTrainedModelSummary` projection
    /// `ListCollaborationTrainedModels` returns.
    pub fn to_collaboration_summary(&self) -> CollaborationTrainedModelSummary {
        CollaborationTrainedModelSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            trained_model_arn: self.arn.clone(),
            name: self.name.clone(),
            version_identifier: self.version_identifier.clone(),
            incremental_training_data_channels: self.incremental_training_data_channels.clone(),
            description: self.description.clone(),
            membership_identifier: self.membership_identifier.clone(),
            collaboration_identifier: self.collaboration_identifier.clone(),
            status: self.status.clone(),
            configured_model_algorithm_association_arn: self
                .configured_model_algorithm_association_arn
                .clone(),
            creator_account_id: self.creator_account_id.clone(),
            ml_model_training_payer_account_id: self.ml_model_training_payer_account_id.clone(),
        }
    }
}

/// One Clean Rooms ML trained model inference job, as this emulator persists it.
///
/// The job is one run of a trained model over a data source. It is started under
/// a membership, and every member of the collaboration that membership belongs
/// to sees it through the collaboration-scoped read — so the collaboration is
/// held on the record beside the membership its ARN names, exactly as
/// [`TrainedModel`] holds it. The model the job ran, and the version of that
/// model, are held too: they are what the collaboration-scoped list filters on.
#[derive(Debug, Clone)]
pub struct TrainedModelInferenceJob {
    /// The `TrainedModelInferenceJobArn` this record is keyed by.
    pub arn: String,
    /// The region the job was started in, which its ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    /// The membership the job was started under, which its ARN names.
    pub membership_identifier: String,
    /// The collaboration that membership belongs to. It is the scope
    /// `ListCollaborationTrainedModelInferenceJobs` reports in: a job is visible
    /// to that call only through the collaboration it was run in.
    pub collaboration_identifier: String,
    pub name: String,
    /// The trained model the job ran, which
    /// `CollaborationTrainedModelInferenceJobSummary$trainedModelArn` requires.
    pub trained_model_arn: String,
    /// The version of that model the job ran, when the model carries versions:
    /// the summary's `trainedModelVersionIdentifier` is optional, so a job that
    /// ran an unversioned model reports none.
    pub trained_model_version_identifier: Option<String>,
    /// The configured model algorithm association the job ran through, when it
    /// ran one.
    pub configured_model_algorithm_association_arn: Option<String>,
    /// Where the inference output goes, held as the start named it so the read
    /// reports exactly what was stored.
    pub output_configuration: InferenceOutputConfiguration,
    pub description: Option<String>,
    /// The account of the member that started the job.
    pub creator_account_id: String,
    /// The account that pays for the inference, when the start named one.
    pub ml_model_inference_payer_account_id: Option<String>,
    /// One of `MetricsStatus`'s values, once the job has metrics to publish.
    pub metrics_status: Option<String>,
    pub metrics_status_details: Option<String>,
    /// One of `LogsStatus`'s values, once the job has logs to publish.
    pub logs_status: Option<String>,
    pub logs_status_details: Option<String>,
    /// The tags assigned to this job, held the same way every other taggable
    /// record in this store holds them.
    pub tags: BTreeMap<String, String>,
    /// One of `TrainedModelInferenceJobStatus`'s values.
    pub status: String,
    /// The status change the clock owes this job, if any.
    pub pending: Option<PendingTransition>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl TrainedModelInferenceJob {
    /// Materialise the status change the clock owes this job, if `now` is past
    /// its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending(&mut self.status, &mut self.pending, now);
    }

    /// The `CollaborationTrainedModelInferenceJobSummary` projection
    /// `ListCollaborationTrainedModelInferenceJobs` returns.
    pub fn to_collaboration_summary(&self) -> CollaborationTrainedModelInferenceJobSummary {
        CollaborationTrainedModelInferenceJobSummary {
            trained_model_inference_job_arn: self.arn.clone(),
            configured_model_algorithm_association_arn: self
                .configured_model_algorithm_association_arn
                .clone(),
            membership_identifier: self.membership_identifier.clone(),
            trained_model_arn: self.trained_model_arn.clone(),
            trained_model_version_identifier: self.trained_model_version_identifier.clone(),
            collaboration_identifier: self.collaboration_identifier.clone(),
            status: self.status.clone(),
            output_configuration: self.output_configuration.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            metrics_status: self.metrics_status.clone(),
            metrics_status_details: self.metrics_status_details.clone(),
            logs_status: self.logs_status.clone(),
            logs_status_details: self.logs_status_details.clone(),
            ml_model_inference_payer_account_id: self.ml_model_inference_payer_account_id.clone(),
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            creator_account_id: self.creator_account_id.clone(),
        }
    }
}

/// A record one of this service's collaboration-scoped lists pages over.
///
/// `ListCollaborationMLInputChannels`, `ListCollaborationTrainedModels` and
/// `ListCollaborationTrainedModelInferenceJobs` serve the same rule over
/// different resources — report the records of one collaboration that the
/// endpoint's own region holds, ARN order, one page at a time — so the walk
/// lives once in [`collaboration_page`] and each record says here what that
/// walk needs of it.
pub trait CollaborationScoped {
    /// The summary projection that list returns, one per reported record.
    type Summary;

    /// The ARN this record is keyed by, which is the pagination token.
    fn page_key(&self) -> &str;
    /// The region the record was created in.
    fn scope_region(&self) -> &str;
    /// The collaboration the record is reported through.
    fn scope_collaboration(&self) -> &str;
    /// Materialise the status change the clock owes this record, if any is due.
    fn settle_now(&mut self, now: SystemTime);
    /// The summary projection of this record.
    fn summary(&self) -> Self::Summary;
}

impl CollaborationScoped for MlInputChannel {
    type Summary = CollaborationMLInputChannelSummary;

    fn page_key(&self) -> &str {
        &self.arn
    }

    fn scope_region(&self) -> &str {
        &self.region
    }

    fn scope_collaboration(&self) -> &str {
        &self.collaboration_identifier
    }

    fn settle_now(&mut self, now: SystemTime) {
        self.settle(now);
    }

    fn summary(&self) -> Self::Summary {
        self.to_collaboration_summary()
    }
}

impl CollaborationScoped for TrainedModelInferenceJob {
    type Summary = CollaborationTrainedModelInferenceJobSummary;

    fn page_key(&self) -> &str {
        &self.arn
    }

    fn scope_region(&self) -> &str {
        &self.region
    }

    fn scope_collaboration(&self) -> &str {
        &self.collaboration_identifier
    }

    fn settle_now(&mut self, now: SystemTime) {
        self.settle(now);
    }

    fn summary(&self) -> Self::Summary {
        self.to_collaboration_summary()
    }
}

impl CollaborationScoped for TrainedModel {
    type Summary = CollaborationTrainedModelSummary;

    fn page_key(&self) -> &str {
        &self.arn
    }

    fn scope_region(&self) -> &str {
        &self.region
    }

    fn scope_collaboration(&self) -> &str {
        &self.collaboration_identifier
    }

    fn settle_now(&mut self, now: SystemTime) {
        self.settle(now);
    }

    fn summary(&self) -> Self::Summary {
        self.to_collaboration_summary()
    }
}

/// The page size a list serves for the `maxResults` the request carried, where
/// `limit` is the maximum that member's `@range` admits and the page served when
/// the request names none.
///
/// The centralised walk refuses anything outside that range before a handler
/// runs, so the only case left here is an absent value. The `clamp` states that
/// invariant rather than correcting a caller — a value reaching here outside the
/// range is a validator bug, which the debug assertion surfaces instead of
/// silently repairing.
pub fn page_bound(max_results: Option<i32>, limit: i32) -> usize {
    let requested = max_results.unwrap_or(limit);
    debug_assert!(
        (1..=limit).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, limit)).expect("a page size clamped into 1..=limit fits")
}

/// One page of the collaboration-scoped `records`, and the token the next page
/// resumes at when the collaboration holds more.
///
/// `records` is iterated in the store's ARN order, which is the one stable total
/// order the token indexes into. `resume_at` is the token the request carried:
/// the page starts at the first ARN at or after it, so a page is still served
/// when the record that token named was deleted meanwhile. A record is reported
/// only through its own collaboration and its own region's endpoint, carrying the
/// status the clock owes it at `now`.
pub fn collaboration_page<'a, Record: CollaborationScoped + 'a>(
    records: impl Iterator<Item = &'a mut Record>,
    collaboration_identifier: &str,
    region: &str,
    page_size: usize,
    resume_at: Option<&str>,
    now: SystemTime,
) -> (Vec<Record::Summary>, Option<String>) {
    collaboration_page_where(
        records,
        collaboration_identifier,
        region,
        page_size,
        resume_at,
        now,
        |_| true,
    )
}

/// [`collaboration_page`], reporting only the records `reported` admits.
///
/// A collaboration-scoped list whose request carries filter members
/// (`ListCollaborationTrainedModelInferenceJobs` filters on the trained model
/// and its version) narrows the same walk: the filter is applied BEFORE the page
/// fills, so the token a full page carries names the next record the list would
/// report rather than one it would skip.
///
/// A record of this collaboration is settled before `reported` sees it, so a
/// filter reads the record in the same state the summary beside it reports: the
/// status change the clock already owed is materialised whether or not anybody
/// called, so it cannot be a filter's business whether this call came first.
pub fn collaboration_page_where<'a, Record: CollaborationScoped + 'a>(
    records: impl Iterator<Item = &'a mut Record>,
    collaboration_identifier: &str,
    region: &str,
    page_size: usize,
    resume_at: Option<&str>,
    now: SystemTime,
    reported: impl Fn(&Record) -> bool,
) -> (Vec<Record::Summary>, Option<String>) {
    let mut page = Vec::with_capacity(page_size);
    let mut next_token = None;
    for record in records {
        if resume_at.is_some_and(|token| record.page_key() < token) {
            continue;
        }
        if record.scope_region() != region
            || record.scope_collaboration() != collaboration_identifier
        {
            continue;
        }
        // Settled before the filter reads it, and before the page-full check names it
        // as the next token: the transition was owed at `now` regardless of this call.
        record.settle_now(now);
        if !reported(record) {
            continue;
        }
        if page.len() == page_size {
            next_token = Some(record.page_key().to_string());
            break;
        }
        page.push(record.summary());
    }
    (page, next_token)
}

/// What a record a MEMBERSHIP-scoped list reports says about itself: the same
/// walk as [`CollaborationScoped`], over the membership a record was created
/// under rather than the collaboration it is visible in.
///
/// `ListConfiguredModelAlgorithmAssociations` and its sibling membership-scoped
/// lists serve one rule — report the records of one membership that the
/// endpoint's own region holds, ARN order, one page at a time — so the walk lives
/// once in [`membership_page`] and each record says here what that walk needs of
/// it. A record reported by both kinds of list implements both traits, each with
/// its own summary projection, because the two lists return different shapes.
pub trait MembershipScoped {
    /// The summary projection that list returns, one per reported record.
    type Summary;

    /// The ARN this record is keyed by, which is the pagination token.
    fn page_key(&self) -> &str;
    /// The region the record was created in.
    fn scope_region(&self) -> &str;
    /// The membership the record was created under.
    fn scope_membership(&self) -> &str;
    /// Materialise the status change the clock owes this record, if any is due.
    fn settle_now(&mut self, now: SystemTime);
    /// The summary projection of this record.
    fn summary(&self) -> Self::Summary;
}

/// One page of the membership-scoped `records`, and the token the next page
/// resumes at when the membership holds more.
///
/// The same walk as [`collaboration_page`], and for the same reasons: `records`
/// is iterated in the store's ARN order, which is the one stable total order the
/// token indexes into, the page starts at the first ARN at or after the token the
/// request carried, and a record is reported only through its own membership and
/// its own region's endpoint, carrying the status the clock owes it at `now`.
pub fn membership_page<'a, Record: MembershipScoped + 'a>(
    records: impl Iterator<Item = &'a mut Record>,
    membership_identifier: &str,
    region: &str,
    page_size: usize,
    resume_at: Option<&str>,
    now: SystemTime,
) -> (Vec<Record::Summary>, Option<String>) {
    let mut page = Vec::with_capacity(page_size);
    let mut next_token = None;
    for record in records {
        if resume_at.is_some_and(|token| record.page_key() < token) {
            continue;
        }
        if record.scope_region() != region || record.scope_membership() != membership_identifier {
            continue;
        }
        // Settled before the page-full check names it as the next token: the
        // transition was owed at `now` regardless of this call.
        record.settle_now(now);
        if page.len() == page_size {
            next_token = Some(record.page_key().to_string());
            break;
        }
        page.push(record.summary());
    }
    (page, next_token)
}

/// One Clean Rooms ML configured model algorithm, as this emulator persists it.
///
/// The resource is container configuration plus the role that reads the
/// repository, and `GetConfiguredModelAlgorithmResponse` models no status member
/// at all — so the algorithm is ready the moment the create returns and the
/// clock owes it no transition.
#[derive(Debug, Clone)]
pub struct ConfiguredModelAlgorithm {
    /// The `ConfiguredModelAlgorithmArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    pub name: String,
    pub role_arn: String,
    /// The training container the create named, held as the request carried it so
    /// the get reports exactly what was stored.
    pub training_container_config: Option<ContainerConfig>,
    /// The inference container the create named, held the same way.
    pub inference_container_config: Option<InferenceContainerConfig>,
    pub kms_key_arn: Option<String>,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

/// One Clean Rooms ML configured model algorithm association, as this emulator
/// persists it.
///
/// The association is the collaboration-scoped link of the custom-model chain: a
/// configured model algorithm is associated under a membership, and every member
/// of the collaboration that membership belongs to sees it through the
/// collaboration-scoped reads — which is why the collaboration is held on the
/// record beside the membership its ARN names, exactly as [`TrainedModel`] holds
/// it.
///
/// `CollaborationConfiguredModelAlgorithmAssociationSummary` and
/// `GetCollaborationConfiguredModelAlgorithmAssociationResponse` model no status
/// member at all: the association is metadata pointing at an algorithm, so the
/// create has nothing to do asynchronously and the clock owes the record no
/// transition.
#[derive(Debug, Clone)]
pub struct ConfiguredModelAlgorithmAssociation {
    /// The `ConfiguredModelAlgorithmAssociationArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    /// The membership the association was created under, which its ARN names.
    pub membership_identifier: String,
    /// The collaboration that membership belongs to. It is the scope
    /// `ListCollaborationConfiguredModelAlgorithmAssociations` reports in: an
    /// association is visible to that call only through the collaboration it was
    /// created in.
    pub collaboration_identifier: String,
    pub name: String,
    /// The configured model algorithm the association points at.
    pub configured_model_algorithm_arn: String,
    pub description: Option<String>,
    /// The privacy configuration the create named, held as the request carried it
    /// so a read reports exactly what was stored.
    pub privacy_configuration: Option<PrivacyConfiguration>,
    /// The account of the member that created the association.
    pub creator_account_id: String,
    /// The association's tags. `TaggableArn` admits a configured model algorithm
    /// association, so this is the one copy of them the tagging operations read
    /// and write, exactly as every other taggable record here holds its own.
    pub tags: BTreeMap<String, String>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl ConfiguredModelAlgorithmAssociation {
    /// The `CollaborationConfiguredModelAlgorithmAssociationSummary` projection
    /// `ListCollaborationConfiguredModelAlgorithmAssociations` returns.
    pub fn to_collaboration_summary(
        &self,
    ) -> CollaborationConfiguredModelAlgorithmAssociationSummary {
        CollaborationConfiguredModelAlgorithmAssociationSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            configured_model_algorithm_association_arn: self.arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            membership_identifier: self.membership_identifier.clone(),
            collaboration_identifier: self.collaboration_identifier.clone(),
            configured_model_algorithm_arn: self.configured_model_algorithm_arn.clone(),
            creator_account_id: self.creator_account_id.clone(),
        }
    }

    /// The `ConfiguredModelAlgorithmAssociationSummary` projection
    /// `ListConfiguredModelAlgorithmAssociations` returns — the membership-scoped
    /// list, whose summary names no creator account because every association it
    /// reports was created under the one membership the request addressed.
    pub fn to_summary(&self) -> ConfiguredModelAlgorithmAssociationSummary {
        ConfiguredModelAlgorithmAssociationSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            configured_model_algorithm_association_arn: self.arn.clone(),
            configured_model_algorithm_arn: self.configured_model_algorithm_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            membership_identifier: self.membership_identifier.clone(),
            collaboration_identifier: self.collaboration_identifier.clone(),
        }
    }
}

impl CollaborationScoped for ConfiguredModelAlgorithmAssociation {
    type Summary = CollaborationConfiguredModelAlgorithmAssociationSummary;

    fn page_key(&self) -> &str {
        &self.arn
    }

    fn scope_region(&self) -> &str {
        &self.region
    }

    fn scope_collaboration(&self) -> &str {
        &self.collaboration_identifier
    }

    /// Nothing: the association carries no status, so the clock owes it no
    /// transition to materialise.
    fn settle_now(&mut self, _now: SystemTime) {}

    fn summary(&self) -> Self::Summary {
        self.to_collaboration_summary()
    }
}

impl MembershipScoped for ConfiguredModelAlgorithmAssociation {
    type Summary = ConfiguredModelAlgorithmAssociationSummary;

    fn page_key(&self) -> &str {
        &self.arn
    }

    fn scope_region(&self) -> &str {
        &self.region
    }

    fn scope_membership(&self) -> &str {
        &self.membership_identifier
    }

    /// Nothing, for the same reason as the collaboration-scoped walk: the
    /// association carries no status.
    fn settle_now(&mut self, _now: SystemTime) {}

    fn summary(&self) -> Self::Summary {
        self.to_summary()
    }
}

/// Which kind of resource a `TaggableArn` names a record of in this store.
///
/// `TaggableArn` is one pattern over every resource Clean Rooms ML tags, and
/// each kind of resource lives in its own ARN-keyed map here — so resolving the
/// ARN to the kind that holds it is what lets the tagging operations reach one
/// resource's tags without caring which kind it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    TrainingDataset,
    AudienceModel,
    ConfiguredModelAlgorithm,
    ConfiguredModelAlgorithmAssociation,
    MlInputChannel,
    TrainedModel,
    TrainedModelInferenceJob,
}

/// Per-service state for com-amazonaws-cleanroomsml-awsstarkcontrolservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState {
    /// Every training dataset, keyed by its ARN. A `BTreeMap` is what gives
    /// `ListTrainingDatasets` the stable total order its pagination token
    /// indexes into: ids are minted zero-padded, so ARNs sort in the order the
    /// datasets were created.
    pub training_datasets: BTreeMap<String, TrainingDataset>,
    /// Every audience model, keyed by its ARN.
    pub audience_models: BTreeMap<String, AudienceModel>,
    /// Every configured model algorithm, keyed by its ARN, which gives the
    /// list that pages over them the one stable total order its pagination
    /// token indexes into.
    pub configured_model_algorithms: BTreeMap<String, ConfiguredModelAlgorithm>,
    /// Every configured model algorithm association, keyed by its ARN, which
    /// gives the collaboration- and membership-scoped lists that page over them
    /// the one stable total order their pagination tokens index into.
    pub configured_model_algorithm_associations:
        BTreeMap<String, ConfiguredModelAlgorithmAssociation>,
    /// Every ML input channel, keyed by its ARN, which gives the
    /// collaboration- and membership-scoped lists the one stable total order
    /// their pagination tokens index into.
    pub ml_input_channels: BTreeMap<String, MlInputChannel>,
    /// Every trained model, keyed by its ARN, which gives the collaboration- and
    /// membership-scoped lists the one stable total order their pagination
    /// tokens index into.
    pub trained_models: BTreeMap<String, TrainedModel>,
    /// Every trained model inference job, keyed by its ARN, for the same reason:
    /// the collaboration- and membership-scoped lists that page over the jobs
    /// carry that ARN in their pagination token.
    pub trained_model_inference_jobs: BTreeMap<String, TrainedModelInferenceJob>,
    /// The counter the resource segment of every minted ARN comes from.
    resource_id_seq: u64,
}

impl ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState {
    /// Mint the resource id a new ARN carries: the hyphenated 32-hex-digit form
    /// Clean Rooms ML gives the resource segment of a training dataset or an
    /// audience model ARN, which `TrainingDatasetArn`'s
    /// `training-dataset/[-a-zA-Z0-9_/.]+` admits.
    ///
    /// Minted from a counter rather than at random, so the ids a run hands out
    /// are distinct, reproducible, and sort in the order they were minted.
    pub fn mint_resource_id(&mut self) -> String {
        self.resource_id_seq = self.resource_id_seq.wrapping_add(1);
        let hex = format!("{:032x}", self.resource_id_seq);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The ARNs of the persisted audience models trained from the training
    /// dataset `arn` names — the dependency `DeleteTrainingDataset` documents
    /// ("You can't delete a training dataset if there are any audience models
    /// that depend on the training dataset").
    pub fn audience_models_depending_on(&self, arn: &str) -> Vec<String> {
        self.audience_models
            .values()
            .filter(|model| model.training_dataset_arn == arn)
            .map(|model| model.arn.clone())
            .collect()
    }

    /// The training dataset `arn` names, if the endpoint `region` serves it.
    ///
    /// A Clean Rooms ML endpoint is regional: it reports the resources created
    /// under its own region and nothing else, so a record from another region is
    /// invisible here even though its ARN is well formed and the store holds it.
    pub fn training_dataset(&self, arn: &str, region: &str) -> Option<&TrainingDataset> {
        self.training_datasets
            .get(arn)
            .filter(|dataset| dataset.region == region)
    }

    /// The configured model algorithm `arn` names, if the endpoint `region`
    /// serves it — the same regional scope as [`Self::training_dataset`].
    pub fn configured_model_algorithm(
        &self,
        arn: &str,
        region: &str,
    ) -> Option<&ConfiguredModelAlgorithm> {
        self.configured_model_algorithms
            .get(arn)
            .filter(|algorithm| algorithm.region == region)
    }

    /// The configured model algorithm association `arn` names, if the endpoint
    /// `region` serves it — the same regional scope as
    /// [`Self::training_dataset`].
    pub fn configured_model_algorithm_association(
        &self,
        arn: &str,
        region: &str,
    ) -> Option<&ConfiguredModelAlgorithmAssociation> {
        self.configured_model_algorithm_associations
            .get(arn)
            .filter(|association| association.region == region)
    }

    /// Which kind of resource the endpoint `region`'s store holds `arn` under,
    /// or `None` when it holds none.
    ///
    /// Every record is keyed by its own ARN and an ARN names exactly one
    /// resource, so at most one of these maps can hold one ARN. The region is
    /// checked for the same reason every other read of this service checks it:
    /// a Clean Rooms ML endpoint serves the resources created under its own
    /// region and nothing else.
    pub fn tagged_resource_kind(&self, arn: &str, region: &str) -> Option<TaggedResourceKind> {
        self.training_datasets
            .get(arn)
            .map(|record| (TaggedResourceKind::TrainingDataset, record.region.as_str()))
            .or_else(|| {
                self.audience_models
                    .get(arn)
                    .map(|record| (TaggedResourceKind::AudienceModel, record.region.as_str()))
            })
            .or_else(|| {
                self.configured_model_algorithms.get(arn).map(|record| {
                    (
                        TaggedResourceKind::ConfiguredModelAlgorithm,
                        record.region.as_str(),
                    )
                })
            })
            .or_else(|| {
                self.configured_model_algorithm_associations
                    .get(arn)
                    .map(|record| {
                        (
                            TaggedResourceKind::ConfiguredModelAlgorithmAssociation,
                            record.region.as_str(),
                        )
                    })
            })
            .or_else(|| {
                self.ml_input_channels
                    .get(arn)
                    .map(|record| (TaggedResourceKind::MlInputChannel, record.region.as_str()))
            })
            .or_else(|| {
                self.trained_models
                    .get(arn)
                    .map(|record| (TaggedResourceKind::TrainedModel, record.region.as_str()))
            })
            .or_else(|| {
                self.trained_model_inference_jobs.get(arn).map(|record| {
                    (
                        TaggedResourceKind::TrainedModelInferenceJob,
                        record.region.as_str(),
                    )
                })
            })
            .filter(|(_, held)| *held == region)
            .map(|(kind, _)| kind)
    }

    /// The tags the endpoint `region`'s store holds for the resource `arn`
    /// names, or `None` when it holds no such resource.
    ///
    /// A resource's tags are part of that resource's record, so this is the one
    /// copy of them that every read reports and every write changes.
    pub fn resource_tags(&self, arn: &str, region: &str) -> Option<&BTreeMap<String, String>> {
        Some(match self.tagged_resource_kind(arn, region)? {
            TaggedResourceKind::TrainingDataset => &self.training_datasets.get(arn)?.tags,
            TaggedResourceKind::AudienceModel => &self.audience_models.get(arn)?.tags,
            TaggedResourceKind::ConfiguredModelAlgorithm => {
                &self.configured_model_algorithms.get(arn)?.tags
            }
            TaggedResourceKind::ConfiguredModelAlgorithmAssociation => {
                &self.configured_model_algorithm_associations.get(arn)?.tags
            }
            TaggedResourceKind::MlInputChannel => &self.ml_input_channels.get(arn)?.tags,
            TaggedResourceKind::TrainedModel => &self.trained_models.get(arn)?.tags,
            TaggedResourceKind::TrainedModelInferenceJob => {
                &self.trained_model_inference_jobs.get(arn)?.tags
            }
        })
    }

    /// The same tag set, for the operations that change it: `TagResource` adds
    /// to it and `UntagResource` removes from it.
    pub fn resource_tags_mut(
        &mut self,
        arn: &str,
        region: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        Some(match self.tagged_resource_kind(arn, region)? {
            TaggedResourceKind::TrainingDataset => &mut self.training_datasets.get_mut(arn)?.tags,
            TaggedResourceKind::AudienceModel => &mut self.audience_models.get_mut(arn)?.tags,
            TaggedResourceKind::ConfiguredModelAlgorithm => {
                &mut self.configured_model_algorithms.get_mut(arn)?.tags
            }
            TaggedResourceKind::ConfiguredModelAlgorithmAssociation => {
                &mut self
                    .configured_model_algorithm_associations
                    .get_mut(arn)?
                    .tags
            }
            TaggedResourceKind::MlInputChannel => &mut self.ml_input_channels.get_mut(arn)?.tags,
            TaggedResourceKind::TrainedModel => &mut self.trained_models.get_mut(arn)?.tags,
            TaggedResourceKind::TrainedModelInferenceJob => {
                &mut self.trained_model_inference_jobs.get_mut(arn)?.tags
            }
        })
    }

    /// The audience model `arn` names, if the endpoint `region` serves it, with
    /// any status change the clock owes it already materialised.
    pub fn settled_audience_model_mut(
        &mut self,
        arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut AudienceModel> {
        let model = self.audience_models.get_mut(arn)?;
        if model.region != region {
            return None;
        }
        model.settle(now);
        Some(model)
    }
}

/// The ARN of a training dataset with resource id `id`, in `region`.
pub fn training_dataset_arn(id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("training-dataset/{id}"))
}

/// The ARN of an audience model with resource id `id`, in `region`.
pub fn audience_model_arn(id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("audience-model/{id}"))
}

/// The ARN of a configured model algorithm with resource id `id`, in `region`.
pub fn configured_model_algorithm_arn(id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("configured-model-algorithm/{id}"),
    )
}

/// The ARN of the ML input channel with resource id `id`, created under the
/// membership `membership_identifier` names, in `region`.
///
/// `MLInputChannelArn` nests the channel under its membership
/// (`membership/{uuid}/ml-input-channel/{id}`), so the membership a channel
/// belongs to is readable from its ARN.
pub fn ml_input_channel_arn(membership_identifier: &str, id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_identifier}/ml-input-channel/{id}"),
    )
}

/// The ARN of the trained model with resource id `id`, created under the
/// membership `membership_identifier` names, in `region`.
///
/// `TrainedModelArn` nests the model under its membership
/// (`membership/{uuid}/trained-model/{id}`), so the membership a model belongs
/// to is readable from its ARN.
pub fn trained_model_arn(membership_identifier: &str, id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_identifier}/trained-model/{id}"),
    )
}

/// The ARN of the trained model inference job with resource id `id`, started
/// under the membership `membership_identifier` names, in `region`.
///
/// `TrainedModelInferenceJobArn` nests the job under its membership
/// (`membership/{uuid}/trained-model-inference-job/{id}`), so the membership a
/// job belongs to is readable from its ARN.
pub fn trained_model_inference_job_arn(
    membership_identifier: &str,
    id: &str,
    region: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_identifier}/trained-model-inference-job/{id}"),
    )
}

/// The ARN of the configured model algorithm association with resource id `id`,
/// created under the membership `membership_identifier` names, in `region`.
///
/// `ConfiguredModelAlgorithmAssociationArn` nests the association under its
/// membership (`membership/{uuid}/configured-model-algorithm-association/{id}`),
/// so the membership an association belongs to is readable from its ARN.
pub fn configured_model_algorithm_association_arn(
    membership_identifier: &str,
    id: &str,
    region: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_identifier}/configured-model-algorithm-association/{id}"),
    )
}

/// The resource portion of `value` when it is shaped like a Clean Rooms ML ARN
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:` and a non-empty
/// remainder), and `None` otherwise.
///
/// Every ARN shape in this service shares that prefix and differs only in the
/// resource portion, so each predicate below checks the prefix through this and
/// then its own resource.
fn cleanrooms_ml_arn_resource(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let partition_tail_ok = partition
        .strip_prefix("aws")
        .is_some_and(|tail| tail.bytes().all(|b| b == b'-' || b.is_ascii_lowercase()));
    let region_ok = !region.is_empty()
        && region
            .bytes()
            .all(|b| b == b'-' || b.is_ascii_lowercase() || b.is_ascii_digit());
    let account_ok = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    (partition_tail_ok && service == ARN_SERVICE && region_ok && account_ok).then_some(resource)
}

/// True when `id` is shaped like the resource id Clean Rooms ML ARNs carry,
/// `[-a-zA-Z0-9_/.]+`.
fn is_arn_resource_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|b| {
            b == b'-' || b == b'_' || b == b'/' || b == b'.' || b.is_ascii_alphanumeric()
        })
}

/// True when `value` is shaped like a modelled `UUID`
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), the form
/// every collaboration and membership identifier in this service carries.
fn is_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = value.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if group.len() != width || !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// True when `value` is shaped like a `TrainingDatasetArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:training-dataset/[-a-zA-Z0-9_/.]+$`).
///
/// The boundary validator already refuses a modelled `TrainingDatasetArn`
/// member, so this is for a value the model does not constrain to that
/// pattern — a `ListTrainingDatasets` `nextToken`, which this service's
/// pagination carries a training dataset ARN in.
pub fn is_training_dataset_arn(value: &str) -> bool {
    cleanrooms_ml_arn_resource(value).is_some_and(|resource| {
        resource
            .strip_prefix("training-dataset/")
            .is_some_and(is_arn_resource_id)
    })
}

/// True when `value` is shaped like one of this service's membership-nested ARNs
/// — `arn:…:cleanrooms-ml:…:membership/{uuid}/{kind}/[-a-zA-Z0-9_/.]+`, where
/// `kind` is the resource segment that follows the membership
/// (`ml-input-channel`, `trained-model`, `trained-model-inference-job`).
///
/// Every membership-nested shape in this service differs only in that segment,
/// so each predicate below names its own and shares this check.
fn is_membership_nested_arn(value: &str, kind: &str) -> bool {
    let Some(resource) = cleanrooms_ml_arn_resource(value) else {
        return false;
    };
    let Some(nested) = resource.strip_prefix("membership/") else {
        return false;
    };
    let Some((membership_identifier, id)) = nested.split_once(&format!("/{kind}/")) else {
        return false;
    };
    is_uuid(membership_identifier) && is_arn_resource_id(id)
}

/// True when `value` is shaped like an `MLInputChannelArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:membership/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/ml-input-channel/[-a-zA-Z0-9_/.]+$`).
///
/// As with [`is_training_dataset_arn`], this is for a value the model does not
/// constrain to that pattern — the `nextToken` of a list that pages over ML
/// input channels, which carries the ARN the next page resumes at.
pub fn is_ml_input_channel_arn(value: &str) -> bool {
    is_membership_nested_arn(value, "ml-input-channel")
}

/// True when `value` is shaped like a `TrainedModelArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:membership/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/trained-model/[-a-zA-Z0-9_/.]+$`),
/// for the same reason: it is the `nextToken` of a list that pages over trained
/// models, which carries the ARN the next page resumes at.
pub fn is_trained_model_arn(value: &str) -> bool {
    is_membership_nested_arn(value, "trained-model")
}

/// True when `value` is shaped like a `TrainedModelInferenceJobArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:membership/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/trained-model-inference-job/[-a-zA-Z0-9_/.]+$`),
/// for the same reason: it is the `nextToken` of a list that pages over trained
/// model inference jobs, which carries the ARN the next page resumes at.
pub fn is_trained_model_inference_job_arn(value: &str) -> bool {
    is_membership_nested_arn(value, "trained-model-inference-job")
}

/// True when `value` is shaped like a `ConfiguredModelAlgorithmAssociationArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:membership/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/configured-model-algorithm-association/[-a-zA-Z0-9_/.]+$`),
/// for the same reason: it is the `nextToken` of a list that pages over
/// configured model algorithm associations, which carries the ARN the next page
/// resumes at.
pub fn is_configured_model_algorithm_association_arn(value: &str) -> bool {
    is_membership_nested_arn(value, "configured-model-algorithm-association")
}

/// The region this request was signed for — what a minted ARN names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The identifier of the collaboration the membership `membership_identifier`
/// belongs to, or `None` when the endpoint `region` serves no such membership.
///
/// A configured model algorithm association is created under a membership and is
/// visible to every member of the collaboration that membership belongs to, so
/// the collaboration is READ from the membership Clean Rooms persisted — never
/// derived from the identifier the request carried. A membership's ARN names the
/// region it was created in, and a Clean Rooms ML endpoint serves its own region
/// only, so a membership from another region is invisible here.
pub fn membership_collaboration<Eng: Send + Sync + 'static>(
    engine: &Eng,
    membership_identifier: &str,
    region: &str,
) -> Option<String> {
    let state = cleanrooms::service_state(engine)?;
    let store = state.lock();
    let membership = store.memberships.get(membership_identifier)?;
    (membership.arn == cleanrooms::membership_arn(region, membership_identifier))
        .then(|| membership.collaboration_id.clone())
}

/// The name Clean Rooms ML gives each kind of resource in the refusal
/// [`resource_not_found`] builds: the service's own display name for the kind,
/// as its model names the shape the identifier belongs to.
pub const KIND_TRAINING_DATASET: &str = "Training Dataset";
/// See [`KIND_TRAINING_DATASET`].
pub const KIND_AUDIENCE_MODEL: &str = "Audience Model";
/// See [`KIND_TRAINING_DATASET`].
pub const KIND_CONFIGURED_MODEL_ALGORITHM: &str = "Configured Model Algorithm";
/// See [`KIND_TRAINING_DATASET`].
pub const KIND_CONFIGURED_MODEL_ALGORITHM_ASSOCIATION: &str =
    "Configured Model Algorithm Association";
/// See [`KIND_TRAINING_DATASET`].
pub const KIND_MEMBERSHIP: &str = "Membership";

/// The `ResourceNotFoundException` payload for an identifier that names no
/// persisted resource of `kind`.
///
/// `ResourceNotFoundException` models `message` and nothing else, so the message
/// is where both go: every refusal of this class names the KIND of resource that
/// was looked for and the identifier the request carried for it. The SENTENCE the
/// two are stated in belongs to the kind rather than to the service — Clean Rooms ML
/// grew a resource type at a time, and each type's own control plane words its
/// not-found refusal ([`KINDS_WORDED_AS_NOT_FOUND`]) — so a new kind brings the
/// wording recorded for it rather than inheriting one.
pub fn resource_not_found(kind: &str, identifier: &str) -> ResourceNotFoundException {
    let message = if KINDS_WORDED_AS_NOT_FOUND.contains(&kind) {
        format!("{kind}: {identifier} not found.")
    } else {
        format!("{kind} {identifier} does not exist.")
    };
    ResourceNotFoundException { message }
}

/// The kinds whose not-found refusal is worded `<kind>: <identifier> not found.` rather
/// than `<kind> <identifier> does not exist.`
///
/// Recorded behavior, kind by kind: the configured-model-algorithm resource answers the
/// first wording — wherever one is resolved, both on a read of the algorithm itself and on
/// a create that references one — and the kinds left out of this list answer the second.
const KINDS_WORDED_AS_NOT_FOUND: [&str; 1] = [KIND_CONFIGURED_MODEL_ALGORITHM];

/// The message the TAGGING path's not-found refusal carries.
///
/// The three tagging operations are served by the front end in front of this service,
/// which answers an ARN that names no resource with its own refusal — the error code
/// `NotFoundException`, which no operation of this service models, and this one fixed
/// string, as the behavioural recordings of these operations show — rather than with the
/// control plane's modelled `ResourceNotFoundException` and [`resource_not_found`]. It
/// names no identifier, so the refusal echoes nothing the caller sent.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The tagging path's refusal for an ARN that names no persisted resource.
pub fn tagged_resource_not_found() -> crate::scaffolded::NotFoundException {
    crate::scaffolded::NotFoundException::new(TAGGED_RESOURCE_NOT_FOUND_MESSAGE)
}

/// The `ConflictException` payload for a resource another resource depends on.
pub fn dependency_conflict(arn: &str, dependents: &[String]) -> ConflictException {
    ConflictException {
        message: format!(
            "Resource {arn} cannot be deleted because {} resource(s) depend on it: {}",
            dependents.len(),
            dependents.join(", ")
        ),
    }
}

/// `tags` as the modelled `TagMap`, which the generated types hold as a
/// `HashMap`.
pub fn tag_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>)
}
