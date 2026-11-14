// SPDX-License-Identifier: CC0-1.0

//! One cluster's submitted steps: the persisted record, its lifecycle and the wire shapes
//! `ListSteps` and `DescribeStep` project from it.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// One persisted step, built from a submitted `StepConfig`.
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub id: String,
    pub name: String,
    pub action_on_failure: Option<String>,
    pub jar: String,
    pub main_class: Option<String>,
    pub args: Vec<String>,
    pub properties: BTreeMap<String, String>,
    pub execution_role_arn: Option<String>,
    pub log_uri: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub state: String,
    pub creation_date_time: SystemTime,
    pub start_date_time: Option<SystemTime>,
    pub end_date_time: Option<SystemTime>,
    /// When the stop this step has been owed comes due, and the state it lands in: a
    /// cancel the caller asked for takes it `CANCEL_PENDING` -> [`step_state::CANCELLED`],
    /// and the shutdown of its cluster takes a step that was already running to
    /// [`step_state::INTERRUPTED`] and one that had not started yet to
    /// [`step_state::CANCELLED`]. Both are cleared once the stop has been applied.
    pub stop_due_at: Option<SystemTime>,
    /// The state [`Self::stop_due_at`] lands this step in.
    pub stop_state: Option<String>,
    /// The work this step's command submits, read off that command once when the step was
    /// submitted ([`read_step_work`]), and `None` when the command submits none this
    /// service can account for.
    pub work: Option<StepWork>,
    /// What that work ANSWERED, once the step runner has resolved it against the state the
    /// work depends on ([`run_due_steps`]) — which is what the step ends in instead of the
    /// not-executed failure every other step ends in. `None` while nothing has been
    /// resolved for this step.
    pub outcome: Option<StepOutcome>,
    /// The Spark application this step's command submits, read off that command once when
    /// the step was submitted ([`submitted_spark_application`]), and `None` when the command
    /// submits none this service can run. A step that submits one does not END until that
    /// application has been RUN and its answer recorded in
    /// [`StepRecord::outcome`] ([`run_due_step_applications`]).
    pub application: Option<SparkApplication>,
    /// The COMMAND this step's command line is, read off that command line once when the
    /// step was submitted ([`StepCommand::of`]) for a step that submits neither of the two
    /// classes above — those answer for themselves. What it answers is the status it exits
    /// with, performed when the step's run ends ([`command_outcome`]).
    pub command: Option<StepCommand>,
    /// The state this step was last PUBLISHED in, so a transition reaches the account's
    /// event bus exactly once however it was materialised ([`step_events`]). `None` while
    /// nothing of this step has been published.
    pub published_state: Option<String>,
}

impl StepRecord {
    /// Persist a submitted `StepConfig` under `id`.
    ///
    /// The step is `PENDING`: it is queued behind whatever its cluster is already running,
    /// and the cluster's step runner ([`ClusterRecord::advance_steps`]) is what starts it
    /// once the cluster is ready and one of its concurrency slots is free.
    ///
    /// The work the command submits is READ here ([`read_step_work`]), because it follows
    /// from the command and the command never changes; what that work answers is resolved
    /// later, against the state it depends on.
    pub fn from_config(
        id: String,
        config: StepConfig,
        execution_role_arn: Option<String>,
        now: SystemTime,
    ) -> Self {
        let monitoring = config
            .step_monitoring_configuration
            .as_ref()
            .and_then(|monitoring| monitoring.s3_monitoring_configuration.as_ref());
        let args = config.hadoop_jar_step.args.unwrap_or_default();
        // The work the step's command submits follows from the command, which never
        // changes, so it is read once here and resolved against the metastore when the
        // runner dispatches the step.
        let work = read_step_work(&config.hadoop_jar_step.jar, &args);
        // The application the command submits, read the same way and for the same reason:
        // the command never changes, and what it submits is what the step will RUN.
        let application = submitted_spark_application(&config.hadoop_jar_step.jar, &args);
        // The command the step's command line is, read the same way. It is read only for a
        // step that submits neither of the two classes above: an application and a statement
        // resolvable in the cluster's metastore answer for themselves, and what the command
        // ANSWERS is the status it exits with, performed when the step's run ends.
        let command = (work.is_none() && application.is_none())
            .then(|| StepCommand::of(&config.hadoop_jar_step.jar, &args))
            .flatten();
        Self {
            id,
            name: config.name,
            action_on_failure: config.action_on_failure,
            jar: config.hadoop_jar_step.jar,
            main_class: config.hadoop_jar_step.main_class,
            args,
            properties: config
                .hadoop_jar_step
                .properties
                .unwrap_or_default()
                .into_iter()
                .filter_map(|entry| Some((entry.key?, entry.value.unwrap_or_default())))
                .collect(),
            execution_role_arn,
            log_uri: monitoring.and_then(|s3| s3.log_uri.clone()),
            encryption_key_arn: monitoring.and_then(|s3| s3.encryption_key_arn.clone()),
            state: step_state::PENDING.to_string(),
            creation_date_time: now,
            start_date_time: None,
            end_date_time: None,
            stop_due_at: None,
            stop_state: None,
            work,
            outcome: None,
            application,
            command,
            published_state: None,
        }
    }

    /// Whether this step is still owed the RUN of the application it submitted: it submitted
    /// one, nothing has answered for it yet, and the step is neither terminal nor stopping.
    ///
    /// Such a step does not end ([`Self::run_between`]): the state it ends in is the state
    /// its application answered, so it stays `RUNNING` until there is an answer rather than
    /// being reported COMPLETED or FAILED for work nothing has performed yet.
    #[must_use]
    pub fn awaits_application(&self) -> bool {
        self.application.is_some()
            && self.outcome.is_none()
            && !self.is_terminal()
            && self.stop_due_at.is_none()
    }

    /// Whether this step is still owed the PERFORMANCE of the command it submitted: it
    /// submitted one, nothing has answered for it yet, and the step is neither terminal nor
    /// stopping.
    ///
    /// Such a step does not end ([`Self::run_between`]) for the same reason one awaiting its
    /// application does not: the state it ends in is the status its command exited with, so
    /// it stays `RUNNING` until that command has been performed
    /// ([`resolve_due_step_commands`]) rather than being reported COMPLETED or FAILED for a
    /// command nothing has performed yet.
    #[must_use]
    pub fn awaits_command(&self) -> bool {
        self.command.is_some()
            && self.outcome.is_none()
            && !self.is_terminal()
            && self.stop_due_at.is_none()
    }

    /// Begin cancelling this step on the caller's request.
    ///
    /// Answers `false` without touching the step when it is past cancelling: "you can
    /// cancel steps that are in a `PENDING` or `RUNNING` state" (`CancelSteps` doc),
    /// so a step that has already finished, failed or been cancelled is refused in the
    /// request's own `CancelStepsInfo` rather than silently reported as submitted.
    ///
    /// "CancelSteps is idempotent but asynchronous" (`CancelSteps` doc): the step goes
    /// `CANCEL_PENDING` now and the clock owes it the move to `CANCELLED`, so a second
    /// cancel of the same step reports the same submission and changes nothing.
    pub fn begin_cancellation(&mut self, now: SystemTime) -> bool {
        // "CancelSteps is idempotent but asynchronous" (`CancelSteps` doc): a step
        // whose cancel is already pending was accepted once, so repeating the request
        // reports the same submission and does not push its due instant out.
        if self.state == step_state::CANCEL_PENDING {
            return true;
        }
        if !CANCELLABLE_STEP_STATES.contains(&self.state.as_str()) {
            return false;
        }
        self.state = step_state::CANCEL_PENDING.to_string();
        self.stop_due_at = Some(now + TRANSITION_SPAN);
        self.stop_state = Some(step_state::CANCELLED.to_string());
        true
    }

    /// Owe this step the stop its cluster's shutdown brings, due at `terminated_at`.
    ///
    /// A step that has reached a terminal state is already done with and keeps it. One
    /// that was running when the shutdown began does not finish: it is `INTERRUPTED`, the
    /// state `StepState` carries for a step whose run was cut short. One that had not
    /// started is `CANCELLED`, since it never ran at all. A step already stopping keeps
    /// the earlier of the two due instants, so a cluster going away cannot push a cancel
    /// the caller already asked for further out.
    pub fn stop_with_cluster(&mut self, terminated_at: SystemTime) {
        if self.is_terminal() {
            return;
        }
        let stop_state = if self.state == step_state::RUNNING {
            step_state::INTERRUPTED
        } else if self.state == step_state::CANCEL_PENDING {
            // The caller's cancel is already owed; the cluster's shutdown only brings it
            // forward if it lands first.
            self.stop_due_at = Some(
                self.stop_due_at
                    .map_or(terminated_at, |owed| owed.min(terminated_at)),
            );
            return;
        } else {
            step_state::CANCELLED
        };
        self.stop_due_at = Some(
            self.stop_due_at
                .map_or(terminated_at, |owed| owed.min(terminated_at)),
        );
        self.stop_state = Some(stop_state.to_string());
    }

    /// Whether this step has reached a state it never leaves.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STEP_STATES.contains(&self.state.as_str())
    }

    /// Whether this step will never run: it is terminal, or it is stopping and the stop it
    /// is owed is the one a step that never ran lands in.
    pub(super) fn will_not_run(&self) -> bool {
        self.is_terminal()
            || self.stop_state.as_deref() == Some(step_state::CANCELLED)
            || self.state == step_state::CANCEL_PENDING
    }

    /// Materialise the transition the clock already made due, and nothing else: a step
    /// that was owed a stop reaches the state that stop lands it in once its instant has
    /// passed.
    pub fn advance_to(&mut self, now: SystemTime) {
        let Some(due) = self.stop_due_at else {
            return;
        };
        if now < due {
            return;
        }
        let Some(stop_state) = self.stop_state.take() else {
            self.stop_due_at = None;
            return;
        };
        self.state = stop_state;
        self.end_date_time = Some(due);
        self.stop_due_at = None;
    }

    /// Report where this step stands at `now`, given the `start` and `end` instants its
    /// cluster's step runner computed for it ([`ClusterRecord::advance_steps`]).
    ///
    /// A step whose turn has not come yet stays `PENDING`; one whose start has passed is
    /// `RUNNING` and records when it started; one whose span has passed has ENDED, and it
    /// ends in the state this service can account for.
    ///
    /// That end is the state the step's own submission ANSWERED, when this service could run
    /// or resolve it ([`StepOutcome`]): a Spark application the step submitted is RUN and the
    /// step ends in what that run came to ([`run_due_step_applications`]) — such a step does
    /// not end until it has ([`Self::awaits_application`]) — and a statement whose only
    /// effect is reading, submitted to a cluster whose Hive metastore this service holds,
    /// ends `COMPLETED` when every table it names resolves and `FAILED` with the driver's
    /// table-not-found when one does not ([`run_due_steps`]). A step that submits neither of
    /// those submits a COMMAND, and it ends in the status that command exited with — zero is
    /// `COMPLETED` and anything else `FAILED` ([`resolve_due_step_commands`]) — which is what
    /// `AddJobFlowSteps` documents a step's completion as; such a step does not end until
    /// that command has been performed ([`Self::awaits_command`]).
    ///
    /// For every other step that end is the modelled FAILURE state, with the details
    /// saying why ([`STEP_NOT_EXECUTED_REASON`]): this service runs the step lifecycle —
    /// the queue, the concurrency slots, the timeline — but it executed nothing for that
    /// submission, so it produced no outcome, and `COMPLETED` ("the step completed
    /// successfully") would be a success for work nothing performed. The reason is what
    /// tells a caller that apart from an application that really failed.
    ///
    /// Because that failure is this service's own rather than the application's, it does
    /// NOT stand in for one in deciding what becomes of the rest of the queue: the step's
    /// `ActionOnFailure` is recorded and reported, and nothing cancels the steps behind it
    /// or shuts the cluster down on the strength of an outcome no application produced.
    ///
    /// The instants come from the schedule, not from `now`, so two reads of one step report
    /// the same timeline. A step that was stopped, or that has already reached a terminal
    /// state, keeps what it has.
    pub(super) fn run_between(&mut self, start: SystemTime, end: SystemTime, now: SystemTime) {
        if self.is_terminal() || self.stop_due_at.is_some() {
            return;
        }
        if now >= end {
            // A step whose application has not run yet, or whose command has not been
            // performed yet, does not end: what it ends in is what that run answers
            // ([`Self::awaits_application`], [`Self::awaits_command`]).
            if self.awaits_application() || self.awaits_command() {
                self.state = step_state::RUNNING.to_string();
                self.start_date_time = Some(start);
                return;
            }
            self.state = self
                .outcome
                .as_ref()
                .map_or_else(|| step_state::FAILED.to_string(), |outcome| outcome.state.clone());
            self.start_date_time = Some(start);
            self.end_date_time = Some(end);
            return;
        }
        if now >= start {
            self.state = step_state::RUNNING.to_string();
            self.start_date_time = Some(start);
        }
    }

    /// The wire `HadoopStepConfig` for this record.
    fn to_config(&self) -> HadoopStepConfig {
        HadoopStepConfig {
            jar: Some(self.jar.clone()),
            properties: Some(
                self.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<HashMap<String, String>>(),
            ),
            main_class: self.main_class.clone(),
            args: optional_list(&self.args),
        }
    }

    /// The wire `StepStatus` for this record.
    fn to_status(&self) -> StepStatus {
        // A step that ENDED in a failure says why, in both members the model gives one:
        // "the reason for the step failure" (`FailureDetails$Reason`) and the descriptive
        // message beside it. Where the step's own WORK answered, that answer is the
        // failure ([`StepOutcome::failure`]); where this service ran the lifecycle and no
        // application ([`STEP_NOT_EXECUTED_REASON`]), that is. Every other state has
        // nothing to report — "currently, the service provides no code for the state
        // change" (`StepStateChangeReason$Code` doc), so the code member stays absent even
        // where the message is given.
        let failed = self.state == step_state::FAILED;
        let (reason, message) = match self.outcome.as_ref().and_then(|outcome| {
            outcome
                .failure
                .as_ref()
                .map(|failure| (failure.reason.clone(), failure.message.clone()))
        }) {
            Some(answered) => answered,
            None => (
                STEP_NOT_EXECUTED_REASON.to_string(),
                STEP_NOT_EXECUTED_MESSAGE.to_string(),
            ),
        };
        StepStatus {
            state: Some(self.state.clone()),
            // A step that did NOT fail has nothing to report in the reason — it was not a
            // failure and "currently, the service provides no code for the state change"
            // (`StepStateChangeReason$Code` doc) — so it carries the EMPTY reason the
            // service answers with rather than either a message or no member at all.
            state_change_reason: Some(if failed {
                StepStateChangeReason {
                    code: None,
                    message: Some(reason.clone()),
                }
            } else {
                StepStateChangeReason {
                    code: None,
                    message: None,
                }
            }),
            failure_details: failed.then_some(FailureDetails {
                reason: Some(reason),
                message: Some(message),
                // No step log was written, so there is no path to one to report.
                log_file: None,
            }),
            timeline: Some(StepTimeline {
                creation_date_time: Some(timestamp(self.creation_date_time)),
                start_date_time: self.start_date_time.map(timestamp),
                end_date_time: self.end_date_time.map(timestamp),
            }),
        }
    }

    /// The wire `Step` for this record — what `DescribeStep` answers with.
    pub fn to_step(&self) -> Step {
        Step {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            config: Some(self.to_config()),
            action_on_failure: self.action_on_failure.clone(),
            status: Some(self.to_status()),
            execution_role_arn: self.execution_role_arn.clone(),
            log_uri: self.log_uri.clone(),
            encryption_key_arn: self.encryption_key_arn.clone(),
        }
    }

    /// The wire `StepSummary` for this record — what `ListSteps` answers with.
    pub fn to_summary(&self) -> StepSummary {
        StepSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            config: Some(self.to_config()),
            action_on_failure: self.action_on_failure.clone(),
            status: Some(self.to_status()),
            log_uri: self.log_uri.clone(),
            encryption_key_arn: self.encryption_key_arn.clone(),
        }
    }
}
