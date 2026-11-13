// SPDX-License-Identifier: CC0-1.0

//! The STORE: every write this service's records take, and every transition the
//! clock owes them.
//!
//! One concern, split out of [`super`] so that the records themselves (what a state
//! machine, an execution, an activity task or a Map Run HOLDS) are read apart from
//! the operations over them. The type is [`super::ComAmazonawsSfnAwsstepfunctionsState`],
//! declared beside its records in the module root; this file carries its inherent
//! `impl` and the helpers only those methods use.
//!
//! The admission rule every write here follows: the store is capped in RECORDS and in
//! BYTES ([`super::MAX_STORE_BYTES`]), so a write is admitted against everything it
//! adds — the record, the task it schedules, the Map Runs it starts and the history
//! events its own life will append — and a write the store has no room for changes
//! nothing.

use std::time::SystemTime;

use super::*;

/// What one `UpdateStateMachine` changes about a stored state machine.
///
/// Every member is optional because the operation changes only what the request
/// carries: "you must include at least one of definition or roleArn", and the
/// configuration structures it leaves out keep the values the state machine holds.
#[derive(Debug, Clone)]
pub struct StateMachineUpdate {
    pub definition: Option<String>,
    pub role_arn: Option<String>,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub tracing_configuration: Option<TracingConfiguration>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// The revision the update moves the state machine to; see
    /// [`ComAmazonawsSfnAwsstepfunctionsState::fresh_revision_id`].
    pub revision_id: String,
    /// When the update happened, reported as `updateDate`.
    pub now: SystemTime,
}

/// Why a state machine update was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::apply_state_machine_update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateMachineUpdateRejection {
    /// The store holds no state machine under that ARN — the modelled
    /// `StateMachineDoesNotExist`.
    NotFound,
    /// The state machine is being deleted — the modelled `StateMachineDeleting`.
    Deleting,
    /// The updated record would take the store past [`MAX_STORE_BYTES`].
    NoRoom,
}

/// What a `SendTask*` quoting a token found when it looked for a handed-over callback
/// token ([`ComAmazonawsSfnAwsstepfunctionsState::answer_callback_handover`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackHandoverReply {
    /// The answer was recorded against the handover: this call ANSWERED the task, and the
    /// walk whose call carries the token finishes the state with it when that call comes
    /// back.
    Recorded,
    /// The handover has already been answered — one token answers one task — so this
    /// call resolves nothing.
    Answered,
    /// The store holds no handover of that token: no call carrying it is in flight, so
    /// the task the store records for it is what answers.
    NoHandover,
}

impl ComAmazonawsSfnAwsstepfunctionsState {
    /// Materialise every transition the clock has already made due.
    ///
    /// This is the ONE write a reporting operation performs, and it is owed whether
    /// or not anybody called: an execution whose outcome came due is reported
    /// terminal, an activity task that ran out of time is closed and the execution
    /// waiting at it has failed, a map run whose items were due reports the status
    /// they produced, and a state machine whose deletion came due is gone.
    /// Tasks settle before executions and executions before deletions, so an outcome
    /// owed before the machine disappeared is still recorded.
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_activity_tasks(now);
        self.settle_callback_handovers(now);
        self.settle_job_tasks(now);
        let mut still_pending = Vec::with_capacity(self.pending_executions.len());
        for arn in std::mem::take(&mut self.pending_executions) {
            let Some(execution) = self.executions.get_mut(&arn) else {
                continue;
            };
            let Some(due_at) = execution.pending.as_ref().map(|pending| pending.due_at) else {
                continue;
            };
            if due_at > now {
                // The outcome is still owed, but the events the walk produced before
                // it are not: each one is reported from its own instant on.
                execution.advance_history(now);
                still_pending.push(arn);
                continue;
            }
            execution.apply_pending();
            // The outcome the clock just made due is also what the execution publishes
            // about having ended, so the observations are queued at the instant the
            // transition is materialised rather than when somebody next looks.
            self.observe_execution_settled(&arn);
        }
        self.pending_executions = still_pending;

        let mut still_running = Vec::with_capacity(self.pending_map_runs.len());
        for arn in std::mem::take(&mut self.pending_map_runs) {
            let Some(map_run) = self.map_runs.get_mut(&arn) else {
                continue;
            };
            let before = map_run.retained_bytes;
            // The items the clock finished are counted, and a Map Run whose budget
            // one of them breached stops running here.
            map_run.advance(now);
            map_run.retained_bytes = map_run.measure();
            let after = map_run.retained_bytes;
            // A Map Run is still owed something while it holds an item the clock has
            // not reached, or the status its walk computed.
            if map_run.pending.is_some() || !map_run.items.is_empty() {
                still_running.push(arn);
            }
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
        }
        self.pending_map_runs = still_running;

        let mut still_deleting = Vec::with_capacity(self.deleting_state_machines.len());
        for arn in std::mem::take(&mut self.deleting_state_machines) {
            let due = self
                .state_machines
                .get(&arn)
                .and_then(|machine| machine.delete_due_at);
            let Some(due) = due else {
                continue;
            };
            if due > now {
                still_deleting.push(arn);
                continue;
            }
            self.remove_state_machine(&arn);
        }
        self.deleting_state_machines = still_deleting;

        // Every status the steps above materialised — an execution's terminal outcome
        // coming due, an activity task that ran out of time failing the execution
        // waiting at it — owes an EventBridge event, and so does every status a CALL
        // wrote before it reached this point. Both are recorded here rather than at each
        // of those sites, so none of them can forget; the events themselves are
        // published with no lock held ([`events::publish`]).
        events::record_status_changes(self, now);
        // And every history event the steps above made reportable owes the series the
        // work it reports is counted in, recorded here for the same reason.
        self.observe_task_work();
    }

    // -----------------------------------------------------------------------
    // Activity tasks
    // -----------------------------------------------------------------------

    /// Close every outstanding activity task the clock has made due, and drop every
    /// one no execution is waiting on any more.
    ///
    /// Both are owed whether or not anybody called: a task whose `TimeoutSeconds` or
    /// `HeartbeatSeconds` ran out has failed its execution at the instant it ran out,
    /// and a task whose execution was stopped — or went with the state machine it ran
    /// on — can never be answered, so no later poll may claim it.
    fn settle_activity_tasks(&mut self, now: SystemTime) {
        // ONE scan over the outstanding tasks, which collects only the tokens this
        // instant changes something for.
        let mut timed_out: Vec<(String, SystemTime)> = Vec::new();
        let mut orphaned: Vec<String> = Vec::new();
        for task in self.activity_tasks.values() {
            if self
                .executions
                .get(&task.execution_arn)
                .is_none_or(Execution::is_terminal)
            {
                orphaned.push(task.token.clone());
                continue;
            }
            if let Some(due) = task.closes_at().filter(|due| *due <= now) {
                timed_out.push((task.token.clone(), due));
            }
        }
        for token in orphaned {
            self.discard_activity_task(&token);
        }
        for (token, due) in timed_out {
            self.time_out_activity_task(&token, due);
        }
    }

    /// Store the task a planned run scheduled for `execution_arn`, minting the
    /// `taskToken` that names it where the walk has not already, and answer that
    /// token.
    ///
    /// Called once the execution has been ADMITTED, against bytes that already
    /// include this task's ([`PlannedActivityTask::measure`]).
    ///
    /// A CALLBACK task arrives with its token and with its work already handed over:
    /// the `Task` state's own integration call carried both, so the task is recorded
    /// as one that has STARTED — which is what its `TimeoutSeconds` is measured from
    /// and what opens its heartbeat window — rather than queued for a poll to claim.
    pub fn schedule_activity_task(
        &mut self,
        execution_arn: &str,
        planned: PlannedActivityTask,
    ) -> String {
        // The token NAMES the task: a minted id, and the resource it belongs to, so a
        // reply to a token whose task has since been closed can still say which one
        // that was. See [`task_token`].
        let token = match planned.token {
            Some(token) => token,
            None => task_token(
                &self.fresh_uuid(),
                planned.activity_arn.as_deref().unwrap_or_default(),
            ),
        };
        // A callback task's work is already with the target the call handed it to,
        // which is where an activity task's claim would put it.
        let started = planned.activity_arn.is_none();
        let task = ActivityTask {
            token: token.clone(),
            activity_arn: planned.activity_arn,
            execution_arn: execution_arn.to_string(),
            state_name: planned.state_name,
            input: planned.input,
            state_input: planned.state_input,
            available_at: planned.available_at,
            worker_name: None,
            started_at: started.then_some(planned.available_at),
            heartbeat_at: started.then_some(planned.available_at),
            timeout_seconds: planned.timeout_seconds,
            heartbeat_seconds: planned.heartbeat_seconds,
            // What the scheduling state's retriers have spent so far: EMPTY for the
            // first attempt of a task, and for a CALLBACK task the budget the walk
            // that suspended at it had already used, so the answer's own walk takes
            // it up instead of restarting it.
            retries_taken: planned.retries_taken,
            retained_bytes: 0,
        };
        self.store_activity_task(task);
        token
    }

    /// Queue `task` under its activity, counting what its record retains.
    ///
    /// The one place a task record enters the store, so a task a `Retry` scheduled is
    /// queued and counted exactly as the attempt before it was. A CALLBACK task goes
    /// in under its token alone: it belongs to no activity, so no poll of one may
    /// reach it.
    fn store_activity_task(&mut self, mut task: ActivityTask) {
        task.retained_bytes = task.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(task.retained_bytes);
        let token = task.token.clone();
        if let Some(activity_arn) = &task.activity_arn {
            self.activity_tasks_by_activity
                .entry(activity_arn.clone())
                .or_default()
                .push(token.clone());
        }
        self.activity_tasks.insert(token, task);
    }

    /// Hand the longest-waiting available task of `activity_arn` to a worker, and
    /// report the task as it now stands — or `None` when the activity has none to
    /// hand out.
    ///
    /// This is the one write `GetActivityTask` makes: the operation exists to assign
    /// a task to a worker ("you can provide an arbitrary name in order to identify
    /// the worker that the task is assigned to"), so the task moves from scheduled to
    /// started, its heartbeat window opens, and the execution's history records it.
    /// A task already handed out is never handed out twice, and one whose execution
    /// has not reached its state yet (every `Wait` before it is still owed) is not
    /// available.
    pub fn claim_activity_task(
        &mut self,
        activity_arn: &str,
        worker_name: Option<&str>,
        now: SystemTime,
    ) -> Option<ActivityTask> {
        let queue = self.activity_tasks_by_activity.get(activity_arn)?;
        let token = queue
            .iter()
            .find(|token| {
                self.activity_tasks
                    .get(*token)
                    .is_some_and(|task| task.is_queued() && task.available_at <= now)
            })
            .cloned()?;
        let task = self.activity_tasks.get_mut(&token)?;
        task.worker_name = worker_name.map(str::to_string);
        task.started_at = Some(now);
        // The claim starts the heartbeat window: a worker that has just been handed a
        // task has not missed a heartbeat.
        task.heartbeat_at = Some(now);
        let claimed = task.clone();
        if let Some(execution) = self.executions.get_mut(&claimed.execution_arn) {
            let before = execution.retained_bytes;
            // "You can provide an arbitrary name in order to identify the worker that
            // the task is assigned to": the event the claim writes is where that name
            // is recorded, against the state whose work the claim took.
            execution.push_state_event(
                EVENT_ACTIVITY_STARTED,
                &claimed.state_name,
                now,
                EventDetails {
                    worker_name: claimed.worker_name.clone(),
                    ..EventDetails::default()
                },
            );
            execution.retained_bytes = execution.measure();
            let after = execution.retained_bytes;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
        }
        // The claim made an event of the task's own work reportable — the worker took
        // it — so the series that work is counted in are queued at this instant rather
        // than at whatever call happens to settle the store next.
        self.observe_task_work();
        Some(claimed)
    }

    /// Restart the heartbeat window of the task `token` names at `now`, and report
    /// whether the store held one.
    ///
    /// "This action resets the Heartbeat clock": the window the scheduling state's
    /// `HeartbeatSeconds` opens is measured from the last heartbeat, which the claim
    /// starts and this moves on.
    /// The task's `TimeoutSeconds` is read from the instant the state scheduled it and
    /// is deliberately NOT touched, because "the Timeout of a task ... is its maximum
    /// allowed duration, regardless of the number of SendTaskHeartbeat requests
    /// received". No byte of the record changes size, so the store's total is
    /// unaffected.
    pub fn restart_heartbeat_window(&mut self, token: &str, now: SystemTime) -> bool {
        let Some(task) = self.activity_tasks.get_mut(token) else {
            return false;
        };
        task.heartbeat_at = Some(now);
        true
    }

    /// Drop the task `token` names, giving back what its record retained, and report
    /// it. `None` when the store holds no task under that token.
    pub fn discard_activity_task(&mut self, token: &str) -> Option<ActivityTask> {
        let task = self.activity_tasks.remove(token)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(task.retained_bytes);
        let queued_under = task
            .activity_arn
            .as_ref()
            .and_then(|activity_arn| self.activity_tasks_by_activity.get_mut(activity_arn));
        if let Some(queue) = queued_under {
            queue.retain(|queued| queued != token);
            if queue.is_empty() {
                if let Some(activity_arn) = &task.activity_arn {
                    self.activity_tasks_by_activity.remove(activity_arn);
                }
            }
        }
        Some(task)
    }

    // -----------------------------------------------------------------------
    // Handed-over callback tokens
    // -----------------------------------------------------------------------

    /// Record that the work of `state_name` in `execution_arn` has been handed `token`,
    /// so a `SendTask*` quoting it is answered from `now` on — before the call carrying it
    /// has come back, and so before the task it suspends at exists ([`super::callbacks`]).
    ///
    /// The record is admitted rather than refused, however full the store is: the token is
    /// already with the work, so a handover the store declined would leave that work
    /// holding a token nothing can answer. Its bytes are counted exactly as every other
    /// record's are, and it is given back the moment the call comes back.
    pub fn hand_over_callback(
        &mut self,
        token: &str,
        execution_arn: &str,
        state_name: &str,
        now: SystemTime,
    ) {
        let mut handover = CallbackHandover {
            execution_arn: execution_arn.to_string(),
            state_name: state_name.to_string(),
            handed_at: now,
            answer: None,
            retained_bytes: 0,
        };
        handover.retained_bytes = handover.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(handover.retained_bytes);
        self.callback_handovers.insert(token.to_string(), handover);
    }

    /// Record `answer` against the handover `token` names, and report what the call
    /// quoting it has to answer.
    ///
    /// One token answers one task, so a handover that already carries an answer takes no
    /// second one.
    pub fn answer_callback_handover(
        &mut self,
        token: &str,
        answer: integration::Replied,
    ) -> CallbackHandoverReply {
        let Some(handover) = self.callback_handovers.get_mut(token) else {
            return CallbackHandoverReply::NoHandover;
        };
        if handover.answer.is_some() {
            return CallbackHandoverReply::Answered;
        }
        let before = handover.retained_bytes;
        handover.answer = Some(answer);
        handover.retained_bytes = handover.measure();
        let after = handover.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        CallbackHandoverReply::Recorded
    }

    /// Whether the store holds a handover of `token` that nothing has answered yet, which
    /// is a task outstanding with the work the call handed it to.
    pub fn callback_handover_is_open(&self, token: &str) -> bool {
        self.callback_handovers
            .get(token)
            .is_some_and(|handover| handover.answer.is_none())
    }

    /// END the handover of `token` and report what answered it, if anything did.
    ///
    /// Called by the walk when the call that carried the token comes back: from here on
    /// the token is answered through the task the store records for the suspended state,
    /// so reading the answer and ending the handover are ONE step — no answer can arrive
    /// between them and be lost.
    pub fn settle_callback_handover(&mut self, token: &str) -> Option<integration::Replied> {
        let handover = self.discard_callback_handover(token)?;
        handover.answer
    }

    /// Drop the handover `token` names, giving back what its record retained.
    fn discard_callback_handover(&mut self, token: &str) -> Option<CallbackHandover> {
        let handover = self.callback_handovers.remove(token)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(handover.retained_bytes);
        Some(handover)
    }

    /// Drop every handover whose call cannot still be in flight
    /// ([`MAX_HANDOVER_DURATION`]).
    ///
    /// The walk ends its own handover when its call comes back, so this closes only the
    /// ones no walk is waiting for any more — an interpretation that ended at its own
    /// deadline, a thread that went away — and it is owed whether or not anybody called.
    fn settle_callback_handovers(&mut self, now: SystemTime) {
        let expired: Vec<String> = self
            .callback_handovers
            .iter()
            .filter(|(_, handover)| after(handover.handed_at, MAX_HANDOVER_DURATION) <= now)
            .map(|(token, _)| token.clone())
            .collect();
        for token in expired {
            self.discard_callback_handover(&token);
        }
    }

    /// Record a worker's reply to the task `token` names: the task is CLOSED and the
    /// execution waiting at it carries on with what the continuation produced.
    ///
    /// `events` and `pending` are what [`plan_reply`] computed for the walk that
    /// continued from the reply, and `extra_bytes` what the next activity task that
    /// walk stopped at — and the Map Runs it started — will retain, so a reply is
    /// admitted against the store's ceiling exactly as a start is rather than growing
    /// it past [`MAX_STORE_BYTES`].
    ///
    /// The task's record is dropped, giving back the bytes it held: a token answers
    /// ONE task, so a later reply under the same token resolves nothing.
    /// `stopped_at` is the unsuccessful step the continuation's own walk stopped at,
    /// when it stopped at one ([`Planned::stopped_at`]), which is the step a later
    /// `RedriveExecution` continues the execution from.
    pub fn answer_activity_task(
        &mut self,
        token: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        stopped_at: Option<asl::RedriveStep>,
    ) -> Result<(), TaskReplyRejection> {
        self.close_activity_task(token, events, pending, extra_bytes, stopped_at, false)
    }

    /// Record the reply that reports the task `token` names FAILED.
    ///
    /// The same close as [`Self::answer_activity_task`], plus the one thing a failure
    /// leaves behind: the attempt stopped AT this task's state, so that state and the
    /// raw input it was entered with are recorded as the step a later
    /// `RedriveExecution` continues the execution from.
    pub fn fail_activity_task(
        &mut self,
        token: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
    ) -> Result<(), TaskReplyRejection> {
        self.close_activity_task(token, events, pending, 0, None, true)
    }

    /// Close the task `token` names, recording what the call that closed it computed.
    ///
    /// `stopped_at_task` is whether the attempt STOPPED at this task's state (the task
    /// failed) rather than carrying on past it (the task was answered with a result).
    fn close_activity_task(
        &mut self,
        token: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        walked_stopped_at: Option<asl::RedriveStep>,
        stopped_at_task: bool,
    ) -> Result<(), TaskReplyRejection> {
        let Some(task) = self.activity_tasks.get(token) else {
            return Err(TaskReplyRejection::Gone);
        };
        let execution_arn = task.execution_arn.clone();
        // The attempt stopped either AT this task's state — the worker reported it
        // failed and no retrier answered — or at the step the continuation's own walk
        // failed at, if it failed at one.
        let stopped_at = if stopped_at_task {
            Some(asl::RedriveStep {
                state_name: task.state_name.clone(),
                state_input: task.state_input.clone(),
            })
        } else {
            walked_stopped_at
        };
        self.continue_execution(&execution_arn, events, pending, extra_bytes, stopped_at)?;
        self.discard_activity_task(token);
        Ok(())
    }

    /// Carry the execution `execution_arn` names on with what a continuation walk
    /// produced: its events are APPENDED to the history its earlier states left and
    /// the outcome it computed is owed to the clock.
    ///
    /// The one place a continuation reaches an execution, whatever the execution was
    /// waiting at — a worker's reply to an activity task, or the job a run-a-job
    /// integration started — so both are admitted against the store's ceiling the
    /// same way: `extra_bytes` is what the walk's own next task and Map Runs will
    /// retain, and a continuation the store has no room for changes nothing.
    ///
    /// `stopped_at` is the step a later `RedriveExecution` continues the execution
    /// from, for an attempt that STOPPED at the state it was waiting at.
    fn continue_execution(
        &mut self,
        execution_arn: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        stopped_at: Option<asl::RedriveStep>,
    ) -> Result<(), TaskReplyRejection> {
        // A task whose execution is terminal — stopped, or failed when the clock
        // closed an earlier bound — is answered by nobody: `settle` discards it, and
        // one that is still here when this runs is in that same state.
        let Some(execution) = self
            .executions
            .get(execution_arn)
            .filter(|execution| !execution.is_terminal())
        else {
            return Err(TaskReplyRejection::Gone);
        };
        let before = execution.retained_bytes;
        let mut candidate = execution.clone();
        candidate.apply_continuation(events, pending);
        if let Some(stopped_at) = stopped_at {
            candidate.redrive_step = Some(stopped_at);
        }
        let after = candidate.measure();
        if after + extra_bytes > before && !self.has_room_for(after + extra_bytes - before) {
            return Err(TaskReplyRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        // The continuation's outcome is owed to the clock, so this execution joins the
        // settle queue; one that stopped at ANOTHER activity task — or parked at
        // another job — owes the clock nothing and does not.
        self.pending_executions
            .retain(|pending| pending != execution_arn);
        if candidate.pending.is_some() {
            self.pending_executions.push(execution_arn.to_string());
        }
        self.executions
            .insert(execution_arn.to_string(), candidate);
        Ok(())
    }

    /// Close `token`'s task at `due`, the instant its own bound made it due, and fail
    /// the execution waiting at it with the error the Amazon States Language names
    /// for a task that ran out of time — unless the scheduling state RETRIES it.
    ///
    /// `due` is the instant the bound was reached, not the instant somebody looked:
    /// two callers reading afterwards see the same `stopDate`.
    fn time_out_activity_task(&mut self, token: &str, due: SystemTime) {
        // "If the task runs longer than the specified seconds, this state fails with a
        // States.Timeout error name": the state's own `Retry` answers for that error
        // before the execution does, and the attempt it schedules is a task a worker
        // polls for exactly as this one was.
        if self.retry_activity_task(token, TASK_TIMEOUT_ERROR, EVENT_ACTIVITY_TIMED_OUT, due) {
            return;
        }
        let Some(task) = self.discard_activity_task(token) else {
            return;
        };
        let cause = task_timeout_cause(&task);
        // The event the language names this closure with is the one for the kind of
        // work that ran out of time: an activity's task times out as an activity,
        // while a callback task's work is the `Task` state's own integration.
        let timed_out = match task.activity_arn {
            Some(_) => EVENT_ACTIVITY_TIMED_OUT,
            None => EVENT_TASK_TIMED_OUT,
        };
        let Some(execution) = self.executions.get_mut(&task.execution_arn) else {
            return;
        };
        if execution.is_terminal() {
            return;
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_FAILED.to_string();
        execution.stop_date = Some(due);
        execution.error = Some(TASK_TIMEOUT_ERROR.to_string());
        execution.cause = Some(cause);
        // The attempt stopped AT this task's state, so that is the step a later
        // redrive continues from — the same step a worker's reported failure leaves.
        execution.redrive_step = Some(asl::RedriveStep {
            state_name: task.state_name.clone(),
            state_input: task.state_input.clone(),
        });
        execution.push_state_event(
            timed_out,
            &task.state_name,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        execution.push_detailed_event(
            EVENT_EXECUTION_FAILED,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        // The record is re-measured exactly as a stop re-measures it: the execution
        // is terminal now, so the bytes a stop had reserved are given back and the
        // events written here are counted.
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        let execution_arn = task.execution_arn;
        self.pending_executions
            .retain(|pending| pending != &execution_arn);
        // The bound the clock reached is this execution's TERMINAL status, reached
        // without an owed outcome for `settle` to apply, so what it publishes about
        // having ended is queued here — the one place that publishes it
        // ([`ComAmazonawsSfnAwsstepfunctionsState::observe_execution_settled`]), so an
        // execution the clock failed is counted exactly as one that failed any other
        // way.
        self.observe_execution_settled(&execution_arn);
    }

    /// Re-schedule the task `token` names for its next attempt, when the state that
    /// scheduled it declares a `Retry` that answers for `error`, and report whether it
    /// did.
    ///
    /// "Retry […] an array of objects, called retriers": the first retrier whose
    /// `ErrorEquals` names the error decides, it makes at most its own `MaxAttempts`
    /// attempts, and the next one is due `IntervalSeconds` scaled by `BackoffRate`
    /// once per attempt already taken and capped by `MaxDelaySeconds` after the
    /// failure ([`asl::activity_retry`]).
    ///
    /// The attempt that failed is CLOSED: its token answers nothing afterwards, so the
    /// worker that held it cannot report on the attempt that replaces it. An identical
    /// task is queued in its place, available once the retrier's interval has passed
    /// and carrying one more spent attempt of that retrier, and the execution's history
    /// records both the failure and the scheduling of the next attempt. The execution
    /// stays `RUNNING`, because the state has not failed yet.
    fn retry_activity_task(
        &mut self,
        token: &str,
        error: &str,
        failure_event: &str,
        at: SystemTime,
    ) -> bool {
        let Some(task) = self.activity_tasks.get(token).cloned() else {
            return false;
        };
        // A CALLBACK task's next attempt is not a record this store can queue: the
        // attempt that replaces it has to MAKE the integration call again, carrying a
        // token of its own, and only the request a walk runs inside can make one. So
        // the retriers of such a state are answered by the walk that continues the
        // execution ([`asl::Interpreter::task_state`]) and never here, and a bound
        // the clock closed is the state's failure.
        if task.activity_arn.is_none() {
            return false;
        }
        let Some(definition) = self.definition_of_execution(&task.execution_arn) else {
            return false;
        };
        let Some((retrier, delay)) =
            asl::activity_retry(&definition, &task.state_name, error, &task.retries_taken)
        else {
            return false;
        };
        // A task whose execution is no longer running is retried by nobody: it was
        // stopped, or an earlier bound of the execution already failed it.
        let Some(execution) = self
            .executions
            .get(&task.execution_arn)
            .filter(|execution| !execution.is_terminal())
            .cloned()
        else {
            return false;
        };
        let available_at = after(at, delay);
        let mut queued = task.clone();
        queued.token = task_token(
            &self.fresh_uuid(),
            task.activity_arn.as_deref().unwrap_or_default(),
        );
        queued.worker_name = None;
        queued.started_at = None;
        queued.heartbeat_at = None;
        queued.available_at = available_at;
        if queued.retries_taken.len() <= retrier {
            queued.retries_taken.resize(retrier + 1, 0);
        }
        if let Some(spent) = queued.retries_taken.get_mut(retrier) {
            *spent = spent.saturating_add(1);
        }
        let mut candidate = execution.clone();
        // The failure happened when the bound was reached; the next attempt is
        // scheduled when its interval has passed, which is also when a poll may first
        // claim it.
        candidate.push_state_event(
            failure_event,
            &task.state_name,
            at,
            EventDetails::of_failure(Some(error.to_string()), None),
        );
        candidate.push_state_event(
            EVENT_ACTIVITY_SCHEDULED,
            &queued.state_name,
            available_at,
            EventDetails::of_scheduled_task(&queued),
        );
        let before = execution.retained_bytes;
        let after_bytes = candidate.measure();
        // What the retry adds is the two events and whatever the next attempt's record
        // holds beyond the one it replaces, which is admitted against the store's
        // ceiling exactly as the first attempt was: a retry the store has no room for
        // leaves the failure to the execution rather than growing past the ceiling.
        let grows = (after_bytes + queued.measure())
            .saturating_sub(before + task.retained_bytes);
        if grows > 0 && !self.has_room_for(grows) {
            return false;
        }
        candidate.retained_bytes = after_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after_bytes)
            .saturating_sub(before);
        self.executions
            .insert(task.execution_arn.clone(), candidate);
        self.discard_activity_task(token);
        self.store_activity_task(queued);
        true
    }

    /// Re-schedule the task `token` names after the worker holding it reported it
    /// FAILED with `error`, and report whether it was retried.
    ///
    /// The error a worker reports is what the scheduling state's `Retry` is matched
    /// against, exactly as the `States.Timeout` the clock reports is: a task the state
    /// retries is queued for its next attempt and the execution stays `RUNNING`, and
    /// one it does not leaves the reply to fail the execution.
    pub fn retry_failed_activity_task(
        &mut self,
        token: &str,
        error: &str,
        at: SystemTime,
    ) -> bool {
        self.retry_activity_task(token, error, EVENT_ACTIVITY_FAILED, at)
    }

    // -----------------------------------------------------------------------
    // The jobs a run-a-job (`.sync`) integration started
    // -----------------------------------------------------------------------

    /// Fail every parked job that has outlasted the time its task was allowed, and
    /// drop every one no execution is waiting at any more.
    ///
    /// Both are owed whether or not anybody called, exactly as an activity task's own
    /// bounds are: a task whose job did not finish inside its `TimeoutSeconds` failed
    /// its execution at the instant that span ran out, and a job whose execution was
    /// stopped — or went with the state machine it ran on — resolves nothing.
    fn settle_job_tasks(&mut self, now: SystemTime) {
        let mut timed_out: Vec<(String, SystemTime)> = Vec::new();
        let mut orphaned: Vec<String> = Vec::new();
        for task in self.job_tasks.values() {
            if self
                .executions
                .get(&task.execution_arn)
                .is_none_or(Execution::is_terminal)
            {
                orphaned.push(task.execution_arn.clone());
                continue;
            }
            let due = task.times_out_at();
            if due <= now {
                timed_out.push((task.execution_arn.clone(), due));
            }
        }
        for execution_arn in orphaned {
            self.discard_job_task(&execution_arn);
        }
        for (execution_arn, due) in timed_out {
            self.time_out_job_task(&execution_arn, due);
        }
    }

    /// Record the job a planned run parked at, so the execution waits at that state
    /// until the job finishes.
    ///
    /// Called once the execution has been ADMITTED, against bytes that already
    /// include this record's ([`PlannedJob::measure`]).
    pub fn park_job(&mut self, execution_arn: &str, planned: PlannedJob, region: &str) {
        let mut task = JobTask {
            execution_arn: execution_arn.to_string(),
            state_name: planned.state_name,
            state_input: planned.state_input,
            job: planned.job,
            api: planned.api,
            parsed_output: planned.parsed_output,
            job_id: planned.job_id,
            naming: planned.naming,
            // The resource the calls that report and stop the job are taken on, in the
            // Region those calls are made in: a resource the starting request named by
            // NAME lives in that Region, so its ARN is minted here rather than where
            // the walk read the name. A minted ARN longer than any ARN this emulator
            // mints ([`MAX_ARN_BYTES`], which the store is admitted against) names no
            // resource of this account, so no access request is formed over it and the
            // invoked operation answers for such a request itself.
            acts_on: planned
                .acts_on
                .as_deref()
                .and_then(|named| planned.job.acts_on_arn(named, region))
                .filter(|arn| arn.len() <= MAX_ARN_BYTES),
            region: region.to_string(),
            parked_at: planned.parked_at,
            timeout_seconds: planned.timeout_seconds,
            frames: planned.frames,
            retained_bytes: 0,
        };
        task.retained_bytes = std::mem::size_of::<JobTask>()
            + task.execution_arn.len()
            + task.state_name.len()
            + task.state_input.len()
            + task.job_id.len()
            + naming_bytes(&task.naming)
            + task.acts_on.as_ref().map_or(0, String::len)
            + frames_bytes(&task.frames)
            + task.region.len();
        self.retained_bytes = self.retained_bytes.saturating_add(task.retained_bytes);
        self.job_tasks.insert(execution_arn.to_string(), task);
    }

    /// Drop the job the execution `execution_arn` is parked at, giving back what its
    /// record retained, and report it. `None` when the execution is parked at none.
    pub fn discard_job_task(&mut self, execution_arn: &str) -> Option<JobTask> {
        let task = self.job_tasks.remove(execution_arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(task.retained_bytes);
        Some(task)
    }

    /// Record what the job an execution was parked at ENDED as: the job is closed and
    /// the execution waiting at it carries on with what the continuation produced.
    ///
    /// `events` and `pending` are what [`plan_job_reply`] computed for the walk that
    /// continued from the job's own report, admitted against the store's ceiling
    /// exactly as a worker's reply is. `stopped_at_task` is whether the attempt
    /// STOPPED at the state that started the job — the job ended without doing its
    /// work and no `Catch` carried the execution on — and `walked_stopped_at` the
    /// unsuccessful step the continuation's own walk stopped at otherwise; either way
    /// it is the step a later `RedriveExecution` continues from.
    pub fn resolve_job_task(
        &mut self,
        execution_arn: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        walked_stopped_at: Option<asl::RedriveStep>,
        stopped_at_task: bool,
    ) -> Result<(), TaskReplyRejection> {
        let Some(task) = self.job_tasks.get(execution_arn) else {
            return Err(TaskReplyRejection::Gone);
        };
        let stopped_at = if stopped_at_task {
            Some(asl::RedriveStep {
                state_name: task.state_name.clone(),
                state_input: task.state_input.clone(),
            })
        } else {
            walked_stopped_at
        };
        self.continue_execution(execution_arn, events, pending, extra_bytes, stopped_at)?;
        self.discard_job_task(execution_arn);
        Ok(())
    }

    /// Close the job the execution `execution_arn` is parked at because its task ran
    /// out of the time it was allowed, and fail that execution with the error the
    /// Amazon States Language names for a task that ran out of time.
    ///
    /// `due` is the instant the bound was reached, not the instant somebody looked, so
    /// two callers reading afterwards see the same `stopDate`.
    fn time_out_job_task(&mut self, execution_arn: &str, due: SystemTime) {
        let Some(task) = self.discard_job_task(execution_arn) else {
            return;
        };
        let cause = job_task_timeout_cause(&task);
        let Some(execution) = self.executions.get_mut(execution_arn) else {
            return;
        };
        if execution.is_terminal() {
            return;
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_FAILED.to_string();
        execution.stop_date = Some(due);
        execution.error = Some(TASK_TIMEOUT_ERROR.to_string());
        execution.cause = Some(cause);
        // The attempt stopped AT the state that started the job, so that is the step a
        // later redrive continues from — the same step a failed task leaves.
        execution.redrive_step = Some(asl::RedriveStep {
            state_name: task.state_name.clone(),
            state_input: task.state_input.clone(),
        });
        execution.push_detailed_event(
            EVENT_TASK_TIMED_OUT,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        execution.push_detailed_event(
            EVENT_EXECUTION_FAILED,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.pending_executions
            .retain(|pending| pending != execution_arn);
        // The terminal status this bound reached is published here, for the reason
        // [`Self::time_out_activity_task`] publishes its own.
        self.observe_execution_settled(execution_arn);
    }

    /// Make the outcome the clock has already made due for the execution `arn` a
    /// FAILURE of `error` carrying `cause`, keeping the events the attempt had already
    /// produced.
    ///
    /// The one caller is the key gate ([`encryption::fail_due_runs_the_key_refuses`]):
    /// an execution does its remaining work under its state machine's customer managed
    /// key, so an outcome that comes due while that key cannot be used for the work is
    /// a failure and not the outcome the walk computed. The event the attempt ENDS on
    /// becomes `ExecutionFailed` in place — same id, same predecessor, same instant —
    /// so the history keeps one terminal event and numbers nothing twice, and the
    /// outcome is left for [`Self::settle`] to apply at the instant it was owed.
    ///
    /// Nothing is changed for an execution that is already terminal, or one whose
    /// outcome is not owed yet: this materialises what the clock made due, and never
    /// anticipates it.
    pub fn fail_due_outcome(&mut self, arn: &str, error: &str, cause: &str) {
        let Some(execution) = self.executions.get_mut(arn) else {
            return;
        };
        if execution.is_terminal() || execution.pending.is_none() {
            return;
        }
        let before = execution.retained_bytes;
        let last = execution.history.last();
        let next_id = last.map_or(0, |event| event.id) + 1;
        let follows = last.map_or(NO_PREVIOUS_EVENT, |event| {
            follows_event(event.id, &event.event_type)
        });
        let details = EventDetails::of_failure(Some(error.to_string()), Some(cause.to_string()));
        if let Some(pending) = execution.pending.as_mut() {
            pending.status = EXECUTION_STATUS_FAILED.to_string();
            // "This field is set only if the execution succeeds": a run that failed
            // reports no output.
            pending.output = None;
            pending.error = Some(error.to_string());
            pending.cause = Some(cause.to_string());
            match pending.events.last_mut() {
                Some(ending) => {
                    ending.event_type = EVENT_EXECUTION_FAILED.to_string();
                    ending.state_name = None;
                    ending.details = details;
                }
                // An attempt that planned no terminal event of its own — nothing is
                // owed but the outcome itself — ends on one minted here, following the
                // last event the history holds.
                None => pending.events.push(HistoryEvent {
                    id: next_id,
                    previous_event_id: follows,
                    event_type: EVENT_EXECUTION_FAILED.to_string(),
                    timestamp: pending.due_at,
                    state_name: None,
                    details,
                }),
            }
        }
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
    }

    /// The definition the execution `arn` RUNS, which is where its states are read
    /// from.
    ///
    /// An execution started through a version ARN — or through an alias that routed to
    /// a version — runs that version's immutable snapshot; every other runs what the
    /// state machine holds. The same choice `plan_reply`'s callers make, so a retried
    /// task's `Retry` is read from the definition the execution is running.
    fn definition_of_execution(&self, arn: &str) -> Option<String> {
        let execution = self.executions.get(arn)?;
        if let Some(version) = execution
            .state_machine_version_arn
            .as_deref()
            .and_then(|version_arn| self.state_machine_versions.get(version_arn))
        {
            return Some(version.definition.clone());
        }
        self.state_machines
            .get(&execution.state_machine_arn)
            .map(|machine| machine.definition.clone())
    }

    /// Drop every outstanding task of the executions `arns` names, for a removal that
    /// takes those executions with it.
    pub(super) fn discard_tasks_of_executions(&mut self, arns: &[String]) {
        let tokens: Vec<String> = self
            .activity_tasks
            .values()
            .filter(|task| arns.contains(&task.execution_arn))
            .map(|task| task.token.clone())
            .collect();
        for token in tokens {
            self.discard_activity_task(&token);
        }
        for arn in arns {
            self.discard_job_task(arn);
        }
    }

    /// The bytes every stored record holds together, against [`MAX_STORE_BYTES`].
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// A fresh execution name for a `StartExecution` that did not give one.
    ///
    /// From the `name` member's own documentation: "If you don't provide a name for
    /// the execution, Step Functions automatically generates a universally unique
    /// identifier (UUID) as the execution name." The value is a version-4 UUID:
    /// 122 bits drawn from the process clock mixed with this store's own counter
    /// through SplitMix64, with RFC 4122's version and variant bits set, rendered
    /// 8-4-4-4-12 in lowercase hex. The counter is what makes two mints in the same
    /// nanosecond differ.
    pub fn fresh_execution_name(&mut self) -> String {
        self.fresh_uuid()
    }

    /// A fresh id for the trailing field of one Express execution ARN (see
    /// [`EXPRESS_EXECUTION_RESOURCE`]).
    ///
    /// Minted from the same generator as an execution name, so two synchronous runs
    /// that reused one name still differ — the id is what makes the ARN unique, and
    /// the counter is what makes two mints in the same nanosecond differ.
    pub fn fresh_execution_id(&mut self) -> String {
        self.fresh_uuid()
    }

    /// One freshly drawn version-4 UUID: the single mint behind every id this store
    /// hands out, so there is one implementation of the draw.
    pub(super) fn fresh_uuid(&mut self) -> String {
        self.name_counter = self.name_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.name_counter.rotate_left(29);
        let high = splitmix64(&mut seed);
        let low = splitmix64(&mut seed);
        uuid_v4(high, low)
    }

    /// One draw in `0..bound`, from the same generator every id this store mints is
    /// drawn from; `0` for an empty span.
    ///
    /// What a weighted routing configuration is sampled with
    /// ([`Self::routed_version`]): the counter is what makes two draws in the same
    /// nanosecond differ, so consecutive starts through one alias do not all route
    /// to the same version.
    pub(super) fn draw_below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.name_counter = self.name_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.name_counter.rotate_left(29);
        splitmix64(&mut seed) % bound
    }

    /// A fresh revision id for a state machine `UpdateStateMachine` just changed.
    ///
    /// `revisionId` is "the revision identifier for the updated state machine", and
    /// it has to differ on every update so a client can order revisions, so it is
    /// minted from the same UUID generator as an execution name.
    pub fn fresh_revision_id(&mut self) -> String {
        self.fresh_uuid()
    }

    /// Whether the store can still take a record of `bytes`.
    ///
    /// Every growth site asks this BEFORE inserting, so a create or a start that
    /// would take the store past [`MAX_STORE_BYTES`] is refused with the operation's
    /// own modelled limit error instead of growing it.
    pub fn has_room_for(&self, bytes: usize) -> bool {
        self.retained_bytes.saturating_add(bytes) <= MAX_STORE_BYTES
    }

    /// Store a newly created state machine under its own ARN, counting what it
    /// retains.
    pub fn insert_state_machine(&mut self, mut machine: StateMachine) {
        machine.retained_bytes = machine.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(machine.retained_bytes);
        self.state_machines.insert(machine.arn.clone(), machine);
    }

    /// Store a newly created activity under its own ARN, counting what it retains.
    pub fn insert_activity(&mut self, mut activity: Activity) {
        activity.retained_bytes = activity.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(activity.retained_bytes);
        self.activities.insert(activity.arn.clone(), activity);
    }

    /// Remove the activity `arn` names, giving back what its record retained, and
    /// report the record that was removed.
    ///
    /// `None` when the store held no activity under that ARN, which `DeleteActivity`
    /// — whose model declares no does-not-exist error — treats as the state the call
    /// asked for rather than as a failure.
    pub fn remove_activity(&mut self, arn: &str) -> Option<Activity> {
        let activity = self.activities.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(activity.retained_bytes);
        Some(activity)
    }

    /// The activities of one region, in ARN order.
    ///
    /// A regional endpoint reports only its own region's resources, and every
    /// activity ARN begins with [`activity_arn_prefix`] for its region, so the
    /// `BTreeMap` holds one region's records contiguously and this is a range scan
    /// rather than a filter over the whole store — exactly as
    /// [`Self::state_machines_in_region`] reads the other ARN-keyed resource.
    pub fn activities_in_region(&self, region: &str) -> Vec<&Activity> {
        let prefix = activity_arn_prefix(region);
        self.activities
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, activity)| activity)
            .collect()
    }

    /// Why the store cannot take one more activity of `bytes`, or `None` when it
    /// can. The caps are on the WHOLE store, like every other cap here.
    pub fn activity_rejection(&self, bytes: usize) -> Option<String> {
        if self.activities.len() >= MAX_ACTIVITIES {
            return Some(format!(
                "this emulator already holds the maximum of {MAX_ACTIVITIES} activities"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this activity would take the store past the {MAX_STORE_BYTES} bytes this \
                 emulator retains"
            ));
        }
        None
    }

    /// The tags recorded under a taggable ARN — an EMPTY list when none are.
    ///
    /// This is a read of the tag set, not a resolution of the resource: a tagging
    /// read reports "an array of tags associated with the resource", and the array
    /// of an ARN nothing is tagged under is empty, whether the resource carries no
    /// tags or the store holds no resource under that ARN at all. The write path
    /// ([`Self::merge_tags`]) is where a resource has to exist.
    ///
    /// Both taggable resource types answer here: a tagging call takes "the ARN for
    /// the Step Functions state machine or activity", and one ARN names at most one
    /// of them, since the resource type is a field of the ARN itself.
    pub fn tags_of(&self, arn: &str) -> Vec<Tag> {
        self.tags_at(arn).cloned().unwrap_or_default()
    }

    /// The tag set stored under `arn`, whichever taggable resource type holds it, or
    /// `None` when the store holds no resource under that ARN.
    fn tags_at(&self, arn: &str) -> Option<&Vec<Tag>> {
        if let Some(machine) = self.state_machines.get(arn) {
            return Some(&machine.tags);
        }
        self.activities.get(arn).map(|activity| &activity.tags)
    }

    /// The tag set and the record measurement of the taggable resource `arn` names,
    /// for the two writes below, so each of them states the merge rule ONCE rather
    /// than once per resource type.
    fn taggable_record_mut(&mut self, arn: &str) -> Option<(&mut Vec<Tag>, &mut usize)> {
        if let Some(machine) = self.state_machines.get_mut(arn) {
            return Some((&mut machine.tags, &mut machine.retained_bytes));
        }
        self.activities
            .get_mut(arn)
            .map(|activity| (&mut activity.tags, &mut activity.retained_bytes))
    }

    /// Merge `incoming` into the tags of the taggable resource `arn` names.
    ///
    /// A resource's tags are a MAP keyed by tag key: a key the resource already
    /// carries takes the incoming value in place, and a key it does not carry is
    /// appended, so re-tagging one key never adds a second entry for it and the
    /// order the keys were first written in is the same for every later reader.
    /// Nothing is written when the merge is rejected, so a refused call leaves the
    /// resource exactly as it was.
    ///
    /// The WORK is bounded before any of it is done: `incoming` is caller-sized and
    /// the merge scans the set per entry, so a list longer than [`MAX_TAGS`] — which
    /// could only ever be refused — is refused up front, leaving at most
    /// `MAX_TAGS` iterations over at most `2 * MAX_TAGS` entries however long the
    /// request's list is.
    pub fn merge_tags(&mut self, arn: &str, incoming: &[Tag]) -> Result<(), TagWriteRejection> {
        let Some(current) = self.tags_at(arn) else {
            return Err(TagWriteRejection::NotFound);
        };
        if incoming.len() > MAX_TAGS {
            return Err(TagWriteRejection::TooManyTags(incoming.len()));
        }

        let tags_before = tags_bytes(current);
        let mut merged = current.clone();
        for tag in incoming {
            match merged.iter_mut().find(|existing| existing.key == tag.key) {
                Some(existing) => existing.value = tag.value.clone(),
                None => merged.push(tag.clone()),
            }
        }
        if merged.len() > MAX_TAGS {
            return Err(TagWriteRejection::TooManyTags(merged.len()));
        }

        // The tags are caller-sized, so the record's measurement moves by what the
        // write changed and a write that would take the store past
        // [`MAX_STORE_BYTES`] is refused instead of growing it.
        let tags_after = tags_bytes(&merged);
        if tags_after > tags_before && !self.has_room_for(tags_after - tags_before) {
            return Err(TagWriteRejection::NoRoom);
        }

        let Some((tags, retained)) = self.taggable_record_mut(arn) else {
            return Err(TagWriteRejection::NotFound);
        };
        *tags = merged;
        *retained = retained.saturating_sub(tags_before).saturating_add(tags_after);
        self.retained_bytes = self
            .retained_bytes
            .saturating_sub(tags_before)
            .saturating_add(tags_after);
        Ok(())
    }

    /// Remove the tags `keys` names from the taggable resource `arn` names.
    ///
    /// Keyed by tag key, like the merge: a key the resource does not carry is
    /// nothing to remove, which is not a failure, and a key removed twice is
    /// removed once. `false` when the store holds no resource under `arn` —
    /// the modelled `ResourceNotFound` of the write path.
    ///
    /// `keys` is caller-sized, so the WORK is bounded by what the resource holds
    /// rather than by the request: the resource's at most [`MAX_TAGS`] keys are read
    /// into a set, the request's list is walked ONCE against it, and only the keys
    /// that actually match are kept — so the allocation is at most `MAX_TAGS`
    /// entries however long the request's list is.
    pub fn remove_tags(&mut self, arn: &str, keys: &[String]) -> bool {
        let Some(current) = self.tags_at(arn) else {
            return false;
        };
        let present: std::collections::HashSet<&str> =
            current.iter().filter_map(|tag| tag.key.as_deref()).collect();
        let removing: std::collections::HashSet<String> = keys
            .iter()
            .filter(|key| present.contains(key.as_str()))
            .cloned()
            .collect();
        if removing.is_empty() {
            // Nothing the resource carries was named, so there is nothing to write
            // and the record's measurement cannot have moved.
            return true;
        }
        let tags_before = tags_bytes(current);

        let Some((tags, retained)) = self.taggable_record_mut(arn) else {
            return false;
        };
        tags.retain(|tag| tag.key.as_deref().is_none_or(|key| !removing.contains(key)));
        // The record's measurement moves by what the write changed; a removal can
        // only shrink it, so it can never take the store past [`MAX_STORE_BYTES`].
        let tags_after = tags_bytes(tags);
        *retained = retained.saturating_sub(tags_before).saturating_add(tags_after);
        self.retained_bytes = self
            .retained_bytes
            .saturating_sub(tags_before)
            .saturating_add(tags_after);
        true
    }

    /// Publish the CURRENT configuration of the state machine `arn` names as its
    /// next version, and report the version that was stored. `None` when the store
    /// holds no state machine under `arn`.    ///
    /// "If you set the publish parameter of this API action to true, it publishes
    /// version 1 as the first revision of the state machine" — so the number comes
    /// from the state machine's own counter and is never reused, and the version
    /// holds a snapshot of the configuration, not a reference to a record that can
    /// still change.
    ///
    /// The caller admits the version's bytes BEFORE calling (see
    /// [`ComAmazonawsSfnAwsstepfunctionsState::state_machine_rejection`]), so this
    /// only counts them.
    pub fn publish_state_machine_version(
        &mut self,
        arn: &str,
        description: Option<String>,
        now: SystemTime,
    ) -> Option<StateMachineVersion> {
        let machine = self.state_machines.get(arn)?;
        let version = machine.version_counter.saturating_add(1);
        let mut candidate = StateMachineVersion {
            arn: state_machine_version_arn(arn, version),
            state_machine_arn: machine.arn.clone(),
            version,
            name: machine.name.clone(),
            definition: machine.definition.clone(),
            role_arn: machine.role_arn.clone(),
            sm_type: machine.sm_type.clone(),
            creation_date: now,
            logging_configuration: machine.logging_configuration.clone(),
            tracing_configuration: machine.tracing_configuration.clone(),
            encryption_configuration: machine.encryption_configuration.clone(),
            revision_id: machine.revision_id.clone(),
            description,
            retained_bytes: 0,
        };
        candidate.retained_bytes = candidate.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(candidate.retained_bytes);

        let machine = self.state_machines.get_mut(arn)?;
        machine.version_counter = version;
        self.state_machine_versions
            .insert(candidate.arn.clone(), candidate.clone());
        // How many versions the state machine holds has changed, which is what it
        // publishes about them: queued here, at the one place a version enters the
        // store, so an accepted publish moves the count and a refused one leaves it
        // exactly where the last accepted one put it.
        self.observe_version_count(arn, now);
        Some(candidate)
    }

    /// What one more published version of the state machine `arn` names would
    /// retain, so a caller can admit it against [`MAX_STORE_BYTES`] BEFORE asking
    /// for it: a version snapshots the record's caller-sized text, so the record's
    /// own measurement bounds it from above (a version carries no tags).
    pub fn version_reserve(&self, arn: &str) -> usize {
        self.state_machines
            .get(arn)
            .map_or(0, |machine| machine.retained_bytes)
    }

    /// The version of one state machine that snapshots the revision `revision_id`
    /// names (`None` for a state machine that has never been updated), if one was
    /// ever published from it.
    ///
    /// This is what makes publishing idempotent: "It doesn't create a duplicate
    /// state machine version if it already exists for the current revision. Step
    /// Functions bases PublishStateMachineVersion's idempotency check on the
    /// stateMachineArn, name, and revisionId parameters" — the state machine (which
    /// its ARN and name identify) plus the revision, and nothing else, so the
    /// description a repeat carries does not make it a different request. The
    /// HIGHEST-numbered match is reported, so the answer is the same whichever order
    /// the store was filled in.
    pub fn version_of_revision(
        &self,
        state_machine_arn: &str,
        revision_id: Option<&str>,
    ) -> Option<&StateMachineVersion> {
        self.versions_of(state_machine_arn)
            .into_iter()
            .rev()
            .find(|version| version.revision_id.as_deref() == revision_id)
    }

    /// How many versions of one state machine the store holds, which
    /// [`MAX_VERSIONS_PER_STATE_MACHINE`] bounds.
    pub fn version_count(&self, state_machine_arn: &str) -> usize {
        self.versions_unordered(state_machine_arn).len()
    }

    /// The published versions of one state machine, in version order.
    pub fn versions_of(&self, state_machine_arn: &str) -> Vec<&StateMachineVersion> {
        let mut versions = self.versions_unordered(state_machine_arn);
        versions.sort_by_key(|version| version.version);
        versions
    }

    /// The published versions of one state machine, NEWEST FIRST.
    ///
    /// From `ListStateMachineVersions`' own documentation: "The results are sorted in
    /// descending order of the version creation time." The comparator is keyed on
    /// that documented sort key — the creation time — rather than on the version
    /// order [`ComAmazonawsSfnAwsstepfunctionsState::versions_of`] reversed, so the
    /// order still follows the clock if a version is ever stored out of number
    /// order; two versions published inside one clock tick carry the same creation
    /// time, so the version number — which only ever increases
    /// ([`ComAmazonawsSfnAwsstepfunctionsState::publish_state_machine_version`]) —
    /// breaks the tie, keeping the order total so a page token means the same thing
    /// on the next call.
    pub fn versions_newest_first(&self, state_machine_arn: &str) -> Vec<&StateMachineVersion> {
        let mut versions = self.versions_unordered(state_machine_arn);
        versions.sort_by(|left, right| {
            right
                .creation_date
                .cmp(&left.creation_date)
                .then_with(|| right.version.cmp(&left.version))
        });
        versions
    }

    /// Remove one published version, uncounting what it retained.
    ///
    /// A version is an immutable snapshot keyed by its own qualified ARN, so this
    /// touches nothing else: "deleting a state machine version won't terminate its
    /// in-progress executions", and the state machine's version counter only ever
    /// increases, so the number this frees is never handed out again.
    pub fn remove_state_machine_version(
        &mut self,
        version_arn: &str,
    ) -> Option<StateMachineVersion> {
        let version = self.state_machine_versions.remove(version_arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(version.retained_bytes);
        // One fewer version is held, which the count reports for the same reason the
        // publish does.
        self.observe_version_count(&version.state_machine_arn, now());
        Some(version)
    }

    /// The published versions of one state machine, in no particular order.
    ///
    /// Every version ARN of one state machine begins with its ARN and a colon, so
    /// this is a range scan over the contiguous block rather than a filter over
    /// every version in the store. It is what both ordered accessors above sort, so
    /// each of them sorts the scan exactly once.
    fn versions_unordered(&self, state_machine_arn: &str) -> Vec<&StateMachineVersion> {
        let prefix = format!("{state_machine_arn}:");
        self.state_machine_versions
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, version)| version)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Aliases
    // -----------------------------------------------------------------------

    /// Store a newly created state machine alias under its own ARN, counting what it
    /// retains and stamping the create's place in the order the store accepted them.
    pub fn insert_state_machine_alias(&mut self, mut alias: StateMachineAlias) {
        self.alias_sequence = self.alias_sequence.saturating_add(1);
        alias.creation_sequence = self.alias_sequence;
        alias.retained_bytes = alias.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(alias.retained_bytes);
        let state_machine_arn = alias.state_machine_arn.clone();
        let created = alias.creation_date;
        self.state_machine_aliases.insert(alias.arn.clone(), alias);
        // How many aliases the state machine holds has changed; see
        // [`ComAmazonawsSfnAwsstepfunctionsState::observe_alias_count`].
        self.observe_alias_count(&state_machine_arn, created);
    }

    /// Apply an `UpdateStateMachineAlias` to the alias `arn` names, and report the
    /// instant it reports as its `updateDate`.
    ///
    /// The record is mutated IN PLACE: an update "modif[ies] its description or
    /// routingConfiguration" and mints nothing, so the alias keeps its ARN, its
    /// creation date and its place in the order the store accepted the creates (see
    /// [`StateMachineAlias::creation_sequence`]). Only the members the request named
    /// are written — `None` leaves what the alias holds.
    ///
    /// "UpdateStateMachineAlias is an idempotent API [...] Requests with the same
    /// parameters return an idempotent response": a request that would write what the
    /// record already holds answers the `updateDate` it already reports and changes
    /// nothing, so a repeat does not keep moving that instant.
    pub fn update_state_machine_alias(
        &mut self,
        arn: &str,
        description: Option<String>,
        routing_configuration: Option<Vec<RoutingConfigurationListItem>>,
        now: SystemTime,
    ) -> Result<SystemTime, AliasUpdateRejection> {
        let Some(alias) = self.state_machine_aliases.get(arn) else {
            return Err(AliasUpdateRejection::NotFound);
        };
        let before = alias.retained_bytes;
        let mut candidate = alias.clone();
        if let Some(description) = description {
            candidate.description = Some(description);
        }
        if let Some(routing_configuration) = routing_configuration {
            candidate.routing_configuration = routing_configuration;
        }
        if candidate.description == alias.description
            && same_routing(
                &candidate.routing_configuration,
                &alias.routing_configuration,
            )
        {
            return Ok(alias.update_date);
        }
        // "The date the state machine alias was last updated" moves only when this
        // call really updated it.
        candidate.update_date = now;
        let after = candidate.measure();
        if after > before && !self.has_room_for(after - before) {
            return Err(AliasUpdateRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.state_machine_aliases
            .insert(arn.to_string(), candidate);
        Ok(now)
    }

    /// Drop the alias `arn` names, giving back the bytes its record held, and report
    /// it. `None` when the store holds no alias under that ARN.
    ///
    /// "When you delete a state machine alias, Step Functions doesn't delete the state
    /// machine versions that alias references": an alias is a pointer, so this touches
    /// nothing but the pointer. The store's creation sequence only ever increases, so
    /// the place this frees is never handed out again and the aliases that remain keep
    /// their relative order.
    pub fn remove_state_machine_alias(&mut self, arn: &str) -> Option<StateMachineAlias> {
        let alias = self.state_machine_aliases.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(alias.retained_bytes);
        // One fewer alias is held, which the count reports for the same reason the
        // create does.
        self.observe_alias_count(&alias.state_machine_arn, now());
        Some(alias)
    }

    /// What the modelled `ResourceNotFound` says when an alias MUTATION resolves no
    /// alias under `alias_arn`, worded for WHICH resource the ARN resolved nothing
    /// for.
    ///
    /// An alias ARN names two resources — the state machine it qualifies, and the
    /// alias name inside it ([`state_machine_alias_arn`]) — so resolving one is two
    /// steps, and the refusal states the step that failed. A state machine this store
    /// does not hold resolves nothing at all, and the mutation reports the ARN the
    /// request named, exactly as the alias's own readers do
    /// ([`resource_not_found_message`]): that ARN is what identifies the alias left
    /// dangling by a state machine that was deleted under it. A state machine that IS
    /// held and carries no such alias is the fixed sentence
    /// ([`RESOURCE_DOES_NOT_EXIST`]), which is what the alias itself being gone is
    /// RECORDED answering.
    pub fn missing_alias_message(&self, alias_arn: &str, state_machine_arn: &str) -> String {
        if self.state_machines.contains_key(state_machine_arn) {
            RESOURCE_DOES_NOT_EXIST.to_string()
        } else {
            resource_not_found_message(alias_arn)
        }
    }

    /// The aliases of one state machine, in ARN order — which is alias-name order,
    /// since every one of those ARNs differs only in the name.
    ///
    /// Every alias ARN of one state machine begins with its ARN and a colon, so this
    /// is a range scan over a contiguous block rather than a filter over every alias
    /// in the store, exactly as [`Self::versions_unordered`] is.
    pub fn aliases_of(&self, state_machine_arn: &str) -> Vec<&StateMachineAlias> {
        let prefix = format!("{state_machine_arn}:");
        self.state_machine_aliases
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, alias)| alias)
            .collect()
    }

    /// The aliases of one state machine, in the order they were CREATED — oldest
    /// first.
    ///
    /// This is the order a client observes from `ListStateMachineAliases`: the service
    /// presents one state machine's aliases in the order the creates were accepted,
    /// and that relative order does not move when another alias of the same state
    /// machine is created, updated or deleted. The order is keyed on
    /// [`StateMachineAlias::creation_sequence`], which no two aliases share, so it is
    /// total and two listings of one unchanged state machine agree — which is what a
    /// paged consumer reconciling positions against its own ledger needs.
    pub fn aliases_in_creation_order(&self, state_machine_arn: &str) -> Vec<&StateMachineAlias> {
        let mut aliases = self.aliases_of(state_machine_arn);
        aliases.sort_by_key(|alias| alias.creation_sequence);
        aliases
    }

    /// The aliases of `state_machine_arn` whose routing configuration names the
    /// version `version_arn`, in the order they were created.
    pub fn aliases_routing_to(
        &self,
        state_machine_arn: &str,
        version_arn: &str,
    ) -> Vec<&StateMachineAlias> {
        self.aliases_in_creation_order(state_machine_arn)
            .into_iter()
            .filter(|alias| alias.routes_to(version_arn))
            .collect()
    }

    /// Why the store cannot take one more alias of `bytes` for `state_machine_arn`,
    /// or `None` when it can — the modelled `ServiceQuotaExceededException`.
    pub fn alias_rejection(&self, state_machine_arn: &str, bytes: usize) -> Option<String> {
        let held = self.aliases_of(state_machine_arn).len();
        if held >= MAX_ALIASES_PER_STATE_MACHINE {
            return Some(format!(
                "state machine {state_machine_arn} already holds the maximum of \
                 {MAX_ALIASES_PER_STATE_MACHINE} aliases"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this state machine alias would take the store past the {MAX_STORE_BYTES} bytes \
                 this emulator retains"
            ));
        }
        None
    }

    /// The state machine every version of `routing` belongs to, or why that routing
    /// configuration cannot be pointed at.
    ///
    /// The ONE place the routing-configuration rules live, so every operation that
    /// takes one judges it identically: [`routing_shape`] judges what the REQUEST
    /// carries — the ARNs, the one state machine they must agree on and the weights —
    /// and [`Self::routing_versions_held`] what the store holds under it. The ARNs are
    /// therefore judged BEFORE the store is consulted, like every other ARN here, so a
    /// defect in an ARN is never reported as a version that is absent.
    ///
    /// This is the reader of an operation that resolves the alias's state machine FROM
    /// the configuration (`CreateStateMachineAlias`, which takes no `stateMachineArn`);
    /// one repointing an alias that already belongs to a state machine resolves the
    /// configuration under THAT state machine — see
    /// [`Self::read_routing_configuration_of`].
    pub fn read_routing_configuration(
        &self,
        routing: &[RoutingConfigurationListItem],
        endpoint_region: &str,
    ) -> Result<String, RoutingRejection> {
        let owner = routing_shape(routing, endpoint_region)?;
        self.routing_versions_held(routing, &owner)?;
        Ok(owner)
    }

    /// `routing` judged as the NEW configuration of the alias that belongs to
    /// `state_machine_arn`.
    ///
    /// An alias is a pointer into ONE state machine's versions, so a repoint resolves
    /// every entry UNDER the state machine the alias belongs to and nowhere else: an
    /// entry naming a version of any other state machine resolves nothing there,
    /// whether or not that version exists elsewhere, which makes it a resource that
    /// cannot be found ([`AliasRoutingRejection::OutsideStateMachine`]) and not a
    /// request whose shape is wrong. Every other rule is the shared one above.
    pub fn read_routing_configuration_of(
        &self,
        routing: &[RoutingConfigurationListItem],
        endpoint_region: &str,
        state_machine_arn: &str,
    ) -> Result<(), AliasRoutingRejection> {
        let owner =
            routing_shape(routing, endpoint_region).map_err(AliasRoutingRejection::Shared)?;
        if owner != state_machine_arn {
            // WHICH entry resolved nothing: the first that is not a version of the
            // alias's own state machine. The shape rule has already refused a
            // configuration whose entries name two state machines, so every entry
            // here names the same foreign one.
            let unresolved = routing
                .iter()
                .map(|item| item.state_machine_version_arn.clone())
                .find(|arn| version_state_machine(arn) != state_machine_arn)
                .unwrap_or_default();
            return Err(AliasRoutingRejection::OutsideStateMachine(unresolved));
        }
        self.routing_versions_held(routing, &owner)
            .map_err(AliasRoutingRejection::Shared)
    }

    /// The STORE's part of the routing rules: every entry names a version the store
    /// holds, and the state machine `owner` those versions belong to is not being
    /// deleted.
    fn routing_versions_held(
        &self,
        routing: &[RoutingConfigurationListItem],
        owner: &str,
    ) -> Result<(), RoutingRejection> {
        for item in routing {
            if !self
                .state_machine_versions
                .contains_key(&item.state_machine_version_arn)
            {
                return Err(RoutingRejection::VersionNotFound(
                    item.state_machine_version_arn.clone(),
                ));
            }
        }
        // A version is removed with the state machine it belongs to, so by here the
        // state machine is stored; one whose deletion has begun can take no more
        // configuration.
        if self
            .state_machines
            .get(owner)
            .is_some_and(StateMachine::is_deleting)
        {
            return Err(RoutingRejection::Deleting(owner.to_string()));
        }
        Ok(())
    }

    /// What the state machine ARN `arn` refers to, or `None` when the store holds no
    /// record it names.
    ///
    /// The ONE place a qualified state machine ARN is resolved, so every operation
    /// that takes one reads it identically: a qualifier of nothing but digits is a
    /// VERSION ([`qualifier_version`]) and any other qualifier is an ALIAS name, each
    /// stored under the qualified ARN itself, and both resolve to the state machine
    /// that owns them. A reference the store holds nothing for is a resource that is
    /// absent, which is what the caller's own not-found error reports; the ARN itself
    /// is judged earlier, by [`read_state_machine_arn`].
    pub fn read_state_machine_reference(
        &self,
        arn: &str,
        parsed: &StateMachineArn,
    ) -> Option<StateMachineReference> {
        // A map-run LABELED ARN names a Distributed Map state inside a state
        // machine, so its trailing field is not a qualifier and nothing here
        // answers for it.
        let qualifier = parsed
            .qualifier
            .as_deref()
            .filter(|_| parsed.label().is_none());
        let Some(qualifier) = qualifier else {
            let machine = self.state_machines.get(arn)?;
            return Some(StateMachineReference {
                state_machine_arn: machine.arn.clone(),
                version_arn: None,
                alias_arn: None,
            });
        };
        if qualifier_version(qualifier).is_some() {
            let version = self.state_machine_versions.get(arn)?;
            return Some(StateMachineReference {
                state_machine_arn: version.state_machine_arn.clone(),
                version_arn: Some(version.arn.clone()),
                alias_arn: None,
            });
        }
        let alias = self.state_machine_aliases.get(arn)?;
        Some(StateMachineReference {
            state_machine_arn: alias.state_machine_arn.clone(),
            version_arn: None,
            alias_arn: Some(alias.arn.clone()),
        })
    }

    /// The version one alias routes an execution to, drawn in proportion to the
    /// weights of its routing configuration, or `None` when the store holds no
    /// such alias.
    ///
    /// "You can also map an alias to split StartExecution requests between two
    /// versions of a state machine. [...] Step Functions randomly chooses which
    /// version runs a given execution based on the percentage you specify" — so the
    /// draw is made once per start, and an entry's share of the total weight is its
    /// chance of being drawn. An entry at weight 0 is therefore never drawn.
    pub fn routed_version(&mut self, alias_arn: &str) -> Option<String> {
        let routing: Vec<(String, u64)> = self
            .state_machine_aliases
            .get(alias_arn)?
            .routing_configuration
            .iter()
            .map(|item| {
                (
                    item.state_machine_version_arn.clone(),
                    u64::try_from(item.weight).unwrap_or(0),
                )
            })
            .collect();
        let total: u64 = routing.iter().map(|(_, weight)| weight).sum();
        // "The sum of the weights in the routing configuration must be equal to
        // 100", so the span below is positive; a configuration that somehow adds up
        // to nothing still routes, to the first version it names.
        let draw = self.draw_below(total);
        let mut reached = 0_u64;
        routing
            .iter()
            .find_map(|(version_arn, weight)| {
                reached = reached.saturating_add(*weight);
                (draw < reached).then(|| version_arn.clone())
            })
            .or_else(|| routing.first().map(|(version_arn, _)| version_arn.clone()))
    }

    /// The run the state machine ARN `arn` names, or `None` when the store holds no
    /// record it refers to.
    ///
    /// This is what both start operations resolve their `stateMachineArn` through: an
    /// unqualified ARN runs the state machine's CURRENT configuration, a version ARN
    /// runs that version's snapshot, and an alias ARN runs the version its routing
    /// configuration draws ([`Self::routed_version`]) — and the version and alias the
    /// ARN named are carried onto the execution, which is what every execution report
    /// attributes it by.
    pub fn read_run_target(&mut self, arn: &str, parsed: &StateMachineArn) -> Option<RunTarget> {
        let reference = self.read_state_machine_reference(arn, parsed)?;
        let alias_arn = reference.alias_arn.clone();
        let version_arn = match alias_arn.as_deref() {
            Some(alias_arn) => self.routed_version(alias_arn),
            None => reference.version_arn.clone(),
        };
        if alias_arn.is_some() || version_arn.is_some() {
            // A version is immutable, so what it holds IS what this run executes.
            let version = self.state_machine_versions.get(version_arn.as_deref()?)?;
            return Some(RunTarget {
                state_machine_arn: version.state_machine_arn.clone(),
                name: version.name.clone(),
                sm_type: version.sm_type.clone(),
                configuration: RanConfiguration::of_version(version),
                version_arn: Some(version.arn.clone()),
                alias_arn,
            });
        }
        let machine = self.state_machines.get(&reference.state_machine_arn)?;
        Some(RunTarget {
            state_machine_arn: machine.arn.clone(),
            name: machine.name.clone(),
            sm_type: machine.sm_type.clone(),
            configuration: RanConfiguration::of_state_machine(machine),
            version_arn: None,
            alias_arn: None,
        })
    }

    /// Apply `update` to the state machine `arn` names: every member the update
    /// carries replaces the stored one, and every member it leaves out keeps the
    /// value the state machine holds.
    ///
    /// "Updates an existing state machine by modifying its definition, roleArn,
    /// loggingConfiguration, or EncryptionConfiguration" — so this is the write that
    /// makes the state machine's CURRENT configuration differ from the one it was
    /// created with, which is what a later `DescribeStateMachine` reports and what
    /// `CreateStateMachine`'s idempotency check compares a repeat against.
    ///
    /// The definition is caller-sized, so the record's measurement moves by what the
    /// write changed and an update that would take the store past
    /// [`MAX_STORE_BYTES`] is refused instead of growing it, leaving the state
    /// machine exactly as it was.
    pub fn apply_state_machine_update(
        &mut self,
        arn: &str,
        update: StateMachineUpdate,
    ) -> Result<(), StateMachineUpdateRejection> {
        let Some(machine) = self.state_machines.get(arn) else {
            return Err(StateMachineUpdateRejection::NotFound);
        };
        if machine.is_deleting() {
            return Err(StateMachineUpdateRejection::Deleting);
        }

        let mut updated = machine.clone();
        if let Some(definition) = update.definition {
            updated.definition = definition;
        }
        if let Some(role_arn) = update.role_arn {
            updated.role_arn = role_arn;
        }
        if update.logging_configuration.is_some() {
            updated.logging_configuration = update.logging_configuration;
        }
        if update.tracing_configuration.is_some() {
            updated.tracing_configuration = update.tracing_configuration;
        }
        if update.encryption_configuration.is_some() {
            updated.encryption_configuration = update.encryption_configuration;
        }
        updated.revision_id = Some(update.revision_id);
        updated.update_date = update.now;

        let before = machine.retained_bytes;
        let after = updated.measure();
        if after > before && !self.has_room_for(after - before) {
            return Err(StateMachineUpdateRejection::NoRoom);
        }
        updated.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.state_machines.insert(arn.to_string(), updated);
        Ok(())
    }

    /// Begin deleting the state machine `arn` names: it reports `DELETING` from now
    /// and is removed once [`DELETE_SETTLE_DELAY`] has passed. `false` when nothing
    /// matched, which `DeleteStateMachine` answers as a success all the same.
    pub fn begin_delete_state_machine(&mut self, arn: &str, now: SystemTime) -> bool {
        let Some(machine) = self.state_machines.get_mut(arn) else {
            return false;
        };
        if machine.is_deleting() {
            return true;
        }
        machine.status = STATE_MACHINE_STATUS_DELETING.to_string();
        machine.delete_due_at = Some(now + DELETE_SETTLE_DELAY);
        self.deleting_state_machines.push(arn.to_string());
        true
    }

    /// Remove a state machine, its versions, its aliases and its executions.
    ///
    /// The executions go with it, so no execution ARN is left naming a state machine
    /// that no longer exists, and so do its published versions: "when you delete a
    /// state machine, its versions are deleted too", and a version ARN that outlived
    /// its state machine would answer for a resource that is gone. An alias points at
    /// versions of this one state machine, so it cannot outlive them either.
    pub fn remove_state_machine(&mut self, arn: &str) -> Option<StateMachine> {
        let machine = self.state_machines.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(machine.retained_bytes);
        let version_arns: Vec<String> = self
            .versions_of(arn)
            .into_iter()
            .map(|version| version.arn.clone())
            .collect();
        for version_arn in version_arns {
            if let Some(version) = self.state_machine_versions.remove(&version_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(version.retained_bytes);
            }
        }
        let alias_arns: Vec<String> = self
            .aliases_of(arn)
            .into_iter()
            .map(|alias| alias.arn.clone())
            .collect();
        for alias_arn in alias_arns {
            if let Some(alias) = self.state_machine_aliases.remove(&alias_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(alias.retained_bytes);
            }
        }
        for execution_arn in self.executions_by_sm.remove(arn).unwrap_or_default() {
            if let Some(execution) = self.executions.remove(&execution_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(execution.retained_bytes);
            }
            // An outstanding activity task names the execution it belongs to, so it
            // goes with it: no token is left for a worker to answer against an
            // execution that no longer exists.
            self.discard_tasks_of_executions(std::slice::from_ref(&execution_arn));
            // A Map Run names the execution that started it, for the same reason.
            self.discard_map_runs_of(&execution_arn);
        }
        Some(machine)
    }

    /// The state machines of one region, in ARN order.
    ///
    /// A regional endpoint reports only its own region's resources, and every state
    /// machine ARN begins with [`state_machine_arn_prefix`] for its region, so the
    /// `BTreeMap` holds one region's records contiguously and this is a range scan
    /// rather than a filter over the whole store.
    pub fn state_machines_in_region(&self, region: &str) -> Vec<&StateMachine> {
        let prefix = state_machine_arn_prefix(region);
        self.state_machines
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, machine)| machine)
            .collect()
    }

    /// Why the store cannot take one more state machine of `bytes`, or `None` when
    /// it can. The caps are on the WHOLE store, so no value a caller chooses — the
    /// signed region included — can mint a fresh quota.
    pub fn state_machine_rejection(&self, bytes: usize) -> Option<String> {
        if self.state_machines.len() >= MAX_STATE_MACHINES {
            return Some(format!(
                "this emulator already holds the maximum of {MAX_STATE_MACHINES} state machines"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this state machine would take the store past the {MAX_STORE_BYTES} bytes this \
                 emulator retains"
            ));
        }
        None
    }

    /// What a `StartExecution` whose execution ARN the store ALREADY holds answers,
    /// or `None` when the ARN is free.
    ///
    /// "StartExecution is idempotent for STANDARD workflows. [...] if you call
    /// StartExecution with the same name and input as a running execution, the call
    /// succeeds and return the same response as the original request. If the
    /// execution is closed or if the input is different, it returns a 400
    /// ExecutionAlreadyExists error." It "isn't idempotent for EXPRESS workflows".
    ///
    /// The decision lives HERE because `StartExecution` takes it twice: once before
    /// the definition is interpreted, so a repeat of an idempotent start does not
    /// make a `Task` state's service integration calls a second time, and once with
    /// the admitting lock held, because the name could have been taken while the
    /// walk ran.
    pub fn started_already(
        &self,
        execution_arn: &str,
        standard: bool,
        input: &str,
    ) -> Option<StartedAlready> {
        let existing = self.executions.get(execution_arn)?;
        if standard && !existing.is_terminal() && existing.input == input {
            return Some(StartedAlready::Idempotent {
                start_date: existing.start_date,
            });
        }
        Some(StartedAlready::NameTaken)
    }

    /// Why the store cannot take one more execution of `bytes` for
    /// `state_machine_arn`, or `None` when it can.
    pub fn execution_rejection(&self, state_machine_arn: &str, bytes: usize) -> Option<String> {
        if self.execution_count(state_machine_arn) >= MAX_EXECUTIONS_PER_STATE_MACHINE {
            return Some(format!(
                "state machine {state_machine_arn} already holds the maximum of \
                 {MAX_EXECUTIONS_PER_STATE_MACHINE} executions"
            ));
        }
        if self.executions.len() >= MAX_EXECUTIONS {
            return Some(format!(
                "this emulator already holds the maximum of {MAX_EXECUTIONS} executions"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this execution would take the store past the {MAX_STORE_BYTES} bytes this \
                 emulator retains"
            ));
        }
        None
    }

    /// Store a newly started execution and index it under its state machine,
    /// counting what it retains.
    ///
    /// The ARN must not already be stored: `StartExecution` refuses a name already in
    /// use before reaching here, so there is no duplicate to guard against and
    /// `executions_by_sm` is appended to unconditionally.
    pub fn insert_execution(&mut self, mut execution: Execution) {
        execution.retained_bytes = execution.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(execution.retained_bytes);
        if execution.pending.is_some() {
            self.pending_executions.push(execution.arn.clone());
        }
        self.executions_by_sm
            .entry(execution.state_machine_arn.clone())
            .or_default()
            .push(execution.arn.clone());
        let arn = execution.arn.clone();
        self.executions.insert(execution.arn.clone(), execution);
        // What the new execution publishes about itself: it has STARTED, and — for a
        // record that arrives terminal, which a synchronous Express run does — what it
        // publishes about having ended. Queued here, in the one place a started
        // execution enters the store, and written out by the call that queued it.
        self.observe_execution_started(&arn);
    }

    // -----------------------------------------------------------------------
    // Map runs
    // -----------------------------------------------------------------------

    /// Store the Map Runs one attempt of `execution_arn`'s definition started, and
    /// index them under that execution.
    ///
    /// Called once the execution has been ADMITTED, against bytes that already
    /// include these records' ([`PlannedMapRun::measure`]).
    ///
    /// One attempt's Map Runs are reported in the order it entered their `Map` states,
    /// and the ARN each one is recorded under was decided by the WALK as that state was
    /// entered ([`PlannedMapRun::arn`]): a walk CONTINUING an attempt is handed the Map
    /// Run that attempt already started — matched by the state's own label and by which
    /// entry into it the walk is making ([`asl::StartedMapRun::arn`],
    /// [`super::map_runs::resumed`]) — and mints one for every other state, because the
    /// results a run's `ResultWriter` writes and the output its state puts out name the
    /// Map Run before this call can record it. A run the walk re-entered therefore
    /// MOVES, keeping the ARN a client may be holding and counting the redrive.
    ///
    /// A Map Run an earlier attempt started that this one did not re-enter STAYS a Map
    /// Run of this execution: "Step Functions preserves the results and execution
    /// history of the successful steps", and a Map Run that ran is a resource that
    /// happened, so `ListMapRuns` goes on reporting it in the order its state was
    /// entered.
    pub fn record_map_runs(
        &mut self,
        execution_arn: &str,
        state_machine_arn: &str,
        planned: Vec<PlannedMapRun>,
    ) {
        // An attempt that entered no Distributed Map state reports nothing, and says
        // nothing about the records an earlier attempt left.
        if planned.is_empty() {
            return;
        }
        // The Map Runs this execution already has, in the order their states were
        // entered; one whose record the store no longer holds is not one of them.
        let mut recorded: Vec<String> = self
            .map_runs_by_execution
            .get(execution_arn)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|arn| self.map_runs.contains_key(arn))
            .collect();
        for plan in planned {
            let arn = plan.arn.clone();
            // A run this attempt re-entered keeps its place in the list; one no earlier
            // attempt started is appended after them.
            if !recorded.contains(&arn) {
                recorded.push(arn.clone());
            }
            self.put_map_run(arn, execution_arn, state_machine_arn, plan);
        }
        self.map_runs_by_execution
            .insert(execution_arn.to_string(), recorded);
    }

    /// Record the Map Runs a CONTINUATION of `execution_arn` started — the states a
    /// worker's reply to an activity task carried the walk on through — keeping the
    /// ones the attempt's earlier states already started.
    ///
    /// A reply continues ONE attempt rather than starting another, so the records
    /// already listed under this execution stand: they belong to states this walk
    /// passed before the task, which happened once and are not re-entered. Each Map
    /// Run here is therefore a position no earlier call reached, appended after them
    /// under the ARN the walk minted for it.
    pub fn append_map_runs(
        &mut self,
        execution_arn: &str,
        state_machine_arn: &str,
        planned: Vec<PlannedMapRun>,
    ) {
        if planned.is_empty() {
            return;
        }
        let mut recorded: Vec<String> = self
            .map_runs_by_execution
            .get(execution_arn)
            .cloned()
            .unwrap_or_default();
        for plan in planned {
            let arn = plan.arn.clone();
            self.put_map_run(arn.clone(), execution_arn, state_machine_arn, plan);
            recorded.push(arn);
        }
        self.map_runs_by_execution
            .insert(execution_arn.to_string(), recorded);
    }

    /// Store one Map Run under `arn`, counting what it retains and queueing the
    /// terminal status its items produced for the clock.
    ///
    /// A record already at that ARN is the SAME Map Run running again — which is what
    /// a redrive of its execution does — so it keeps the ARN a client may be holding
    /// and counts the redrive; the bytes it held are given back and the new record's
    /// are counted.
    fn put_map_run(
        &mut self,
        arn: String,
        execution_arn: &str,
        state_machine_arn: &str,
        plan: PlannedMapRun,
    ) {
        let (redrive_count, retained_before) = match self.map_runs.get(&arn) {
            Some(map_run) => (
                map_run.redrive_count.saturating_add(1),
                map_run.retained_bytes,
            ),
            None => (0, 0),
        };
        let mut record = MapRun {
            arn: arn.clone(),
            execution_arn: execution_arn.to_string(),
            state_machine_arn: state_machine_arn.to_string(),
            label: plan.label,
            // The items ran in the walk, but the Map Run reaches its terminal
            // status when the clock says it did, exactly as its execution does:
            // a caller reading straight after the start sees it RUNNING.
            status: MAP_RUN_STATUS_RUNNING.to_string(),
            start_date: plan.start_date,
            stop_date: None,
            pending: Some(PendingMapRunOutcome {
                due_at: plan.due_at,
                status: plan.status,
            }),
            max_concurrency: plan.max_concurrency,
            tolerated_failure_count: plan.tolerated_failure_count,
            tolerated_failure_percentage: plan.tolerated_failure_percentage,
            total_items: plan.total_items,
            // Nothing this Map Run iterated is counted yet: each item is counted from
            // the instant its own iteration finished, which the clock has still to
            // reach, exactly as the Map Run's status is.
            succeeded_items: 0,
            failed_items: 0,
            items: plan.items,
            redrive_count,
            // "If you have not yet redriven [...] the redriveDate is null": the
            // instant the re-run that produced this record began.
            redrive_date: (redrive_count > 0).then_some(plan.start_date),
            retained_bytes: 0,
        };
        record.retained_bytes = record.measure();
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(record.retained_bytes)
            .saturating_sub(retained_before);
        self.pending_map_runs.retain(|pending| pending != &arn);
        self.pending_map_runs.push(arn.clone());
        self.map_runs.insert(arn.clone(), record);
        // The children of the attempt this record is REPLACING belong to a run of
        // this `Map` state that happened before: a redrive re-ran the state, so the
        // executions it started then are gone and the ones it started now take their
        // place under the same Map Run — all but the ones this attempt KEPT, whose
        // items it did not run again ([`PlannedChildExecution::preserved`]) and whose
        // records therefore stay exactly as they are.
        let kept: Vec<String> = plan
            .children
            .iter()
            .filter(|child| child.preserved)
            .map(|child| child.arn.clone())
            .collect();
        self.discard_child_executions_of_except(&arn, &kept);
        self.record_child_executions(&arn, execution_arn, plan.children);
    }

    /// Store the CHILD workflow executions one Map Run's items ran as, and index them
    /// under that Map Run.
    ///
    /// Each one is an execution in its own right, with the input its item was handed
    /// and the terminal status its iteration reached owed to the clock exactly as the
    /// Map Run's own status is — so a caller reading one straight after the start sees
    /// it `RUNNING`. It runs the state machine its PARENT ran, which is the state
    /// machine the Map Run reports, so it holds that parent's configuration snapshot
    /// rather than a copy of it.
    ///
    /// Called with the bytes already admitted ([`PlannedChildExecution::measure`]).
    fn record_child_executions(
        &mut self,
        map_run_arn: &str,
        execution_arn: &str,
        children: Vec<PlannedChildExecution>,
    ) {
        if children.is_empty() {
            return;
        }
        // The parent is stored before its Map Runs are, so what it RAN is in hand;
        // without it there is no state machine for a child to have run and nothing to
        // record.
        let Some(parent) = self.executions.get(execution_arn) else {
            return;
        };
        let ran = std::sync::Arc::clone(&parent.ran);
        let state_machine_arn = parent.state_machine_arn.clone();
        let version_arn = parent.state_machine_version_arn.clone();
        let alias_arn = parent.state_machine_alias_arn.clone();
        let trace_header = parent.trace_header.clone();
        let mut recorded = Vec::with_capacity(children.len());
        for child in children {
            // An execution this attempt KEPT is one it did not start: its record holds
            // the history, the dates and the output of the attempt that did, which a
            // redrive preserves, so it is listed under the Map Run again and written
            // not at all.
            if child.preserved && self.executions.contains_key(&child.arn) {
                recorded.push(child.arn);
                continue;
            }
            let opened = HistoryEvent {
                id: 1,
                previous_event_id: 0,
                event_type: EVENT_EXECUTION_STARTED.to_string(),
                timestamp: child.start_date,
                state_name: None,
                details: EventDetails::default(),
            };
            let terminal = HistoryEvent {
                id: 2,
                // The event after an `ExecutionStarted` has no predecessor — see
                // [`follows_event`].
                previous_event_id: follows_event(opened.id, &opened.event_type),
                event_type: if child.status == EXECUTION_STATUS_SUCCEEDED {
                    EVENT_EXECUTION_SUCCEEDED.to_string()
                } else {
                    EVENT_EXECUTION_FAILED.to_string()
                },
                timestamp: child.due_at,
                state_name: None,
                details: EventDetails::of_outcome(
                    child.output.clone(),
                    child.error.clone(),
                    child.cause.clone(),
                ),
            };
            let record = Execution {
                arn: child.arn.clone(),
                state_machine_arn: state_machine_arn.clone(),
                name: child.name,
                status: EXECUTION_STATUS_RUNNING.to_string(),
                start_date: child.start_date,
                stop_date: None,
                input: child.input,
                output: None,
                error: None,
                cause: None,
                trace_header: trace_header.clone(),
                state_machine_version_arn: version_arn.clone(),
                state_machine_alias_arn: alias_arn.clone(),
                ran: std::sync::Arc::clone(&ran),
                redrive_count: 0,
                redrive_date: None,
                redrive_tokens: Vec::new(),
                history: vec![opened],
                redrive_step: None,
                pending: Some(PendingOutcome {
                    due_at: child.due_at,
                    status: child.status,
                    output: child.output,
                    error: child.error,
                    cause: child.cause,
                    events: vec![terminal],
                }),
                map_run_arn: Some(map_run_arn.to_string()),
                item_count: Some(child.item_count),
                // Measured and counted below, exactly as `insert_execution` does.
                retained_bytes: 0,
                // No read of this child's history has handed a page cursor out yet.
                issued_page_cursors: Vec::new(),
                // Nothing of this child's history has been counted into the series its
                // tasks publish yet.
                observed_events: 0,
            };
            recorded.push(record.arn.clone());
            self.insert_child_execution(record);
        }
        self.executions_by_map_run
            .insert(map_run_arn.to_string(), recorded);
    }

    /// Store one child workflow execution and count what it retains.
    ///
    /// It is indexed under its Map Run by its caller, NOT under its state machine:
    /// `ListExecutions` reports a Map Run's children for that Map Run alone.
    fn insert_child_execution(&mut self, mut execution: Execution) {
        execution.retained_bytes = execution.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(execution.retained_bytes);
        if execution.pending.is_some() {
            self.pending_executions.push(execution.arn.clone());
        }
        self.executions.insert(execution.arn.clone(), execution);
    }

    /// Drop every child workflow execution of the Map Run `map_run_arn` names, so no
    /// execution is left under a Map Run whose items it no longer belongs to.
    pub(super) fn discard_child_executions_of(&mut self, map_run_arn: &str) {
        self.discard_child_executions_of_except(map_run_arn, &[]);
    }

    /// As [`ComAmazonawsSfnAwsstepfunctionsState::discard_child_executions_of`],
    /// keeping the executions `kept` names.
    ///
    /// They are the children a continued attempt of the Map Run KEPT: their items were
    /// not run again, so the records they already have are the ones that go on being
    /// reported ([`PlannedChildExecution::preserved`]).
    pub(super) fn discard_child_executions_of_except(
        &mut self,
        map_run_arn: &str,
        kept: &[String],
    ) {
        for arn in self
            .executions_by_map_run
            .remove(map_run_arn)
            .unwrap_or_default()
        {
            if kept.contains(&arn) {
                continue;
            }
            if let Some(execution) = self.executions.remove(&arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(execution.retained_bytes);
            }
            self.pending_executions.retain(|pending| pending != &arn);
            self.discard_tasks_of_executions(std::slice::from_ref(&arn));
        }
    }

    /// One Map Run's child workflow executions, most recent first.
    ///
    /// The same order `ListExecutions` reports a state machine's executions in
    /// ([`Self::executions_newest_first`]), because it is the same listing: "results
    /// are sorted by time, with the most recent execution first".
    pub fn child_executions_newest_first(&self, map_run_arn: &str) -> Vec<&Execution> {
        let mut children: Vec<&Execution> = self
            .executions_by_map_run
            .get(map_run_arn)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|arn| self.executions.get(arn))
            .collect();
        children.sort_by(|left, right| newest_first(left, right));
        children
    }

    /// Apply `UpdateMapRun`'s configuration to the Map Run `arn` names.
    ///
    /// Only the members the request CARRIED are changed; the others keep what the
    /// `Map` state declared, because the request states a change to the settings it
    /// names and says nothing about the rest.
    ///
    /// What the Map Run's items PRODUCED is not touched, and neither is the status
    /// they left it owed: the iterations are not re-run by a configuration change,
    /// and the execution around the Map Run reached its own outcome through the same
    /// `Map` state — so re-deciding the Map Run's status here alone would leave the
    /// two contradicting each other, with a Map Run reported as succeeded inside an
    /// execution reported as failed at it.
    ///
    /// The thresholds this writes ARE the failure budget the Map Run goes on running
    /// under: [`MapRun::advance`] weighs the failures it counts against what this
    /// record carries at that instant, so a budget a caller raised keeps a Map Run
    /// running over failures the budget it started with would have stopped it on.
    ///
    /// Call [`Self::settle`] first, so a Map Run the clock has already finished is
    /// refused as the completed Map Run it is.
    pub fn update_map_run(
        &mut self,
        arn: &str,
        max_concurrency: Option<i32>,
        tolerated_failure_count: Option<i64>,
        tolerated_failure_percentage: Option<f32>,
    ) -> Result<(), MapRunUpdateRejection> {
        let Some(map_run) = self.map_runs.get_mut(arn) else {
            return Err(MapRunUpdateRejection::NotFound);
        };
        // "Updates an IN-PROGRESS Map Run's configuration": a Map Run that has
        // reached a terminal status has no configuration left to govern it.
        if map_run.is_terminal() {
            return Err(MapRunUpdateRejection::Completed);
        }
        let before = map_run.retained_bytes;
        if let Some(max_concurrency) = max_concurrency {
            // A caller's `0` is the same "no limit" the `Map` state's own default is,
            // so the record reports it the one way a Map Run reports no limit.
            map_run.max_concurrency = reported_max_concurrency(i64::from(max_concurrency));
        }
        if let Some(tolerated_failure_count) = tolerated_failure_count {
            map_run.tolerated_failure_count = tolerated_failure_count;
        }
        if let Some(tolerated_failure_percentage) = tolerated_failure_percentage {
            map_run.tolerated_failure_percentage = tolerated_failure_percentage;
        }
        // The settings a caller sends cost no bytes of their own, but the record is
        // re-measured exactly as every other write to one is, so the store's total
        // never depends on which write path touched it last.
        map_run.retained_bytes = map_run.measure();
        let after = map_run.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        Ok(())
    }

    /// One execution's Map Runs, in the order their `Map` states were entered.
    pub fn map_runs_of(&self, execution_arn: &str) -> Vec<&MapRun> {
        self.map_runs_by_execution
            .get(execution_arn)
            .map(|arns| {
                arns.iter()
                    .filter_map(|arn| self.map_runs.get(arn))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stop every Map Run of `execution_arn` that has not finished, at `now`, and
    /// every child workflow execution of theirs that has not finished either.
    ///
    /// A Map Run runs INSIDE its execution, so an execution that was stopped before
    /// its Map Run's items were due stopped them too: the terminal status the walk
    /// had computed is discarded along with its place in the settle queue, exactly as
    /// the execution's own is.
    pub(super) fn abort_map_runs_of(&mut self, execution_arn: &str, now: SystemTime) {
        let arns = self
            .map_runs_by_execution
            .get(execution_arn)
            .cloned()
            .unwrap_or_default();
        for arn in arns {
            let Some(map_run) = self.map_runs.get_mut(&arn) else {
                continue;
            };
            if map_run.is_terminal() {
                continue;
            }
            let before = map_run.retained_bytes;
            map_run.pending = None;
            // The items the clock had not reached are never going to finish: the
            // execution that was iterating them was stopped.
            map_run.items.clear();
            map_run.status = MAP_RUN_STATUS_ABORTED.to_string();
            map_run.stop_date = Some(now);
            map_run.retained_bytes = map_run.measure();
            let after = map_run.retained_bytes;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
            self.pending_map_runs.retain(|pending| pending != &arn);
            self.abort_child_executions_of(&arn, now);
        }
    }

    /// Stop every child workflow execution of the Map Run `map_run_arn` names that
    /// has not reached a terminal status, at `now`.
    ///
    /// A child runs INSIDE its Map Run, so the outcome the walk computed for one the
    /// clock had not reached is discarded along with its place in the settle queue,
    /// exactly as the Map Run's own status and the execution's around it are.
    fn abort_child_executions_of(&mut self, map_run_arn: &str, now: SystemTime) {
        let arns = self
            .executions_by_map_run
            .get(map_run_arn)
            .cloned()
            .unwrap_or_default();
        for arn in arns {
            let Some(child) = self.executions.get_mut(&arn) else {
                continue;
            };
            if child.stop_date.is_some() {
                continue;
            }
            let before = child.retained_bytes;
            child.pending = None;
            child.status = EXECUTION_STATUS_ABORTED.to_string();
            child.stop_date = Some(now);
            child.push_event(EVENT_EXECUTION_ABORTED, now);
            child.retained_bytes = child.measure();
            let after = child.retained_bytes;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
            self.pending_executions.retain(|pending| pending != &arn);
            // A child workflow execution is an execution in its own right, so the
            // terminal status this abort gives it is published exactly as any other
            // execution's is.
            self.observe_execution_settled(&arn);
        }
    }

    /// Drop every Map Run of `execution_arn`, so no Map Run ARN is left naming one
    /// whose execution no longer exists.
    pub(super) fn discard_map_runs_of(&mut self, execution_arn: &str) {
        for arn in self
            .map_runs_by_execution
            .remove(execution_arn)
            .unwrap_or_default()
        {
            if let Some(map_run) = self.map_runs.remove(&arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(map_run.retained_bytes);
            }
            self.pending_map_runs.retain(|pending| pending != &arn);
            // The child workflow executions it started go with it, for the same
            // reason: no execution ARN is left naming one whose Map Run is gone.
            self.discard_child_executions_of(&arn);
        }
    }

    /// Stop the execution `arn` names, and report the `stopDate` it carries together
    /// with the job it was PARKED at, if any.
    ///
    /// The outcome the interpreter had computed is DISCARDED along with its place in
    /// the settle queue: the execution was stopped before the clock made that outcome
    /// due, so it never reached it. An execution that has already reached a terminal
    /// status is not moved — its own `stopDate` comes back, so two callers that stop
    /// the same execution see the same instant. `None` when nothing matched.
    ///
    /// A job a run-a-job task started is work of ANOTHER service, which this store
    /// cannot reach: the record of it comes back so the operation that stopped the
    /// execution cancels that job across the service boundary, which is what stopping
    /// an execution parked inside such a task does.
    pub fn stop_execution(
        &mut self,
        arn: &str,
        now: SystemTime,
        error: Option<String>,
        cause: Option<String>,
    ) -> Option<Stopped> {
        let execution = self.executions.get_mut(arn)?;
        if let Some(stop_date) = execution.stop_date {
            return Some(Stopped {
                stop_date,
                parked: None,
            });
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_ABORTED.to_string();
        execution.stop_date = Some(now);
        execution.error = error;
        execution.cause = cause;
        // The event carries the error and the cause the STOP reported, so the history
        // keeps them whatever a later attempt of this execution computes.
        execution.push_detailed_event(
            EVENT_EXECUTION_ABORTED,
            now,
            EventDetails::of_failure(execution.error.clone(), execution.cause.clone()),
        );
        // The caller's `error` and `cause` are caller-sized, so the record is
        // re-measured and the store's total moved by the difference. The record
        // already reserved their maxima while it was running (see
        // [`STOP_RESERVE_BYTES`]), so this difference can only ever be a shrink and
        // the stop path cannot take the store past [`MAX_STORE_BYTES`].
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.pending_executions.retain(|pending| pending != arn);
        // The stop is this execution's terminal status, so it is also what the
        // execution publishes about having ended.
        self.observe_execution_settled(arn);
        // A Map Run of this execution had not finished either, so it stopped here too.
        self.abort_map_runs_of(arn, now);
        // The job this execution was parked at is nobody's work any more: the record
        // leaves the store here — the next `settle` would drop it as orphaned anyway
        // — and comes back so its own service is told to stop it. An activity task
        // the execution was waiting at is dropped by that same settle, as it is for
        // an execution that reached a terminal status any other way.
        Some(Stopped {
            stop_date: now,
            parked: self.discard_job_task(arn),
        })
    }

    /// Why `RedriveExecution` would refuse `execution` at `now`, in the words
    /// `DescribeExecution`'s `redriveStatusReason` documents — or `None` when it
    /// would redrive it, which is exactly when `redriveStatus` is `REDRIVABLE`
    /// ("redriveStatus is NOT_REDRIVABLE if calling the RedriveExecution API action
    /// would return the ExecutionNotRedrivable error").
    ///
    /// The conditions are `RedriveExecution`'s own: "You can redrive executions if
    /// your original execution meets the following conditions: The execution status
    /// isn't SUCCEEDED. Your workflow execution has not exceeded the redrivable
    /// period of 14 days [...] The workflow execution has not exceeded the maximum
    /// open time of one year [...] The execution event history count is less than
    /// 24,999." An execution that has not reached a terminal status has not finished,
    /// so there is nothing to continue from, and a state machine that is being
    /// deleted can run nothing: both are reasons that member's list names too.
    pub fn not_redrivable_reason(
        &self,
        execution: &Execution,
        now: SystemTime,
    ) -> Option<&'static str> {
        if !execution.is_terminal() {
            return Some(NOT_REDRIVABLE_RUNNING);
        }
        if execution.status == EXECUTION_STATUS_SUCCEEDED {
            return Some(NOT_REDRIVABLE_SUCCEEDED);
        }
        // A state machine whose deletion has begun runs nothing; one the store no
        // longer holds at all is past the end of that same deletion.
        if self
            .state_machines
            .get(&execution.state_machine_arn)
            .is_none_or(StateMachine::is_deleting)
        {
            return Some(NOT_REDRIVABLE_DELETING);
        }
        // "Redriven executions append their event history to the existing event
        // history. Make sure your workflow execution contains less than 24,999
        // events to accommodate the ExecutionRedriven history event and at least one
        // other history event." This emulator's own history ceiling is lower, and an
        // execution already at it has no room for that event either, so whichever is
        // reached first is this reason.
        if execution.history.len() + 2 > MAX_REDRIVABLE_HISTORY_EVENTS.min(MAX_HISTORY_EVENTS) {
            return Some(NOT_REDRIVABLE_EVENT_LIMIT);
        }
        if now
            .duration_since(execution.start_date)
            .is_ok_and(|open| open > MAX_OPEN_DURATION)
        {
            return Some(NOT_REDRIVABLE_MAX_EXECUTION_TIME);
        }
        // The redrivable period "starts from the day a state machine completes its
        // execution", which is the execution's own `stopDate`.
        if execution
            .stop_date
            .and_then(|completed| now.duration_since(completed).ok())
            .is_some_and(|since| since > REDRIVABLE_PERIOD)
        {
            return Some(NOT_REDRIVABLE_PERIOD);
        }
        None
    }

    /// Record a redrive of the execution `arn` names, with the opening history event
    /// and the outcome a re-run of its definition computed.
    ///
    /// The execution reports `RUNNING` again until the clock makes that outcome due,
    /// its `redriveCount` and `redriveDate` move, and `client_token` — when the
    /// caller supplied one — is remembered so a repeat answers the same
    /// `redriveDate`. Answers the `redriveDate` the redrive carries.
    ///
    /// `extra_bytes` is what the activity task the re-run stopped at and the Map Runs
    /// it re-entered will retain, so a redrive is admitted against the whole of what
    /// it adds.
    pub fn redrive_execution(
        &mut self,
        arn: &str,
        now: SystemTime,
        client_token: Option<&str>,
        attempt: RedrivenAttempt,
    ) -> Result<SystemTime, RedriveRejection> {
        let RedrivenAttempt {
            opening,
            pending,
            extra_bytes,
            stopped_at,
        } = attempt;
        let Some(execution) = self.executions.get(arn) else {
            return Err(RedriveRejection::NotFound);
        };
        // A redrive GROWS the record — the outcome the re-run computed, the events it
        // appends and one remembered client token — so it is admitted against the
        // store's ceiling exactly as a start is, rather than growing it past
        // [`MAX_STORE_BYTES`].
        let before = execution.retained_bytes;
        let mut candidate = execution.clone();
        candidate.apply_redrive(now, client_token, opening, pending);
        // The step THIS attempt stopped at, which the next redrive continues from;
        // `apply_redrive` consumed the one the attempt before it left.
        candidate.redrive_step = stopped_at;
        let after = candidate.measure();
        if after + extra_bytes > before && !self.has_room_for(after + extra_bytes - before) {
            return Err(RedriveRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        // The re-run's outcome is owed to the clock, so this execution joins the
        // settle queue again; it left the queue when its first attempt settled. A
        // re-run that SUSPENDED at an activity task owes the clock nothing, so it
        // does not rejoin it.
        self.pending_executions.retain(|pending| pending != arn);
        if candidate.pending.is_some() {
            self.pending_executions.push(arn.to_string());
        }
        self.executions.insert(arn.to_string(), candidate);
        // The execution has been re-run, which is what it publishes about having been
        // redriven: queued at the one place an accepted redrive is recorded, so a
        // refused one — and the idempotent repeat of one — is counted in nothing.
        self.observe_execution_redriven(arn, now);
        Ok(now)
    }

    /// How many executions this state machine already holds.
    pub fn execution_count(&self, state_machine_arn: &str) -> usize {
        self.executions_by_sm
            .get(state_machine_arn)
            .map_or(0, Vec::len)
    }

    /// One state machine's executions, most recent first ([`newest_first`]); the
    /// child workflow executions of its Map Runs are not among them.
    pub fn executions_newest_first(&self, state_machine_arn: &str) -> Vec<&Execution> {
        let mut executions: Vec<&Execution> = self
            .executions_by_sm
            .get(state_machine_arn)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|arn| self.executions.get(arn))
            .collect();
        executions.sort_by(|left, right| newest_first(left, right));
        executions
    }
}

/// The state machine every version of `routing` belongs to, read from the REQUEST
/// alone — nothing of the store is consulted.
///
/// The rules of a routing configuration's own shape, in the order the service judges
/// them: each entry names a state machine VERSION of this service in the endpoint's
/// region, "if you specify the ARN of a second version, it must belong to the same
/// state machine as the first version", and "the sum of the weights in the routing
/// configuration must be equal to 100".
fn routing_shape(
    routing: &[RoutingConfigurationListItem],
    endpoint_region: &str,
) -> Result<String, RoutingRejection> {
    let mut owner: Option<String> = None;
    for item in routing {
        let arn = item.state_machine_version_arn.as_str();
        let parsed =
            read_state_machine_arn(arn, endpoint_region).map_err(RoutingRejection::InvalidArn)?;
        // A routing configuration routes to VERSIONS: an unqualified ARN, a
        // map-run labeled one and an alias ARN each name something that is not
        // a version, so none of them is a configuration this can be applied to.
        let version = parsed
            .qualifier
            .as_deref()
            .filter(|_| parsed.label().is_none())
            .and_then(qualifier_version);
        if version.is_none() {
            return Err(RoutingRejection::InvalidRouting(format!(
                "{arn} is not the ARN of a state machine version"
            )));
        }
        let machine = version_state_machine(arn);
        match owner.as_deref() {
            Some(first) if first != machine => {
                return Err(RoutingRejection::InvalidRouting(format!(
                    "every version of a routing configuration must belong to one state \
                     machine, got {first} and {machine}"
                )))
            }
            Some(_) => {}
            None => owner = Some(machine.to_string()),
        }
    }
    let Some(owner) = owner else {
        // UNREACHABLE through the wire: `routingConfiguration` is modelled
        // 1..2, so the centralized walk has already refused an empty list.
        return Err(RoutingRejection::InvalidRouting(
            "routingConfiguration must name at least one state machine version".to_string(),
        ));
    };

    let weights: Vec<i32> = routing.iter().map(|item| item.weight).collect();
    let total = weights.iter().copied().fold(0_i32, i32::saturating_add);
    if total != ROUTING_WEIGHT_TOTAL {
        return Err(RoutingRejection::InvalidRouting(routing_weight_sum_message(
            &weights,
        )));
    }
    Ok(owner)
}

/// The state machine a VERSION ARN belongs to: that ARN without the `:<version>`
/// qualifier on the end of it.
fn version_state_machine(version_arn: &str) -> &str {
    version_arn
        .rsplit_once(':')
        .map_or(version_arn, |(machine, _)| machine)
}

/// How `ListExecutions` orders two executions: "results are sorted by time, with the
/// most recent execution first. Running executions are sorted by their `startDate`
/// [...] and other executions are sorted by their `stopDate`."
///
/// The sort key is therefore an execution's `stopDate` once it has one and its
/// `startDate` until then, descending; the ARN breaks ties so the order is total and
/// a page token means the same thing on the next call. ONE comparator for both
/// listings, so a Map Run's children and a state machine's executions are never
/// ordered two ways.
fn newest_first(left: &Execution, right: &Execution) -> std::cmp::Ordering {
    let left_key = left.stop_date.unwrap_or(left.start_date);
    let right_key = right.stop_date.unwrap_or(right.start_date);
    right_key
        .cmp(&left_key)
        .then_with(|| right.start_date.cmp(&left.start_date))
        .then_with(|| left.arn.cmp(&right.arn))
}

/// Why the clock closed an outstanding task, for the `cause` the execution it failed
/// reports.
///
/// The error NAME is the language's own ([`TASK_TIMEOUT_ERROR`]); this says which of
/// the scheduling state's two bounds ran out, what that bound was, and what the state
/// was waiting for — a worker's reply to an activity task, or the answer to the token
/// its callback integration handed out. All of it is built from the definition rather
/// than from any one request.
fn task_timeout_cause(task: &ActivityTask) -> String {
    let waited_for = match task.activity_arn {
        Some(_) => "its activity task",
        None => "its task token",
    };
    if task.closed_by_heartbeat() {
        let seconds = task.heartbeat_seconds.unwrap_or_default();
        return format!(
            "state '{}' sent no heartbeat for {waited_for} within the {seconds} seconds its \
             'HeartbeatSeconds' allows",
            task.state_name
        );
    }
    let seconds = task.timeout_seconds.unwrap_or_default();
    format!(
        "state '{}' ran {waited_for} for longer than the {seconds} seconds its \
         'TimeoutSeconds' allows",
        task.state_name
    )
}

/// Why the task that started a parked job ran out of time, for the `cause` the
/// execution it failed reports.
///
/// The error NAME is the language's own ([`TASK_TIMEOUT_ERROR`]); this says which
/// span ran out — the state's own `TimeoutSeconds` where it declares one, else how
/// long this emulator polls one job for — and how long that span was.
fn job_task_timeout_cause(task: &JobTask) -> String {
    match task.timeout_seconds {
        Some(seconds) => format!(
            "state '{}' ran the '{}' job it started for longer than the {seconds} seconds its \
             'TimeoutSeconds' allows",
            task.state_name, task.job.starts
        ),
        None => format!(
            "state '{}' ran the '{}' job it started for longer than the {} seconds this emulator \
             waits for one to finish",
            task.state_name,
            task.job.starts,
            MAX_PARKED_JOB_DURATION.as_secs()
        ),
    }
}

/// What stopping an execution left: the `stopDate` it carries, and the job it was
/// PARKED at when the stop reached it.
#[derive(Debug, Clone)]
pub struct Stopped {
    pub stop_date: SystemTime,
    /// The job a run-a-job task of this execution had started and nothing has
    /// finished, for the caller to stop through that job's own service.
    pub parked: Option<JobTask>,
}

/// One worker's open poll of an activity, counted against [`MAX_ACTIVITY_POLLERS`].
///
/// That count is what the modelled `ActivityWorkerLimitExceeded` reports — "the
/// maximum number of workers concurrently polling for activity tasks has been
/// reached" — so it has to fall again however the poll ends: this gives it back when
/// it is DROPPED, which covers the answer, every early return and a caller that went
/// away while its poll was held.
pub struct ActivityPoll {
    state: ::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
    activity_arn: String,
}

impl ActivityPoll {
    /// Count one more open poll of `activity_arn`, or answer `None` when
    /// [`MAX_ACTIVITY_POLLERS`] are already open on it — or
    /// [`MAX_OPEN_ACTIVITY_POLLS`] across the whole store.
    pub fn open(
        state: &::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
        activity_arn: &str,
    ) -> Option<Self> {
        {
            let mut store = state.lock();
            let open = store
                .activity_pollers
                .get(activity_arn)
                .copied()
                .unwrap_or_default();
            if open >= MAX_ACTIVITY_POLLERS
                || store.open_activity_polls >= MAX_OPEN_ACTIVITY_POLLS
            {
                return None;
            }
            store
                .activity_pollers
                .insert(activity_arn.to_string(), open + 1);
            store.open_activity_polls = store.open_activity_polls.saturating_add(1);
        }
        Some(Self {
            state: state.clone(),
            activity_arn: activity_arn.to_string(),
        })
    }
}

impl Drop for ActivityPoll {
    fn drop(&mut self) {
        let mut store = self.state.lock();
        store.open_activity_polls = store.open_activity_polls.saturating_sub(1);
        let open = store
            .activity_pollers
            .get(&self.activity_arn)
            .copied()
            .unwrap_or_default()
            .saturating_sub(1);
        if open == 0 {
            store.activity_pollers.remove(&self.activity_arn);
        } else {
            store
                .activity_pollers
                .insert(self.activity_arn.clone(), open);
        }
    }
}
