// SPDX-License-Identifier: CC0-1.0

//! The batch-evaluation plane's state: the batch evaluation jobs this service holds.
//!
//! A batch evaluation job is MINTED by `StartBatchEvaluation` — the call that starts it
//! chooses the data source its agent session traces are pulled from, the evaluators and
//! insights to run and where the results are written, and this emulator mints the
//! `batchEvaluationId` and the ARN it is addressed by ([`mint_batch_evaluation_id`],
//! [`batch_evaluation_arn`]). `GetBatchEvaluation` reports one of them back, so a
//! `batchEvaluationId` this plane holds no job for is refused with the shared
//! not-found sentence ([`super::resource_not_found_message`]).
//!
//! A job's `status` is read off the CLOCK rather than stored:
//! `StartBatchEvaluation` answers 202 Accepted, so a job that was just started is still
//! `PENDING`, is `IN_PROGRESS` once [`BATCH_EVALUATION_TRANSITION_SPAN`] has passed and
//! has run to its end one span after that, with no call needed to make it so
//! ([`BatchEvaluationRecord::status_as_of`]). Nothing is slept for and nothing is
//! written by a read: the instants a transition falls due follow from the instant the
//! job was started.
//!
//! `StopBatchEvaluation` and `DeleteBatchEvaluation` are the two calls that end a job, and
//! both are decided by the status it is in when they arrive. A stop is accepted only while
//! there is still work to end ([`BATCH_EVALUATION_STOPPABLE_STATUSES`]) — a stop of a job
//! with no work left is refused with this plane's fixed conflict sentence
//! ([`BATCH_EVALUATION_NOT_STOPPABLE_MESSAGE`]) — and from then on
//! the job is on the STOP's clock — `STOPPING`, then `STOPPED` one span later. A delete is
//! accepted only once the job has finished ([`BATCH_EVALUATION_TERMINAL_STATUSES`]), and
//! it removes the record, so the id it addressed afterwards resolves to nothing at all.
//!
//! The plane has an explicit ceiling: `StartBatchEvaluation` is its growth site and a
//! caller chooses both the names it starts jobs under and the `clientToken` each insert
//! is recorded with, so it is bounded by [`MAX_BATCH_EVALUATIONS`] and
//! [`MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS`], each evicting its least recently
//! started record once that maximum is passed.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BatchEvaluationSummary, DataSourceConfig, DatasetExecutionDetails, EvaluationMetadata,
    Evaluator, Insight, OutputConfig,
};

use super::{
    mint_named_resource_id, resource_not_found_message, NotFoundResource, ARN_SERVICE,
    ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
};

/// The most batch evaluation jobs this emulator retains; the least recently started one
/// is evicted once the store passes it.
///
/// An EMULATOR ceiling, not an AWS quota: every `StartBatchEvaluation` appends a record,
/// so without one a caller looping on the start would grow the process heap without end.
pub const MAX_BATCH_EVALUATIONS: usize = 256;

/// The most `StartBatchEvaluation` `clientToken` records this emulator retains.
pub const MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS: usize = 1_000;

/// How long each step of a batch evaluation job's lifecycle takes: `PENDING` until one
/// span has passed, `IN_PROGRESS` for the span after that.
///
/// `StartBatchEvaluation` answers 202 Accepted — the job is accepted for execution
/// rather than finished — and AWS publishes no duration for how long a batch evaluation
/// takes (it depends on how many sessions the data source yields), so this span is the
/// EMULATOR's own: long enough that a caller which starts a job and reads it back at once
/// still observes the transient status, short enough not to hold a test suite up.
pub const BATCH_EVALUATION_TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// `BatchEvaluationStatus` for a job that has been accepted and has not started running
/// yet (source: smithy_query query="shape" shape="BatchEvaluationStatus", values PENDING
/// | IN_PROGRESS | COMPLETED | COMPLETED_WITH_ERRORS | FAILED | STOPPING | STOPPED |
/// DELETING).
pub const BATCH_EVALUATION_STATUS_PENDING: &str = "PENDING";

/// `BatchEvaluationStatus` for a job that is running, from the same enum.
pub const BATCH_EVALUATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `BatchEvaluationStatus` for a job that has run to its end, from the same enum.
pub const BATCH_EVALUATION_STATUS_COMPLETED: &str = "COMPLETED";

/// `BatchEvaluationStatus` for a job that ran to its end but could not evaluate every
/// session it was given, from the same enum.
pub const BATCH_EVALUATION_STATUS_COMPLETED_WITH_ERRORS: &str = "COMPLETED_WITH_ERRORS";

/// `BatchEvaluationStatus` for a job that gave up, from the same enum.
pub const BATCH_EVALUATION_STATUS_FAILED: &str = "FAILED";

/// `BatchEvaluationStatus` for a job a stop has been asked for but has not taken effect
/// yet, from the same enum.
pub const BATCH_EVALUATION_STATUS_STOPPING: &str = "STOPPING";

/// `BatchEvaluationStatus` for a job a stop ended, from the same enum.
pub const BATCH_EVALUATION_STATUS_STOPPED: &str = "STOPPED";

/// `BatchEvaluationStatus` for a job a delete has been accepted for, from the same enum —
/// the status the delete reports back for the deletion it accepted.
pub const BATCH_EVALUATION_STATUS_DELETING: &str = "DELETING";

/// The statuses a batch evaluation job has FINISHED in: a job in one of them is running
/// no longer, so there is no work left for a delete to interrupt. Every other value of
/// `BatchEvaluationStatus` names a job that is still on its way somewhere (`PENDING`,
/// `IN_PROGRESS`, `STOPPING`, `DELETING`).
///
/// The order is the order [`batch_evaluation_not_deletable_message`] lists them in.
pub const BATCH_EVALUATION_TERMINAL_STATUSES: [&str; 4] = [
    BATCH_EVALUATION_STATUS_COMPLETED,
    BATCH_EVALUATION_STATUS_COMPLETED_WITH_ERRORS,
    BATCH_EVALUATION_STATUS_FAILED,
    BATCH_EVALUATION_STATUS_STOPPED,
];

/// Whether a job reported under `status` has finished
/// ([`BATCH_EVALUATION_TERMINAL_STATUSES`]) — which is what decides whether a delete of
/// it can be accepted.
#[must_use]
pub fn batch_evaluation_status_is_terminal(status: &str) -> bool {
    BATCH_EVALUATION_TERMINAL_STATUSES.contains(&status)
}

/// The message a delete of a batch evaluation job that has not finished is refused with.
///
/// The sentence names the status the job is in at the instant of the delete — its only
/// variable part — and then the statuses a job can be deleted in
/// ([`BATCH_EVALUATION_TERMINAL_STATUSES`]), so a caller is told both what blocks the
/// delete and what to wait for.
#[must_use]
pub fn batch_evaluation_not_deletable_message(status: &str) -> String {
    let [first, second, third, last] = BATCH_EVALUATION_TERMINAL_STATUSES;
    format!(
        "Cannot delete batch evaluation in status {status}. \
         Only {first}, {second}, {third}, or {last} evaluations can be deleted."
    )
}

/// The statuses a batch evaluation job can be STOPPED in: a stop ends work that is still
/// owed, so there has to be some. Every other value of `BatchEvaluationStatus` names a
/// job whose run is already over or already ending.
pub const BATCH_EVALUATION_STOPPABLE_STATUSES: [&str; 2] = [
    BATCH_EVALUATION_STATUS_PENDING,
    BATCH_EVALUATION_STATUS_IN_PROGRESS,
];

/// Whether a job reported under `status` still has work a stop could end
/// ([`BATCH_EVALUATION_STOPPABLE_STATUSES`]) — which is what decides whether a stop of it
/// can be accepted.
#[must_use]
pub fn batch_evaluation_status_is_stoppable(status: &str) -> bool {
    BATCH_EVALUATION_STOPPABLE_STATUSES.contains(&status)
}

/// The whole `ConflictException` message a stop of a batch evaluation job that has no work
/// left to end is refused with — FIXED text, exactly as this plane's not-found sentence
/// ([`BATCH_EVALUATION_NOT_FOUND_MESSAGE`]) is.
///
/// The batch-evaluation API words a conflicting change after the resource the request
/// failed to change and the retry that would succeed, so nothing of the request or of the
/// store appears in it: not the `batchEvaluationId` the call spelled, not the status the
/// job is in and not the statuses a stop is still possible in
/// ([`BATCH_EVALUATION_STOPPABLE_STATUSES`], which decides the refusal but is not reported
/// by it).
pub const BATCH_EVALUATION_NOT_STOPPABLE_MESSAGE: &str =
    "Failed to update the resource as a concurrent request modified the resource. \
     Retry the request after the other request has completed.";

/// The message a lookup of a batch evaluation job that this plane holds none for is
/// refused with — the whole `ResourceNotFoundException` message, which is FIXED text: the
/// batch-evaluation API words this refusal after the resource the request failed to reach
/// and the retry that would succeed, so neither the `batchEvaluationId` the request
/// spelled nor the resource type appears in it.
///
/// One sentence for every caller that misses — the read, the listing's own lookups and the
/// delete, including a delete or a read of an id a delete has already removed — so the
/// "gone" answer cannot be worded two ways.
pub const BATCH_EVALUATION_NOT_FOUND_MESSAGE: &str =
    "Failed to retrieve the resource because it doesn't exist. \
     Retry the request with a different resource identifier.";

/// One batch evaluation job: what `StartBatchEvaluation` persisted, which is what every
/// later read of the job reports.
#[derive(Debug, Clone)]
pub struct BatchEvaluationRecord {
    /// Minted by [`mint_batch_evaluation_id`]; never client-supplied.
    pub batch_evaluation_id: String,
    /// Minted by [`batch_evaluation_arn`] for the region the starting request was signed
    /// for.
    pub batch_evaluation_arn: String,
    pub name: String,
    pub description: Option<String>,
    /// Where the agent session traces to evaluate are pulled from.
    pub data_source_config: DataSourceConfig,
    pub evaluators: Option<Vec<Evaluator>>,
    pub insights: Option<Vec<Insight>>,
    /// The per-session ground truth and test-scenario identifiers the starting request
    /// supplied, kept with the job so what it was started with is what is held.
    pub evaluation_metadata: Option<EvaluationMetadata>,
    pub output_config: Option<OutputConfig>,
    /// The dataset run this job carries out before it evaluates anything, as the start
    /// RESOLVED it ([`super::resolve_dataset_execution`]): the dataset version its
    /// scenarios come from and the agent runtime version they are sent to, settled at the
    /// instant the job started. `None` for a job whose start asked for no dataset run.
    pub dataset_execution_details: Option<DatasetExecutionDetails>,
    pub tags: Option<HashMap<String, String>>,
    pub kms_key_arn: Option<String>,
    pub created_at: SystemTime,
    /// The instant a `StopBatchEvaluation` asked for this job to stop, or `None` while
    /// none has: the stop is what takes the job off the run's own clock
    /// ([`Self::status_as_of`]).
    pub stop_requested_at: Option<SystemTime>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this job: its eviction order, and what keeps two jobs started in the same
    /// millisecond apart.
    pub seq: u64,
}

impl BatchEvaluationRecord {
    /// A job as the call that starts it leaves it: the configuration that call asked for,
    /// started at `created_at`.
    #[allow(clippy::too_many_arguments)]
    pub fn started(
        seq: u64,
        region: &str,
        name: String,
        description: Option<String>,
        data_source_config: DataSourceConfig,
        evaluators: Option<Vec<Evaluator>>,
        insights: Option<Vec<Insight>>,
        evaluation_metadata: Option<EvaluationMetadata>,
        output_config: Option<OutputConfig>,
        dataset_execution_details: Option<DatasetExecutionDetails>,
        tags: Option<HashMap<String, String>>,
        kms_key_arn: Option<String>,
        created_at: SystemTime,
    ) -> Self {
        let batch_evaluation_id = mint_batch_evaluation_id(&name, created_at, seq);
        Self {
            batch_evaluation_arn: batch_evaluation_arn(region, &batch_evaluation_id),
            batch_evaluation_id,
            name,
            description,
            data_source_config,
            evaluators,
            insights,
            evaluation_metadata,
            output_config,
            dataset_execution_details,
            tags,
            kms_key_arn,
            created_at,
            stop_requested_at: None,
            seq,
        }
    }

    /// The instant this job starts running, or `None` when that instant is not
    /// representable (so nothing can be past it).
    pub fn running_at(&self) -> Option<SystemTime> {
        self.created_at.checked_add(BATCH_EVALUATION_TRANSITION_SPAN)
    }

    /// The instant this job has run to its end, read the same way as
    /// [`Self::running_at`]: one more span after it started running.
    pub fn finished_at(&self) -> Option<SystemTime> {
        self.running_at()
            .and_then(|running_at| running_at.checked_add(BATCH_EVALUATION_TRANSITION_SPAN))
    }

    /// The instant this job has stopped, read the same way as [`Self::running_at`]: one
    /// span after a stop was asked for, or `None` while none has been.
    pub fn stopped_at(&self) -> Option<SystemTime> {
        self.stop_requested_at
            .and_then(|requested_at| requested_at.checked_add(BATCH_EVALUATION_TRANSITION_SPAN))
    }

    /// This job's `BatchEvaluationStatus` at `now`: `PENDING` until it starts running,
    /// `IN_PROGRESS` while it is, and `COMPLETED` once it has run to its end. Read off
    /// the stored start instant and the clock, so a report of the job states the
    /// transition that was already due without writing anything.
    ///
    /// A job a stop was asked for is on the STOP's clock instead, from the instant that
    /// call was served: `STOPPING` until one span has passed and `STOPPED` after it. The
    /// run it was carrying out is over either way, so the instants the original run would
    /// have fallen due at no longer say anything about the job.
    pub fn status_as_of(&self, now: SystemTime) -> &'static str {
        if self.stop_requested_at.is_some() {
            return match self.stopped_at() {
                Some(stopped_at) if now >= stopped_at => BATCH_EVALUATION_STATUS_STOPPED,
                _ => BATCH_EVALUATION_STATUS_STOPPING,
            };
        }
        match (self.running_at(), self.finished_at()) {
            (_, Some(finished_at)) if now >= finished_at => BATCH_EVALUATION_STATUS_COMPLETED,
            (Some(running_at), _) if now >= running_at => BATCH_EVALUATION_STATUS_IN_PROGRESS,
            _ => BATCH_EVALUATION_STATUS_PENDING,
        }
    }

    /// Record that a stop was asked for at `now`, which is what moves this job onto the
    /// stop's clock ([`Self::status_as_of`]).
    pub fn stop(&mut self, now: SystemTime) {
        self.stop_requested_at = Some(now);
    }

    /// This job as the modelled `BatchEvaluationSummary`, as of `now`.
    ///
    /// The summary carries the same identity, configuration and clock-derived lifecycle a
    /// full report of the job does, so a listing and a read of one job cannot disagree;
    /// `evaluationResults` and `errorDetails` are left out for the same reason the full
    /// report leaves them out — no judge model runs here, so nothing has scored the job
    /// and nothing has failed.
    pub fn summary(&self, now: SystemTime) -> BatchEvaluationSummary {
        BatchEvaluationSummary {
            batch_evaluation_id: self.batch_evaluation_id.clone(),
            batch_evaluation_arn: self.batch_evaluation_arn.clone(),
            batch_evaluation_name: self.name.clone(),
            status: self.status_as_of(now).to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            description: self.description.clone(),
            evaluators: self.evaluators.clone(),
            insights: self.insights.clone(),
            evaluation_results: None,
            error_details: None,
            kms_key_arn: self.kms_key_arn.clone(),
            updated_at: Some(::aws_smithy_types::DateTime::from(
                self.updated_at_as_of(now),
            )),
        }
    }

    /// The total order batch evaluation jobs are reported in: newest first, with the
    /// start sequence breaking a tie between two jobs started in the same millisecond, so
    /// one state always paginates the same way.
    pub fn order_key(&self) -> (SystemTime, u64) {
        (self.created_at, self.seq)
    }

    /// When this job last changed at `now`: the instant of the most recent lifecycle
    /// transition the clock has made due, which is its start instant while it is still
    /// `PENDING`.
    ///
    /// A job a stop was asked for reports the instant of that call while it is `STOPPING`
    /// and the instant it stopped once it has, for the same reason
    /// [`Self::status_as_of`] reads the stop's clock rather than the run's.
    pub fn updated_at_as_of(&self, now: SystemTime) -> SystemTime {
        if let Some(stop_requested_at) = self.stop_requested_at {
            return match self.stopped_at() {
                Some(stopped_at) if now >= stopped_at => stopped_at,
                _ => stop_requested_at,
            };
        }
        match (self.running_at(), self.finished_at()) {
            (_, Some(finished_at)) if now >= finished_at => finished_at,
            (Some(running_at), _) if now >= running_at => running_at,
            _ => self.created_at,
        }
    }
}

/// Mint a `batchEvaluationId` for a job of `name` started at `created_at`.
///
/// `BatchEvaluationId` publishes the pattern
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, so the id is the name the request
/// asked for followed by the ten-character discriminator
/// [`mint_named_resource_id`] builds — the shared form for every id of this service that
/// publishes that pattern.
pub fn mint_batch_evaluation_id(name: &str, created_at: SystemTime, seq: u64) -> String {
    mint_named_resource_id(name, created_at, seq)
}

/// The ARN a batch evaluation job is addressed by in `region`.
///
/// `BatchEvaluationArn` publishes no pattern of its own, only the ARN resource type it
/// references (`AWS::BedrockAgentCore::BatchEvaluate`, source: smithy_query
/// query="shape" shape="BatchEvaluationArn"); this service's ARNs spell a resource
/// segment as that type lowercased and hyphenated, which is also the path segment the
/// operation is bound to (`/evaluations/batch-evaluate`).
pub fn batch_evaluation_arn(region: &str, batch_evaluation_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("batch-evaluate/{batch_evaluation_id}"),
    )
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The batch evaluation job this `StartBatchEvaluation` `clientToken` already
    /// started, or `None` when it has started none this store still holds.
    pub fn batch_evaluation_for_token(&self, client_token: &str) -> Option<&BatchEvaluationRecord> {
        self.batch_evaluation_idempotency
            .get(client_token)
            .and_then(|batch_evaluation_id| self.batch_evaluations.get(batch_evaluation_id))
    }

    /// Store a started batch evaluation job, recording `client_token` as the token that
    /// started it, and hold every bound this plane declares.
    pub fn insert_batch_evaluation(
        &mut self,
        record: BatchEvaluationRecord,
        client_token: Option<String>,
    ) {
        if let Some(token) = client_token {
            self.batch_evaluation_idempotency
                .insert(token, record.batch_evaluation_id.clone());
        }
        self.batch_evaluations
            .insert(record.batch_evaluation_id.clone(), record);

        while self.batch_evaluations.len() > MAX_BATCH_EVALUATIONS {
            // The least recently started job gives way first; the jobs a live workflow
            // just started are the ones that stay readable.
            let Some(oldest) = self
                .batch_evaluations
                .values()
                .min_by_key(|held| held.seq)
                .map(|held| held.batch_evaluation_id.clone())
            else {
                break;
            };
            self.batch_evaluations.remove(&oldest);
            // The tokens that pointed at it go with it: a token whose job is gone would
            // otherwise let a replayed start mint a SECOND job instead of answering with
            // the one it already started.
            self.batch_evaluation_idempotency
                .retain(|_, held| held != &oldest);
        }
        while self.batch_evaluation_idempotency.len() > MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS {
            // A token whose job is already gone is of no use to a replay, so it gives way
            // before one that can still be answered.
            let Some(oldest) = self
                .batch_evaluation_idempotency
                .iter()
                .min_by_key(|(token, batch_evaluation_id)| {
                    (
                        self.batch_evaluations
                            .get(*batch_evaluation_id)
                            .map_or(0, |held| held.seq),
                        (*token).clone(),
                    )
                })
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.batch_evaluation_idempotency.remove(&oldest);
        }
    }

    /// Every batch evaluation job this store holds, newest first
    /// ([`BatchEvaluationRecord::order_key`]) — the one order every page of a listing is
    /// cut from, so following a token reports each held job exactly once.
    pub fn batch_evaluations_newest_first(&self) -> Vec<&BatchEvaluationRecord> {
        let mut jobs: Vec<&BatchEvaluationRecord> = self.batch_evaluations.values().collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.order_key()));
        jobs
    }

    /// The batch evaluation job a read addressed, or the sentence naming it as the
    /// missing resource — the SHARED not-found wording this service words every missed
    /// lookup with.
    pub fn resolve_batch_evaluation(
        &self,
        batch_evaluation_id: &str,
    ) -> Result<&BatchEvaluationRecord, String> {
        self.batch_evaluations
            .get(batch_evaluation_id)
            .ok_or_else(|| {
                resource_not_found_message(
                    NotFoundResource::BatchEvaluation,
                    batch_evaluation_id,
                )
            })
    }

    /// The same lookup taken mutably, for the stop that changes what it found.
    pub fn resolve_batch_evaluation_mut(
        &mut self,
        batch_evaluation_id: &str,
    ) -> Result<&mut BatchEvaluationRecord, String> {
        self.batch_evaluations
            .get_mut(batch_evaluation_id)
            .ok_or_else(|| {
                resource_not_found_message(
                    NotFoundResource::BatchEvaluation,
                    batch_evaluation_id,
                )
            })
    }

    /// Remove the batch evaluation job a delete addressed and hand it back, or the
    /// sentence this plane refuses a missed lookup with.
    ///
    /// The `clientToken` records that point at it go with it, exactly as they do when the
    /// plane's ceiling evicts a job: a token whose job is gone would otherwise let a
    /// replayed start answer with a job no caller can read.
    pub fn remove_batch_evaluation(
        &mut self,
        batch_evaluation_id: &str,
    ) -> Result<BatchEvaluationRecord, String> {
        let removed = self
            .batch_evaluations
            .remove(batch_evaluation_id)
            .ok_or_else(|| {
                resource_not_found_message(
                    NotFoundResource::BatchEvaluation,
                    batch_evaluation_id,
                )
            })?;
        self.batch_evaluation_idempotency
            .retain(|_, held| held != &removed.batch_evaluation_id);
        Ok(removed)
    }
}
