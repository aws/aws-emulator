// SPDX-License-Identifier: CC0-1.0

//! The notebook-execution lifecycle, one concern of this service's state.
//!
//! `StartNotebookExecution` records a [`NotebookExecutionRecord`] here and the clock walks
//! it through the statuses `NotebookExecution$Status` names
//! ([`NOTEBOOK_EXECUTION_PROGRESSION`] and the end its own run answered,
//! [`NotebookOutcome`]; [`NOTEBOOK_EXECUTION_STOP_PROGRESSION`] once a caller has asked it
//! to stop);
//! `DescribeNotebookExecution` reports that record and `ListNotebookExecutions` a
//! PROJECTION of it ([`NotebookExecutionRecord::to_summary`]). Its names are re-exported by
//! the state root, so every caller keeps reaching them through `crate::state`.
//!
//! What the notebook itself CAME TO is the notebook runner's concern
//! ([`super::notebook_run`]): this module owns the statuses and their spans, and the end of
//! the walk is whatever that run answered ([`NotebookOutcome`]).

use super::*;

/// The ARN of a notebook execution:
/// `arn:aws:elasticmapreduce:<region>:<account>:notebook-execution/<id>`.
pub fn notebook_execution_arn(region: &str, execution_id: &str) -> String {
    ::engine::emulator_arn(
        "elasticmapreduce",
        region,
        &format!("notebook-execution/{execution_id}"),
    )
}

/// `ListNotebookExecutions` page size: "Returns a maximum of 50 notebook executions and
/// a marker to track the paging of a longer notebook execution list across multiple
/// `ListNotebookExecutions` calls." (`ListNotebookExecutions` doc, Smithy model.)
pub const LIST_NOTEBOOK_EXECUTIONS_PAGE_SIZE: usize = 50;

/// How far back `ListNotebookExecutions` looks when the request names no `From`: "The
/// beginning of time range filter for listing notebook executions. The default is the
/// timestamp of 30 days ago." (`ListNotebookExecutionsInput$From` doc, Smithy model.)
pub const NOTEBOOK_EXECUTION_DEFAULT_FROM: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The statuses a notebook execution passes through, in order, from the moment the cluster
/// has received the request until its notebook's own run is under way.
///
/// Straight off `NotebookExecution$Status`: "`START_PENDING` indicates that the cluster has
/// received the execution request but execution has not begun", "`STARTING` indicates that
/// the execution is starting on the cluster" and "`RUNNING` indicates that the execution is
/// being processed by the cluster". Each move is owed [`notebook_phase_span`] after the one
/// before it.
///
/// What follows `RUNNING` is not fixed here: it is the end the execution's OWN run answered
/// ([`NotebookOutcome::end_progression`]) — the finishing statuses for a notebook whose code
/// cells all ran, the failing ones for a notebook that raised and for one nothing ran.
pub const NOTEBOOK_EXECUTION_PROGRESSION: &[&str] = &[
    notebook_execution_status::START_PENDING,
    notebook_execution_status::STARTING,
    notebook_execution_status::RUNNING,
];

/// How much longer an execution stays `RUNNING` — "the execution is being processed by the
/// cluster" (`NotebookExecution$Status` doc) — ONCE its own run has answered.
///
/// The clock does not time the run itself: an execution leaves `RUNNING` when its notebook's
/// run reports what it came to ([`NotebookExecutionRecord::record_outcome`]), so the phase
/// lasts as long as the notebook really runs for. This is what is added on top of that, so
/// `RUNNING` is observable — and a stop still reaches a running execution — even for a
/// notebook whose cells answered before the walk had got there. It is one phase of the walk
/// like the moves around it, which is why it is one [`NOTEBOOK_PHASE_SPAN`]. Stored as a due
/// instant — nothing sleeps.
pub const NOTEBOOK_RUN_SPAN: Duration = NOTEBOOK_PHASE_SPAN;

/// How long the move OFF `status` takes: leaving `RUNNING` costs [`NOTEBOOK_RUN_SPAN`],
/// every other move one [`NOTEBOOK_PHASE_SPAN`].
///
/// The one reading of a notebook execution's spans, so the walk and the stop progression
/// cannot come to time themselves differently.
pub fn notebook_phase_span(status: &str) -> Duration {
    if status == notebook_execution_status::RUNNING {
        return NOTEBOOK_RUN_SPAN;
    }
    NOTEBOOK_PHASE_SPAN
}

/// The statuses an execution whose notebook ran without raising ends through: "`FINISHING`
/// indicates that execution processing is in the final stages" and "`FINISHED` indicates
/// that the execution has completed without error" (`NotebookExecution$Status` doc).
pub const NOTEBOOK_EXECUTION_FINISH_PROGRESSION: &[&str] = &[
    notebook_execution_status::FINISHING,
    notebook_execution_status::FINISHED,
];

/// The statuses an execution that FAILED ends through: "`FAILING` indicates that the
/// execution is failing and will not finish successfully" and "`FAILED` indicates that the
/// execution failed" (`NotebookExecution$Status` doc).
///
/// Two kinds of execution end here, and `LastStateChangeReason` is what tells them apart
/// ([`NotebookOutcome::failure_reason`]): one whose notebook RAN and raised, which carries
/// the engine's own exception, and one nothing ran at all, which carries
/// [`NOTEBOOK_NOT_EXECUTED_REASON`].
pub const NOTEBOOK_EXECUTION_FAILURE_PROGRESSION: &[&str] = &[
    notebook_execution_status::FAILING,
    notebook_execution_status::FAILED,
];

/// What a notebook execution's own run came to, which decides the end of its status walk.
///
/// The run itself is [`super::run_notebook`]: it fetches the notebook the execution names
/// out of Amazon S3 and executes its code cells in the contained interpreter this emulator
/// runs caller-supplied code in. Nothing here reports an outcome for work nothing performed:
/// an execution whose notebook could not be fetched, read or run is [`Self::NotRun`] and
/// says so in its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotebookOutcome {
    /// No notebook was run: the execution names no Amazon S3 notebook, Amazon S3 holds no
    /// such object, the object is no readable notebook, or its kernel is not a kind this
    /// emulator can run.
    NotRun,
    /// Every code cell of the notebook ran without raising, and this is how many there
    /// were.
    Finished {
        /// How many code cells ran.
        cells: usize,
    },
    /// The run ended in a failure, with the reason the engine gave for it.
    Failed {
        /// `NotebookExecution$LastStateChangeReason` for the failure, as the run reported
        /// it.
        reason: String,
    },
}

impl NotebookOutcome {
    /// The statuses an execution that reached `RUNNING` ends through, for this outcome.
    pub fn end_progression(&self) -> &'static [&'static str] {
        match self {
            Self::Finished { .. } => NOTEBOOK_EXECUTION_FINISH_PROGRESSION,
            Self::NotRun | Self::Failed { .. } => NOTEBOOK_EXECUTION_FAILURE_PROGRESSION,
        }
    }

    /// `NotebookExecution$LastStateChangeReason` for the failing statuses of this outcome:
    /// the engine's own reason for a run that failed, and the sentence that says no notebook
    /// was run for an execution nothing ran.
    pub fn failure_reason(&self) -> Option<String> {
        match self {
            Self::Finished { .. } => None,
            Self::NotRun => Some(NOTEBOOK_NOT_EXECUTED_REASON.to_string()),
            Self::Failed { reason } => Some(reason.clone()),
        }
    }
}

/// `NotebookExecution$LastStateChangeReason` for an execution whose notebook this service
/// did not run — the reason that tells a caller the failure is this emulator's own and not
/// the notebook's.
///
/// An execution reaches it when there is no notebook to run: the request names no Amazon S3
/// notebook (a notebook under an editor is kept in storage this emulator does not hold),
/// Amazon S3 hands over no such object, or the object is no notebook whose cells this
/// emulator can run. `FINISHED` ("the execution has completed without error") would be a
/// success for work nothing performed, which is why this stands in `LastStateChangeReason`
/// instead and says so.
pub const NOTEBOOK_NOT_EXECUTED_REASON: &str =
    "This emulator records notebook executions but runs no notebook, so the execution \
     ended without running.";

/// The statuses a notebook execution passes through once a caller has asked it to stop:
/// "`STOP_PENDING` indicates that the cluster has received a `StopNotebookExecution`
/// request and the stop is pending", "`STOPPING` indicates that the cluster is in the
/// process of stopping the execution" and "`STOPPED` indicates that the execution stopped
/// because of a `StopNotebookExecution` request" (`NotebookExecution$Status` doc).
pub const NOTEBOOK_EXECUTION_STOP_PROGRESSION: &[&str] = &[
    notebook_execution_status::STOP_PENDING,
    notebook_execution_status::STOPPING,
    notebook_execution_status::STOPPED,
];

/// The status that follows `status` for an execution whose own run came to `outcome`, or
/// `None` for one that ends its progression and for one no progression names.
///
/// The ONE reading of what a notebook execution's clock owes it next, so the walk to
/// `RUNNING`, the end the run answered and the stop progression cannot drift apart from the
/// statuses above.
/// An execution whose own run has NOT answered yet owes nothing past `RUNNING`: it is "being
/// processed by the cluster" until that run reports what it came to, which is why `outcome`
/// is optional here.
pub fn next_notebook_execution_status(
    status: &str,
    outcome: Option<&NotebookOutcome>,
) -> Option<&'static str> {
    let end: &'static [&'static str] = outcome.map(NotebookOutcome::end_progression).unwrap_or(&[]);
    for progression in [
        NOTEBOOK_EXECUTION_PROGRESSION,
        end,
        NOTEBOOK_EXECUTION_STOP_PROGRESSION,
    ] {
        if let Some(position) = progression.iter().position(|step| *step == status) {
            return progression
                .get(position + 1)
                .copied()
                // The walk to `RUNNING` ends where the END the run answered begins: a
                // notebook that ran finishes, one that failed or that nothing ran fails —
                // and a run that has not answered yet ends nothing.
                .or_else(|| {
                    (progression == NOTEBOOK_EXECUTION_PROGRESSION)
                        .then(|| end.first().copied())
                        .flatten()
                });
        }
    }
    None
}

/// `NotebookExecution$LastStateChangeReason` for an execution a caller stopped.
pub const NOTEBOOK_STOPPED_REASON: &str = "Stopped by a StopNotebookExecution request";

/// The status a notebook execution's clock owes it, and when that comes due.
#[derive(Debug, Clone)]
pub struct PendingNotebookExecutionStatus {
    /// The instant the status below was owed, stored rather than slept for.
    pub due: SystemTime,
    pub status: String,
    /// The reason the move records, for a move that has one to give.
    pub reason: Option<String>,
}

/// A persisted notebook execution: the wire `NotebookExecution` that
/// `DescribeNotebookExecution` reports, plus the status the clock still owes it.
///
/// `ListNotebookExecutions` reports a PROJECTION of this record
/// ([`NotebookExecutionRecord::to_summary`]); no handler builds that summary from its
/// own input.
#[derive(Debug, Clone)]
pub struct NotebookExecutionRecord {
    pub execution: NotebookExecution,
    pub pending_status: Option<PendingNotebookExecutionStatus>,
    /// What this execution's own run of the notebook came to, which is the END of its
    /// status walk ([`NotebookOutcome`]) — and `None` while that run is still under way,
    /// which is what holds the execution at `RUNNING`.
    pub outcome: Option<NotebookOutcome>,
}

impl NotebookExecutionRecord {
    /// The execution `StartNotebookExecution` records for `input`, under the id and ARN
    /// EMR minted for it.
    ///
    /// The notebook's own run has not answered yet — the cluster has only RECEIVED the
    /// request — so the record carries no outcome until that run reports one
    /// ([`Self::record_outcome`]).
    pub fn from_request(
        id: String,
        arn: String,
        input: StartNotebookExecutionInput,
        now: SystemTime,
    ) -> Self {
        let mut record = Self {
            execution: NotebookExecution {
                notebook_execution_id: Some(id),
                editor_id: input.editor_id,
                execution_engine: Some(input.execution_engine),
                notebook_execution_name: input.notebook_execution_name,
                notebook_params: input.notebook_params,
                // "START_PENDING indicates that the cluster has received the execution
                // request but execution has not begun." (`NotebookExecution$Status`
                // doc.) That is exactly where a just-recorded execution stands.
                status: Some(notebook_execution_status::START_PENDING.to_string()),
                start_time: Some(timestamp(now)),
                end_time: None,
                arn: Some(arn),
                // "The location of the notebook execution's output file in Amazon S3"
                // (`NotebookExecution$OutputNotebookURI` doc): no cell of the notebook has
                // run yet, so there is no output file to point at.
                output_notebook_uri: None,
                last_state_change_reason: None,
                notebook_instance_security_group_id: input.notebook_instance_security_group_id,
                tags: input.tags,
                notebook_s3_location: input.notebook_s3_location.map(|location| {
                    NotebookS3LocationForOutput {
                        bucket: location.bucket,
                        key: location.key,
                    }
                }),
                output_notebook_s3_location: input.output_notebook_s3_location.map(|location| {
                    OutputNotebookS3LocationForOutput {
                        bucket: location.bucket,
                        key: location.key,
                    }
                }),
                output_notebook_format: input.output_notebook_format,
                environment_variables: input.environment_variables,
            },
            // The execution request has been received; the clock owes it the next status of
            // the progression ([`NOTEBOOK_EXECUTION_PROGRESSION`]) and then each one after
            // that, up to the terminal one. A progression that names nothing after
            // `START_PENDING` owes NOTHING — the execution then simply stays where it is,
            // rather than being defaulted into a status nothing took it to.
            pending_status: None,
            outcome: None,
        };
        record.pending_status = next_notebook_execution_status(
            notebook_execution_status::START_PENDING,
            record.outcome.as_ref(),
        )
        .map(|next| PendingNotebookExecutionStatus {
            due: now + notebook_phase_span(notebook_execution_status::START_PENDING),
            status: next.to_string(),
            reason: record.phase_reason(next),
        });
        record
    }

    /// Record what this execution's OWN run of its notebook came to, and owe the execution
    /// the end that run answered.
    ///
    /// The run happens off the `StartNotebookExecution` request path, so this is how its
    /// answer reaches the record: an execution already at `RUNNING` — "being processed by
    /// the cluster", which is where one waits for its run — is owed the first status of that
    /// end one phase later, and one the clock has not walked to `RUNNING` yet picks the same
    /// end up when it gets there ([`Self::advance_to`]). An execution a caller stopped, or
    /// one that has already ended, keeps the status it is walking to: the stop abandoned
    /// whatever the run still owed.
    pub fn record_outcome(&mut self, outcome: NotebookOutcome, now: SystemTime) {
        if self.outcome.is_some() {
            return;
        }
        self.outcome = Some(outcome);
        let status = self.execution.status.clone().unwrap_or_default();
        if notebook_execution_status::TERMINAL.contains(&status.as_str())
            || NOTEBOOK_EXECUTION_STOP_PROGRESSION.contains(&status.as_str())
            || self.pending_status.is_some()
        {
            return;
        }
        if let Some(next) = next_notebook_execution_status(&status, self.outcome.as_ref()) {
            let reason = self.phase_reason(next);
            self.pending_status = Some(PendingNotebookExecutionStatus {
                due: now + notebook_phase_span(&status),
                status: next.to_string(),
                reason,
            });
        }
    }

    /// Materialise every status the clock already made due, and nothing else.
    ///
    /// An execution moves one phase at a time, so a read that comes several phases late
    /// applies each of them in turn and reports the one the clock has actually reached —
    /// the answer does not depend on how often anybody looked.
    pub fn advance_to(&mut self, now: SystemTime) {
        while let Some(pending) = self.pending_status.clone() {
            if now < pending.due {
                return;
            }
            self.pending_status = None;
            self.execution.status = Some(pending.status.clone());
            if let Some(reason) = pending.reason {
                self.execution.last_state_change_reason = Some(reason);
            }
            // A status the execution never leaves is where it ended, and the instant it
            // ended is the one the transition was owed at — not the instant somebody
            // looked.
            if notebook_execution_status::TERMINAL.contains(&pending.status.as_str()) {
                self.execution.end_time = Some(timestamp(pending.due));
                return;
            }
            // Whatever the phase that just landed owes next, one span later.
            if let Some(next) =
                next_notebook_execution_status(&pending.status, self.outcome.as_ref())
            {
                let reason = self.phase_reason(next);
                self.pending_status = Some(PendingNotebookExecutionStatus {
                    due: pending.due + notebook_phase_span(&pending.status),
                    status: next.to_string(),
                    reason,
                });
            }
        }
    }

    /// The reason a move to `status` records, for a move that has one to give: the stop
    /// progression says the caller asked for the stop, the failing phases carry the reason
    /// this execution's own run gave ([`NotebookOutcome::failure_reason`]) — the notebook's
    /// exception, or the sentence that says no notebook was run — and the phases on the way,
    /// like the finishing ones, have nothing to report.
    fn phase_reason(&self, status: &str) -> Option<String> {
        if NOTEBOOK_EXECUTION_STOP_PROGRESSION.contains(&status) {
            return Some(NOTEBOOK_STOPPED_REASON.to_string());
        }
        if NOTEBOOK_EXECUTION_FAILURE_PROGRESSION.contains(&status) {
            return self
                .outcome
                .as_ref()
                .and_then(NotebookOutcome::failure_reason);
        }
        None
    }

    /// Begin stopping this execution on the caller's request, answering whether there
    /// was anything to stop.
    ///
    /// "STOP_PENDING indicates that the cluster has received a `StopNotebookExecution`
    /// request and the stop is pending" (`NotebookExecution$Status` doc), so the stop is
    /// asynchronous: the execution goes `STOP_PENDING` now and the clock owes it the rest
    /// of the stop progression ([`NOTEBOOK_EXECUTION_STOP_PROGRESSION`]), which abandons
    /// whatever the run still owed. An execution already stopping was accepted once, so
    /// repeating the request does not push that due instant out.
    pub fn begin_stop(&mut self, now: SystemTime) -> bool {
        let status = self.execution.status.as_deref().unwrap_or_default();
        if notebook_execution_status::TERMINAL.contains(&status) {
            return false;
        }
        if NOTEBOOK_EXECUTION_STOP_PROGRESSION.contains(&status) {
            return true;
        }
        self.execution.status = Some(notebook_execution_status::STOP_PENDING.to_string());
        self.execution.last_state_change_reason = Some(NOTEBOOK_STOPPED_REASON.to_string());
        self.pending_status = next_notebook_execution_status(
            notebook_execution_status::STOP_PENDING,
            self.outcome.as_ref(),
        )
        .map(|next| PendingNotebookExecutionStatus {
            due: now + notebook_phase_span(notebook_execution_status::STOP_PENDING),
            status: next.to_string(),
            reason: Some(NOTEBOOK_STOPPED_REASON.to_string()),
        });
        true
    }

    /// The wire `NotebookExecutionSummary` for this record.
    pub fn to_summary(&self) -> NotebookExecutionSummary {
        NotebookExecutionSummary {
            notebook_execution_id: self.execution.notebook_execution_id.clone(),
            editor_id: self.execution.editor_id.clone(),
            notebook_execution_name: self.execution.notebook_execution_name.clone(),
            status: self.execution.status.clone(),
            start_time: self.execution.start_time,
            end_time: self.execution.end_time,
            notebook_s3_location: self.execution.notebook_s3_location.clone(),
            execution_engine_id: self
                .execution
                .execution_engine
                .as_ref()
                .map(|engine| engine.id.clone()),
        }
    }

    /// The id of the execution engine this execution names, which
    /// `ListNotebookExecutions$ExecutionEngineId` filters on.
    pub fn execution_engine_id(&self) -> Option<&str> {
        self.execution
            .execution_engine
            .as_ref()
            .map(|engine| engine.id.as_str())
    }
}
