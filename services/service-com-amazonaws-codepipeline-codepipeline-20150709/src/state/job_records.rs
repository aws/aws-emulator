// SPDX-License-Identifier: CC0-1.0

//! The JOB records this store holds: the job a `Custom`-owned action's worker polls for,
//! the one a partner's `ThirdParty` integration polls for, and how each is reported.
//!
//! Split out of [`super`] so one file holds one concern (and so neither file grows past
//! the project's file-length limit); every name stays re-exported from `crate::state`,
//! which is where the handlers reach it. What a worker REPORTS about a job it holds — the
//! acknowledgement and the two results — is the lifecycle in [`super::jobs`].

use super::*;

/// One job waiting for a CUSTOM action's job worker — what `PollForJobs` hands out.
///
/// `PollForJobs` "is valid only for action types with Custom in the owner field":
/// CodePipeline does not run a custom action, the account's own job worker does. So
/// when an execution reaches such an action the execution plane queues this record and
/// the action stays `InProgress` until a worker reports its result; what the worker
/// needs to do the work (the action's configuration, the artifacts it consumes and the
/// ones it is to produce, the pipeline it belongs to) is captured here from the
/// declaration the execution pinned, so the job a poll reports is computed from stored
/// state rather than from the poll.
#[derive(Debug, Clone)]
pub struct JobRecord {
    /// The job's id, matching `JobId`'s modelled pattern
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
    pub job_id: String,
    /// The custom action type whose worker this job waits for.
    pub action_type: ActionTypeId,
    /// The pipeline the action belongs to.
    pub pipeline_name: String,
    /// That pipeline's ARN — `JobData$pipelineContext` documents that it "includes
    /// pipelineArn and pipelineExecutionId for custom jobs".
    pub pipeline_arn: String,
    /// The execution that reached the action.
    pub pipeline_execution_id: String,
    /// The stage that contains the action.
    pub stage_name: String,
    /// The declared action's name.
    pub action_name: String,
    /// The id the action's execution is reported under.
    pub action_execution_id: String,
    /// Where the action sits in its execution's plan: the stage's index.
    pub stage: usize,
    /// Where the action sits in that stage: the action's index.
    pub action: usize,
    /// The action's `configuration`, with the execution's variables resolved — what
    /// `JobData$actionConfiguration` reports, and what a `queryParam` poll selects on.
    pub configuration: BTreeMap<String, String>,
    /// The artifacts supplied to the job, at the artifact-store locations the earlier
    /// actions produced them in.
    pub input_artifacts: Vec<Artifact>,
    /// The artifacts the job is to produce, at the artifact-store locations they are
    /// to be written to.
    pub output_artifacts: Vec<Artifact>,
    /// The customer managed key the pipeline's artifact store declares, when it
    /// declares one: the worker needs it to read and write those artifacts.
    pub encryption_key: Option<EncryptionKey>,
    /// A `JobStatus` value: where the job is between being queued and a worker
    /// reporting its outcome.
    pub status: String,
    /// The `nonce` the newest poll published for this job, or `None` while no poll
    /// has reported it. The worker presents it to `AcknowledgeJob`.
    pub nonce: Option<String>,
    /// Every nonce a poll has published for this job while it was still waiting to be
    /// claimed, oldest first — each of them still claims it.
    ///
    /// A redelivery does not invalidate the nonce of an earlier dispatch: the nonce is
    /// there so that "the job is being worked on by only one job worker", and what
    /// settles which worker that is is the ACKNOWLEDGEMENT, so until one of them
    /// claims the job every nonce issued for it reaches it ([`Self::admits_nonce`])
    /// and the claim closes the whole set ([`Self::claim`]). Bounded by
    /// [`MAX_OUTSTANDING_NONCES`]: a job may be re-dispatched once per lease for as
    /// long as no worker claims it, so the set is trimmed oldest-first rather than
    /// grown per poll.
    pub outstanding_nonces: Vec<String>,
    /// When the claim the newest poll granted lapses, so the job may be reported to
    /// another worker again (see [`JOB_POLL_LEASE`]).
    pub leased_until: Option<SystemTime>,
    /// The continuation token the last result supplied, identifying a custom action
    /// still in progress.
    pub continuation_token: Option<String>,
    /// When the job was queued — the order the jobs of one action type are handed out
    /// in, and which job the [`MAX_QUEUED_JOBS`] bound drops first.
    pub queued_at: SystemTime,
    /// When this job's status last changed.
    pub last_status_change: SystemTime,
    /// The temporary credentials a worker reads and writes this job's artifacts with,
    /// minted when the job was handed out.
    ///
    /// `JobData$artifactCredentials` is "an Amazon Web Services session credentials
    /// object" whose credentials "can be used to access input and output artifacts in the
    /// S3 bucket used to store artifact for the pipeline", and `GetJobDetails` documents
    /// that "CodePipeline returns temporary credentials for the S3 bucket used to store
    /// artifacts for the pipeline, if the action requires access to that S3 bucket for
    /// input or output artifacts" — so the DISPATCH is what mints them and every later
    /// read of the job reports the ones that dispatch published, rather than a fresh set
    /// per read.
    ///
    /// `None` until a poll hands the job out, and for a job whose action reads and writes
    /// no artifact at all.
    pub artifact_credentials: Option<AWSSessionCredentials>,
    /// The result a worker reported that carried this job to its terminal status, or
    /// `None` while no result has terminated it.
    ///
    /// Retained because the refusal a LATER result is answered with is decided against
    /// it: a job "cannot be updated with the same status, but different result or output
    /// variables", so a report restating the result the job already holds asks for
    /// nothing the job has not already recorded, while one differing from it is the
    /// state error that sentence states. See [`ReportedJobResult`].
    pub reported_result: Option<ReportedJobResult>,
}

/// The RESULT a job worker reported for a job, kept beside the status that result put
/// the job in.
///
/// `PutJobSuccessResult` and `PutJobFailureResult` each state an outcome plus what the
/// worker observed producing it — the revision it worked on, the execution details, the
/// output variables it published, or the failure it hit — and CodePipeline holds a job to
/// ONE such statement: the same status may not be reported again "with the same status,
/// but different result or output variables". Deciding that needs the statement the job
/// already carries, so it is recorded here, flattened to the members the two results are
/// built from (every one of them from the REQUEST, none invented) so two reports can be
/// compared member by member.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedJobResult {
    /// `PutJobSuccessResult$continuationToken`, when the result carried one.
    pub continuation_token: Option<String>,
    /// `CurrentRevision$revision`, when the result named the revision worked on.
    pub revision: Option<String>,
    /// `CurrentRevision$changeIdentifier`.
    pub change_identifier: Option<String>,
    /// `CurrentRevision$revisionSummary`.
    pub revision_summary: Option<String>,
    /// `CurrentRevision$created`, as the seconds and the subsecond nanoseconds the
    /// timestamp carries — the instant itself, compared exactly as it was stated.
    pub revision_created: Option<(i64, u32)>,
    /// `ExecutionDetails$summary`.
    pub summary: Option<String>,
    /// `ExecutionDetails$externalExecutionId`, or `FailureDetails$externalExecutionId`
    /// for a failure.
    pub external_execution_id: Option<String>,
    /// `ExecutionDetails$percentComplete`.
    pub percent_complete: Option<i32>,
    /// `FailureDetails$type`, the `FailureType` a failure stated.
    pub failure_type: Option<String>,
    /// `FailureDetails$message`.
    pub failure_message: Option<String>,
    /// `PutJobSuccessResult$outputVariables`, in name order so one result is one
    /// statement however the request ordered its map.
    pub output_variables: BTreeMap<String, String>,
}

impl ReportedJobResult {
    /// The statement a `PutJobSuccessResult` made.
    pub fn success(
        continuation_token: Option<&str>,
        current_revision: Option<&CurrentRevision>,
        execution_details: Option<&ExecutionDetails>,
        output_variables: Option<&HashMap<String, String>>,
    ) -> Self {
        Self {
            continuation_token: continuation_token.map(str::to_string),
            revision: current_revision.map(|revision| revision.revision.clone()),
            change_identifier: current_revision
                .map(|revision| revision.change_identifier.clone()),
            revision_summary: current_revision
                .and_then(|revision| revision.revision_summary.clone()),
            revision_created: current_revision
                .and_then(|revision| revision.created)
                .map(|created| (created.secs(), created.subsec_nanos())),
            summary: execution_details.and_then(|details| details.summary.clone()),
            external_execution_id: execution_details
                .and_then(|details| details.external_execution_id.clone()),
            percent_complete: execution_details.and_then(|details| details.percent_complete),
            failure_type: None,
            failure_message: None,
            output_variables: output_variables
                .map(|variables| {
                    variables
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The statement a `PutJobFailureResult` made.
    pub fn failure(failure: &FailureDetails) -> Self {
        Self {
            failure_type: Some(failure.r#type.clone()),
            failure_message: Some(failure.message.clone()),
            external_execution_id: failure.external_execution_id.clone(),
            ..Self::default()
        }
    }
}

impl JobRecord {
    /// A job of `action_type` queued at `now`, with nothing of a pipeline recorded
    /// against it yet.
    ///
    /// The job starts at [`JOB_STATUS_QUEUED`] and unleased: it is waiting for a
    /// worker, and no worker owns its outcome until one polls for it and
    /// acknowledges it. The execution plane fills in the pipeline context it queues
    /// the job with.
    pub fn new(job_id: impl Into<String>, action_type: ActionTypeId, now: SystemTime) -> Self {
        Self {
            job_id: job_id.into(),
            action_type,
            pipeline_name: String::new(),
            pipeline_arn: String::new(),
            pipeline_execution_id: String::new(),
            stage_name: String::new(),
            action_name: String::new(),
            action_execution_id: String::new(),
            stage: 0,
            action: 0,
            configuration: BTreeMap::new(),
            input_artifacts: Vec::new(),
            output_artifacts: Vec::new(),
            encryption_key: None,
            status: JOB_STATUS_QUEUED.to_string(),
            nonce: None,
            outstanding_nonces: Vec::new(),
            leased_until: None,
            continuation_token: None,
            queued_at: now,
            last_status_change: now,
            artifact_credentials: None,
            reported_result: None,
        }
    }

    /// Whether this job's worker needs credentials for the artifact store at all: it does
    /// exactly when the action it performs reads or writes an artifact.
    pub fn needs_artifact_credentials(&self) -> bool {
        !self.input_artifacts.is_empty() || !self.output_artifacts.is_empty()
    }

    /// This job as `PollForJobs` reports it to the worker it was just handed to.
    pub fn to_job(&self) -> Job {
        Job {
            id: Some(self.job_id.clone()),
            data: Some(self.to_data()),
            nonce: self.nonce.clone(),
            // "The ID of the Amazon Web Services account to use when performing the
            // job": the account the pipeline lives in, which is the one account this
            // emulator serves.
            account_id: Some(::engine::emulator_account_id().to_string()),
        }
    }

    /// This job as `GetJobDetails` reports it.
    ///
    /// The same job, read back: `JobDetails` carries everything `Job` does except the
    /// `nonce`, which is the claim one poll granted rather than a property of the job.
    pub fn to_details(&self) -> crate::ops::types::JobDetails {
        crate::ops::types::JobDetails {
            id: Some(self.job_id.clone()),
            data: Some(self.to_data()),
            account_id: Some(::engine::emulator_account_id().to_string()),
        }
    }

    /// What a worker needs to perform this job, as `JobData` models it.
    pub fn to_data(&self) -> JobData {
        JobData {
            action_type_id: Some(self.action_type.clone()),
            action_configuration: Some(ActionConfiguration {
                configuration: (!self.configuration.is_empty()).then(|| {
                    self.configuration
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect()
                }),
            }),
            pipeline_context: Some(PipelineContext {
                pipeline_name: Some(self.pipeline_name.clone()),
                stage: Some(StageContext {
                    name: Some(self.stage_name.clone()),
                }),
                action: Some(ActionContext {
                    name: Some(self.action_name.clone()),
                    action_execution_id: Some(self.action_execution_id.clone()),
                }),
                pipeline_arn: Some(self.pipeline_arn.clone()),
                pipeline_execution_id: Some(self.pipeline_execution_id.clone()),
            }),
            input_artifacts: optional_list(self.input_artifacts.clone()),
            output_artifacts: optional_list(self.output_artifacts.clone()),
            // The credentials the DISPATCH minted for this job (see
            // [`Self::artifact_credentials`]): absent until a worker was handed the job,
            // and for a job whose action reads and writes no artifact.
            artifact_credentials: self.artifact_credentials.clone(),
            continuation_token: self.continuation_token.clone(),
            encryption_key: self.encryption_key.clone(),
        }
    }

    /// Whether this job may be handed to a polling worker at `now`: it is still
    /// waiting for one, and no claim on it is in force.
    pub fn is_available(&self, now: SystemTime) -> bool {
        self.status == JOB_STATUS_QUEUED
            && self.leased_until.is_none_or(|until| until <= now)
    }

    /// Hand this job to the worker that is polling: mint the `nonce` that worker
    /// presents to `AcknowledgeJob`, and hold the job for [`JOB_POLL_LEASE`].
    ///
    /// The nonce is minted per lease and is unpredictable, so the claim it grants
    /// cannot be taken by a worker that was never handed the job. A job that is handed
    /// out again after its lease lapsed carries a FRESH nonce, and the earlier one
    /// stays outstanding: a redelivery does not take the job away from the worker that
    /// was handed it first (see [`Self::outstanding_nonces`]).
    pub fn lease(&mut self, now: SystemTime) {
        let nonce = random_uuid_v4();
        self.outstanding_nonces.push(nonce.clone());
        let overflow = self
            .outstanding_nonces
            .len()
            .saturating_sub(MAX_OUTSTANDING_NONCES);
        self.outstanding_nonces.drain(..overflow);
        self.nonce = Some(nonce);
        self.leased_until = Some(now + JOB_POLL_LEASE);
    }

    /// Whether `nonce` is one of the values that still claims this job.
    ///
    /// Every nonce a poll published for the job while it was unclaimed does, and none
    /// does once a claim has closed them ([`Self::claim`]).
    pub fn admits_nonce(&self, nonce: &str) -> bool {
        self.outstanding_nonces
            .iter()
            .any(|issued| issued == nonce)
    }

    /// Record that the worker presenting `nonce` has claimed this job: it is being
    /// worked on by that worker, and the nonces of every other dispatch stop claiming
    /// it, which is what makes the claim exclusive.
    pub fn claim(&mut self, nonce: &str, now: SystemTime) {
        self.status = JOB_STATUS_IN_PROGRESS.to_string();
        self.nonce = Some(nonce.to_string());
        self.outstanding_nonces.clear();
        self.last_status_change = now;
    }

    /// Put this job back to waiting for a worker, with no claim outstanding: the
    /// nonces of the claim that has just ended reach it no more, and the next poll
    /// mints the one that does.
    pub fn release(&mut self) {
        self.nonce = None;
        self.leased_until = None;
        self.outstanding_nonces.clear();
    }

    /// Whether this job's action configuration satisfies a poll's `queryParam` pair.
    ///
    /// `PollForJobs` documents it: "only jobs whose action configuration matches the
    /// mapped value are returned". A poll that supplied no pair selects every job.
    pub fn matches_query(&self, query: Option<(&str, &str)>) -> bool {
        query.is_none_or(|(name, value)| {
            self.configuration.get(name).map(String::as_str) == Some(value)
        })
    }
}

/// What `JobNotFoundException` states where a call asks the service to CONFIRM or
/// REPORT a job it is not holding — the custom-action acknowledge/details family
/// (`AcknowledgeJob`, `GetJobDetails`): the id the caller asked for bare, and nothing
/// else about the store.
///
/// One source per family, so the operations of a family cannot drift on what a missing
/// job is reported as; the family that REPORTS a result words it with the id quoted
/// ([`job_result_not_found_message`]) and the partner plane closes it with a period
/// ([`third_party_job_not_found_message`]), so the templates are separate rather than
/// shared.
pub fn job_not_found_message(job_id: &str) -> String {
    format!("Job with id {job_id} does not exist")
}

/// What `JobNotFoundException` states where a WORKER reports a result for a job the
/// service is not holding — the put-result family (`PutJobSuccessResult`,
/// `PutJobFailureResult`): the same statement with the id quoted.
pub fn job_result_not_found_message(job_id: &str) -> String {
    format!("Job with id '{job_id}' does not exist")
}

/// What `JobNotFoundException` states on the THIRD-PARTY job plane — the operations a
/// partner's worker calls (`AcknowledgeThirdPartyJob`,
/// `PutThirdPartyJobSuccessResult`, `PutThirdPartyJobFailureResult`): the id quoted,
/// and the statement closed with a period.
///
/// The partner plane states this condition in its own words, which is why it is a
/// third template beside the two the custom-action plane uses: the wording belongs to
/// the operation family that answers, so no family may be worded from another's.
pub fn third_party_job_not_found_message(job_id: &str) -> String {
    format!("Job with id '{job_id}' does not exist.")
}

/// What `InvalidNonceException` states: the job the nonce was presented for.
///
/// The nonce itself is not repeated back — the caller already holds it, and the
/// condition is about the JOB, which either admits no nonce any more (a worker has
/// claimed it) or never published this one. One source, so every operation that
/// refuses a nonce words it the same way.
pub fn invalid_nonce_message(job_id: &str) -> String {
    format!("Invalid nonce for job id {job_id}")
}

/// One artifact of a job, named and placed where its bytes live — or, for an artifact
/// the job is still to produce, where they are to be written.
///
/// `ArtifactLocation` is how `JobData` tells a worker where to read and write: the
/// artifact store is an S3 bucket, so the location is the bucket and object key the
/// execution plane assigned the artifact.
pub fn job_artifact(name: &str, location: &ArtifactRef) -> Artifact {
    Artifact {
        name: Some(name.to_string()),
        revision: location.revision.clone(),
        location: Some(ArtifactLocation {
            r#type: Some(ARTIFACT_LOCATION_TYPE_S3.to_string()),
            s3_location: Some(S3ArtifactLocation {
                bucket_name: location.bucket.clone(),
                object_key: location.key.clone(),
            }),
        }),
    }
}

/// One third-party job waiting for a partner's job worker.
#[derive(Debug, Clone)]
pub struct ThirdPartyJobRecord {
    /// The job's id in CodePipeline, matching `ThirdPartyJob$jobId`'s modelled pattern
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
    pub job_id: String,
    /// The `clientId` a worker is given for the job, matching
    /// `ThirdPartyJob$clientId`'s modelled pattern (the same uuid form). It is the
    /// first half of the `clientId`/`clientToken` pair the model documents: the
    /// worker presents the matching `clientToken` to `GetThirdPartyJobDetails` and
    /// `AcknowledgeThirdPartyJob`.
    pub client_id: String,
    /// The `clientToken` half of that pair: the token a worker's call has to carry
    /// to reach this job, as every third-party operation's `clientToken` member
    /// documents ("used to verify that the calling entity is allowed access to the
    /// job and its details"). `ClientToken` carries no modelled pattern, so this is
    /// an opaque value minted with the job.
    pub client_token: String,
    /// The job's current `Nonce`: "a system-generated random number that CodePipeline
    /// uses to ensure that the job is being worked on by only one job worker"
    /// (`AcknowledgeThirdPartyJob$nonce`), which a worker reads off
    /// `GetThirdPartyJobDetails` and presents to `AcknowledgeThirdPartyJob`. Minted
    /// with the job and well inside `Nonce`'s modelled 1..50 length, so a queued job
    /// always has exactly one value that admits a worker to it.
    pub nonce: String,
    /// A `JobStatus` value: where the job is between being queued and a worker
    /// reporting its outcome.
    pub status: String,
    /// The continuation token the last successful result supplied, identifying a
    /// partner action still in progress. `PutThirdPartyJobSuccessResult$continuationToken`
    /// documents that "future jobs use this token to identify the running instance
    /// of the action", and that "when the action is complete, no continuation token
    /// should be supplied" — so an absent token here means the action finished with
    /// this job.
    pub continuation_token: Option<String>,
    /// The revision the worker reported as current for the artifact it worked on.
    pub current_revision: Option<CurrentRevision>,
    /// The execution details the worker reported with its result.
    pub execution_details: Option<ExecutionDetails>,
    /// The failure the worker reported for this job, when it reported one.
    ///
    /// `PutThirdPartyJobFailureResult$failureDetails` is "information about failure
    /// details" — the `FailureType` the worker chose, its message, and the external
    /// execution id of the run that failed — so it is the outcome of a failed job and
    /// is retained with it, exactly as [`Self::execution_details`] retains what a
    /// successful result reported. `None` means no failure has been reported for this
    /// job.
    pub failure_details: Option<FailureDetails>,
    /// When this job's status last changed.
    pub last_status_change: SystemTime,
}

impl ThirdPartyJobRecord {
    /// A record of the job identified by `job_id`, to be served to the worker holding
    /// `client_id` — what the action-execution plane queues when an execution reaches
    /// a partner action.
    ///
    /// The job starts at [`JOB_STATUS_QUEUED`]: it is waiting for a worker, and no
    /// worker owns its outcome until one acknowledges it. The `clientToken` half of
    /// the pair is minted here, so a queued job always has the one token that
    /// reaches it, and so is the `nonce` that admits one worker to it.
    ///
    /// Both come from [`random_uuid_v4`], whose bits are hash-seed material rather
    /// than a CSPRNG's: they are FIXTURE-GRADE values that make one queued job's
    /// token and nonce distinct from another's, not a secret an attacker is kept out
    /// by. Nothing in this emulator is protected by them — the store is in-memory and
    /// per-process, and a real worker credential is minted by the emulator's
    /// credential store, never here — so the `clientToken`/`nonce` check they feed is
    /// the service's modelled behaviour, not an authorization boundary.
    pub fn new(job_id: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            client_id: client_id.into(),
            client_token: random_uuid_v4(),
            nonce: random_uuid_v4(),
            status: JOB_STATUS_QUEUED.to_string(),
            continuation_token: None,
            current_revision: None,
            execution_details: None,
            failure_details: None,
            last_status_change: SystemTime::now(),
        }
    }

    /// This job as `PollForThirdPartyJobs` reports it.
    pub fn to_job(&self) -> ThirdPartyJob {
        ThirdPartyJob {
            client_id: Some(self.client_id.clone()),
            job_id: Some(self.job_id.clone()),
        }
    }

    /// This job as `GetThirdPartyJobDetails` reports it to the worker that holds it:
    /// the job's id, the `nonce` that admits a worker to it, and what the store holds
    /// about the work itself.
    ///
    /// `action_type` is the identity of the queue the job waits in
    /// ([`ComAmazonawsCodepipelineCodepipeline20150709State::third_party_job`]), which
    /// is what `ThirdPartyJobData$actionTypeId` reports, and the continuation token is
    /// the one the last result reported for the job.
    ///
    /// The remaining `ThirdPartyJobData` members — the action's configuration, the
    /// pipeline context, the artifacts, the artifact store's encryption key and the
    /// temporary credentials for it — are what a DISPATCH captures on the job it
    /// queues, the way a `Custom`-owned action's [`JobRecord`] captures them. This
    /// record carries no such capture, because a partner job is queued for a
    /// registered partner integration and nothing in the assembled emulator creates
    /// one ([`ComAmazonawsCodepipelineCodepipeline20150709State::third_party_jobs`]),
    /// so each is ABSENT here rather than invented per read: a worker is told what the
    /// service holds about its job and nothing it does not.
    pub fn to_details(&self, action_type: &ActionTypeId) -> crate::ops::types::ThirdPartyJobDetails {
        crate::ops::types::ThirdPartyJobDetails {
            id: Some(self.job_id.clone()),
            data: Some(crate::ops::types::ThirdPartyJobData {
                action_type_id: Some(action_type.clone()),
                action_configuration: None,
                pipeline_context: None,
                input_artifacts: None,
                output_artifacts: None,
                artifact_credentials: None,
                continuation_token: self.continuation_token.clone(),
                encryption_key: None,
            }),
            // "A system-generated random number that CodePipeline uses to ensure that
            // the job is being worked on by only one job worker" — the value the
            // worker then presents to `AcknowledgeThirdPartyJob`, read off the job.
            nonce: Some(self.nonce.clone()),
        }
    }

    /// Whether a job worker may still report a result for this job.
    ///
    /// Only a job the worker has acknowledged and not yet completed: a job still
    /// being handed out (`Created`, `Queued`, `Dispatched`) has no worker that owns
    /// its outcome, and one that already reached a terminal status (`Succeeded`,
    /// `Failed`, `TimedOut`) has an outcome that cannot be replaced.
    pub fn accepts_result(&self) -> bool {
        self.status == JOB_STATUS_IN_PROGRESS
    }

    /// Whether this job's work has ended ([`TERMINAL_JOB_STATUSES`]).
    pub fn is_terminal(&self) -> bool {
        TERMINAL_JOB_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `nonce` is the value that admits a worker to this job.
    ///
    /// The job's current nonce, and only while the job is still live: the nonce is
    /// there to "ensure that the job is being worked on by only one job worker", and a
    /// job whose work has ended is being worked on by none, so no nonce admits it.
    pub fn admits_nonce(&self, nonce: &str) -> bool {
        !self.is_terminal() && self.nonce == nonce
    }

    /// Record that a job worker has confirmed receipt of this job, which is what
    /// `AcknowledgeThirdPartyJob` does: the job is now being worked on, so it carries
    /// [`JOB_STATUS_IN_PROGRESS`] and its outcome is that worker's to report
    /// ([`Self::accepts_result`]).
    ///
    /// Idempotent: the worker holding the nonce may repeat the acknowledgement, and a
    /// job already `InProgress` stays exactly where it is — including the instant it
    /// last changed, since nothing about it changed.
    pub fn acknowledge(&mut self, now: SystemTime) {
        if self.status != JOB_STATUS_IN_PROGRESS {
            self.status = JOB_STATUS_IN_PROGRESS.to_string();
            self.last_status_change = now;
        }
    }

    /// Record a worker's successful result: the job reaches
    /// [`JOB_STATUS_SUCCEEDED`] and keeps what the worker reported with it.
    pub fn record_success(
        &mut self,
        continuation_token: Option<String>,
        current_revision: Option<CurrentRevision>,
        execution_details: Option<ExecutionDetails>,
        now: SystemTime,
    ) {
        self.status = JOB_STATUS_SUCCEEDED.to_string();
        self.continuation_token = continuation_token;
        self.current_revision = current_revision;
        self.execution_details = execution_details;
        self.last_status_change = now;
    }

    /// Record a worker's failure for this job: the job reaches [`JOB_STATUS_FAILED`]
    /// and keeps the `failureDetails` the worker reported with it.
    ///
    /// The job's work has ended, so the status is terminal ([`Self::is_terminal`]) and
    /// no later report for the same job is accepted ([`Self::accepts_result`]): the
    /// outcome recorded here is the one a reader answers from.
    pub fn record_failure(&mut self, failure: FailureDetails, now: SystemTime) {
        self.status = JOB_STATUS_FAILED.to_string();
        self.failure_details = Some(failure);
        self.last_status_change = now;
    }
}
