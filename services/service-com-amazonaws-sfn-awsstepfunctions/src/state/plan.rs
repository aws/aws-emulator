// SPDX-License-Identifier: CC0-1.0

//! What ONE walk of a definition leaves for the store to record.
//!
//! One concern of this service's state, held in one file: an attempt is interpreted
//! here ([`plan_run`], [`plan_redrive`]), a suspended execution is continued here
//! ([`plan_reply`], [`plan_callback_reply`], [`plan_job_reply`]), and what each walk
//! reported is turned into the history events, the pending outcome, the activity task,
//! the parked job and the Map Runs the store writes ([`plan_from_run`]). The store's
//! own records and the walk itself live beside this: nothing here holds a lock, so an
//! interpretation never runs with the shared store locked.

use super::*;

/// One attempt at running an execution: what is interpreted, over what input, and
/// where the attempt's history continues from.
///
/// The members travel together — every caller of [`plan_run`] has all of them and
/// none of them means anything without the rest — so they are one argument.
pub struct RunAttempt<'a> {
    pub definition: &'a str,
    pub input: &'a str,
    /// When this attempt started, which every offset the walk reports is measured
    /// from.
    pub start: SystemTime,
    /// What the definition reads through the context object (`$$`): the execution's
    /// and the state machine's own identity, which the caller's request already
    /// fixed.
    pub context: &'a asl::Context,
    /// The type of the event the attempt opens its history with —
    /// [`EVENT_EXECUTION_STARTED`] for a `StartExecution`,
    /// [`EVENT_EXECUTION_REDRIVEN`] for a `RedriveExecution`.
    pub opening: &'a str,
    /// The last id the execution's history already holds, so a redrive's events
    /// CONTINUE the existing numbering instead of restarting it.
    pub previous_event_id: i64,
}

/// Interpret `attempt`'s definition against its input and schedule what the clock
/// owes.
///
/// The run happens HERE, in the call that starts it: [`asl::run`] walks the states
/// and what it reached is reported as a [`Planned`] — a terminal outcome parked in a
/// [`PendingOutcome`] due at `start + EXECUTION_SETTLE_DELAY + <every Wait the walk
/// passed>`, or the activity task the walk SUSPENDED on, which the execution waits
/// at until a worker answers it.
///
/// `tasks` is what the `Task` states of the definition make their service
/// integration calls through ([`integration::interpret`]).
pub fn plan_run(
    attempt: &RunAttempt<'_>,
    tasks: &dyn integration::Tasks,
) -> Planned {
    plan_run_settling_after(attempt, EXECUTION_SETTLE_DELAY, tasks)
}

/// Plan the attempt a `RedriveExecution` begins.
///
/// "When you redrive an execution, it continues the failed execution from the
/// unsuccessful step and uses the same input": when the execution recorded the step
/// its last attempt stopped at ([`asl::RedriveStep`]), the walk begins THERE, with the
/// raw input that step was entered with, so the steps it already got past are not run
/// again; with no such step recorded, the whole definition is walked again from its
/// `StartAt`, which for the deterministic states this emulator runs reaches the same
/// values those steps produced.
///
/// Either way the attempt opens its history with [`EVENT_EXECUTION_REDRIVEN`] —
/// "redriven executions append their event history to the existing event history" —
/// and its outcome is owed to the clock exactly as a start's is.
///
/// `tasks` is the seam [`plan_run`] takes: the step the redrive continues from, and
/// every state after it, makes its service-integration call exactly as the first
/// attempt's did.
///
/// `entered_at` is the id of the event the preserved history holds that step's ENTRY
/// at, which that step's own diagnoses name: its entry is not recorded again, so the
/// attempt appends the work it does from there on.
pub fn plan_redrive(
    attempt: &RunAttempt<'_>,
    from: Option<&asl::RedriveStep>,
    entered_at: Option<i64>,
    tasks: &dyn integration::Tasks,
) -> Planned {
    let Some(from) = from else {
        return plan_run(attempt, tasks);
    };
    let RunAttempt {
        definition,
        input,
        start,
        context,
        opening,
        previous_event_id,
    } = *attempt;
    let run = asl::redrive(
        definition,
        input,
        start,
        context,
        from,
        entered_at,
        first_walked_event_id(previous_event_id, true),
        EXECUTION_SETTLE_DELAY,
        tasks,
    );
    let mut planned =
        plan_from_run(run, start, EXECUTION_SETTLE_DELAY, Some(opening), previous_event_id);
    // "Step Functions preserves the results and execution history of the successful
    // steps, and doesn't rerun these steps": the step this attempt resumes was ENTERED
    // by the attempt being redriven and is not entered again, so the first event this
    // attempt writes FOR THAT STEP continues the causal chain from the preserved event
    // that entered it — not from the event the attempt opened with, which is what an
    // attempt that walks the definition from its `StartAt` (and therefore enters its
    // first state again) follows. `previousEventId` is "the id of the previous event",
    // the one that really precedes an event in that chain, which is what a reader
    // walking back from a failure to the state it happened in follows.
    if let (Some(entered_at), Some(first)) = (entered_at, first_resumed_step_event_mut(&mut planned))
    {
        first.previous_event_id = entered_at;
    }
    planned
}

/// The first event the WALK of a redriven attempt wrote FOR THE STEP it resumed: the event
/// after the one the attempt opened its history with, wherever this plan holds it — among
/// the events that have already happened, else among the ones its terminal outcome is owed
/// with.
///
/// `None` where the walk wrote no event of that step at all: an attempt that resumed a
/// state which ended the execution outright appends its own terminal event and nothing
/// else, and that event is the EXECUTION's rather than the step's, so it follows the event
/// the attempt opened with like any other event of the attempt.
fn first_resumed_step_event_mut(planned: &mut Planned) -> Option<&mut HistoryEvent> {
    if planned.opening.len() > 1 {
        return planned.opening.get_mut(1);
    }
    let events = &mut planned.pending.as_mut()?.events;
    // The LAST of the events an outcome is owed with is the execution's own terminal
    // event, which no step of the walk wrote.
    if events.len() > 1 {
        return events.first_mut();
    }
    None
}

/// What one planned run of a definition left for the store to record.
///
/// At most one of `pending`, `waiting` and `parked` is set, with ONE exception: a walk
/// either reached a terminal outcome the clock owes, stopped at an activity task the
/// execution is waiting for, or parked at the job a run-a-job integration started — and
/// a walk whose concurrent state failed while a task it had already scheduled was
/// outstanding reports both that outcome and that task, which the clock stops when the
/// outcome comes due ([`asl::Run::aborted_task`]).
#[derive(Debug, Clone)]
pub struct Planned {
    /// The events the attempt has ALREADY produced: the opening event, and — for a
    /// run that suspended — the events of the states it walked up to the task it is
    /// waiting at, which have happened and are therefore history rather than
    /// something the clock still owes.
    pub opening: Vec<HistoryEvent>,
    /// The terminal outcome the walk computed, owed to the clock.
    pub pending: Option<PendingOutcome>,
    /// The activity task the walk suspended on, or the one its concurrent state's
    /// failure aborted while it was outstanding.
    pub waiting: Option<PlannedActivityTask>,
    /// The job of a run-a-job integration the walk parked at.
    pub parked: Option<PlannedJob>,
    /// The jobs the walk STARTED and then abandoned, which the operation that planned
    /// the run stops through each one's own integration
    /// ([`integration::stop_abandoned`]): the task that started one will never
    /// finish, because the concurrent state around it failed, so the job is not left
    /// running behind it.
    pub abandoned: Vec<PlannedJob>,
    /// The Map Run every Distributed Map state the walk entered started, with the
    /// offsets the walk reported turned into instants.
    pub map_runs: Vec<PlannedMapRun>,
    /// The unsuccessful step this attempt stopped at, which a later
    /// `RedriveExecution` continues the execution from ([`asl::Run::stopped_at`]).
    pub stopped_at: Option<asl::RedriveStep>,
}

impl Planned {
    /// The instant the jobs this run ABANDONED are stopped at: the instant its own
    /// terminal outcome comes due.
    ///
    /// It is the failure around the task that abandons the job, and that failure is
    /// owed to the clock — the walk accumulated the `Wait` states the failing branch
    /// passed and slept for none of them — so the job it abandoned is stopped when
    /// that failure happens and not when the run was planned. `None` for a run that
    /// reached no outcome, which owes the clock nothing.
    pub fn abandoned_stop_at(&self) -> Option<SystemTime> {
        self.pending.as_ref().map(|pending| pending.due_at)
    }
}

/// One Map Run a planned run started, ready for the store to record.
///
/// This is [`asl::StartedMapRun`] with the run's offsets turned into instants: the
/// Map Run started when its `Map` state was entered and reaches the status the walk
/// computed when the clock makes it due, which is the same delay the execution's own
/// outcome is owed after.
#[derive(Debug, Clone)]
pub struct PlannedMapRun {
    /// The Map Run's own ARN, as the walk resolved it when the `Map` state was entered
    /// ([`asl::StartedMapRun::arn`]): what the store records this run under, so the
    /// results its `ResultWriter` wrote and the output its state put out name the Map
    /// Run a client then reads.
    pub arn: String,
    pub label: String,
    pub status: String,
    pub start_date: SystemTime,
    pub due_at: SystemTime,
    pub max_concurrency: i32,
    pub tolerated_failure_count: i64,
    pub tolerated_failure_percentage: f32,
    pub total_items: i64,
    /// Each item the `Map` state iterated, with the instant its iteration finished:
    /// what the stored record counts its items from.
    pub items: Vec<MapRunItem>,
    /// The child execution each of those items ran as, in the order they started:
    /// what `ListExecutions` reports for this Map Run once the store holds them.
    pub children: Vec<PlannedChildExecution>,
}

/// One child workflow execution a planned run's Distributed Map state started, ready
/// for the store to record as an execution of its own.
///
/// This is [`asl::StartedChildExecution`] with the run's offsets turned into
/// instants: the child started when its iteration did and reaches the terminal status
/// its iteration produced when the clock makes it due — the same delay the Map Run
/// around it and the execution around that are owed after, so no child reports a
/// terminal status before the Map Run that started it could.
#[derive(Debug, Clone)]
pub struct PlannedChildExecution {
    pub name: String,
    pub arn: String,
    pub input: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub status: String,
    pub start_date: SystemTime,
    pub due_at: SystemTime,
    pub item_count: i32,
    /// Whether this is an execution an EARLIER attempt started that this one KEPT
    /// ([`asl::StartedChildExecution::kept`]): the record it already has is left
    /// exactly as it is, so a redrive does not restart a child that had succeeded.
    pub preserved: bool,
}

impl PlannedChildExecution {
    /// The bytes the execution record this will become retains.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Execution>()
            + self.name.len()
            + self.arn.len()
            + self.input.len()
            + self.output.as_ref().map_or(0, String::len)
            + self.error.as_ref().map_or(0, String::len)
            + self.cause.as_ref().map_or(0, String::len)
            // `status` is held twice while the terminal one is still owed to the
            // clock, as the Map Run's own is.
            + 2 * self.status.len()
            // The state machine ARN and the Map Run ARN the record carries, each
            // inside the modelled maximum.
            + 2 * MAX_ARN_BYTES
            // The two history events a child's own life appends: the one it opens
            // with and the terminal one it ends on.
            + 2 * (std::mem::size_of::<HistoryEvent>() + EVENT_EXECUTION_FAILED.len())
    }
}

impl PlannedMapRun {
    /// The bytes the record this will become retains, so an execution is admitted
    /// against the whole of what its start adds to the store.
    ///
    /// The child executions this Map Run started are records of their own, so what
    /// they retain is counted here too: the start is admitted against everything it
    /// writes.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<MapRun>()
            + self.arn.len()
            + self.label.len()
            // `status` is held twice while the terminal one is still owed to the clock.
            + 2 * self.status.len()
            + self.items.len().saturating_mul(std::mem::size_of::<MapRunItem>())
            // The three ARNs the stored record carries. Each is built from a state
            // machine name, an execution name and a state label, all of which the
            // model bounds well inside the `Arn` maximum.
            + 3 * MAX_ARN_BYTES
            + self
                .children
                .iter()
                .map(PlannedChildExecution::measure)
                .sum::<usize>()
    }
}

/// The bytes a run of planned Map Runs retains together.
pub fn planned_map_run_bytes(planned: &[PlannedMapRun]) -> usize {
    planned.iter().map(PlannedMapRun::measure).sum()
}

/// As [`plan_run`], with the delay after which the outcome settles stated by the
/// caller rather than taken as [`EXECUTION_SETTLE_DELAY`].
///
/// An ASYNCHRONOUS start (`StartExecution`, `RedriveExecution`) answers while the
/// execution is still `RUNNING`, so the terminal outcome it computed is owed to the
/// clock and settles one [`EXECUTION_SETTLE_DELAY`] later. A SYNCHRONOUS start
/// (`StartSyncExecution`) answers the terminal outcome in the call itself — "the
/// status code in the API response doesn't reflect function errors", so the response
/// already carries `status`, `stopDate` and the output — so it owes the clock
/// nothing and passes `Duration::ZERO`.
pub fn plan_run_settling_after(
    attempt: &RunAttempt<'_>,
    settle: Duration,
    tasks: &dyn integration::Tasks,
) -> Planned {
    let RunAttempt {
        definition,
        input,
        start,
        context,
        opening,
        previous_event_id,
    } = *attempt;
    let run = asl::run(
        definition,
        input,
        start,
        context,
        first_walked_event_id(previous_event_id, true),
        settle,
        tasks,
    );
    plan_from_run(run, start, settle, Some(opening), previous_event_id)
}

/// Plan the CONTINUATION of an execution whose suspended activity task a worker has
/// answered.
///
/// The definition is walked on FROM the state that scheduled the task, with the raw
/// input that state was entered with and `reply` as its result (see [`asl::resume`]),
/// so what this reports is what the execution does from that reply onward: the events
/// it has produced since, and the terminal outcome it reached, owed to the clock one
/// [`EXECUTION_SETTLE_DELAY`] after the reply just as a start's outcome is owed after
/// the start.
///
/// `start` is the execution's own start, which the context object (`$$`) reports and a
/// `Wait` state's `Timestamp` is measured against; `now` is the instant the reply
/// arrived, which the continuation's own events are stamped from.
///
/// `tasks` is what the `Task` states AFTER the reply make their service integration
/// calls through, exactly as the start's own states did.
///
/// `None` when the definition no longer declares the state the task was scheduled at,
/// so there is no continuation to record.
#[allow(clippy::too_many_arguments)]
pub fn plan_reply(
    definition: &str,
    input: &str,
    start: SystemTime,
    now: SystemTime,
    context: &asl::Context,
    reply: &asl::ActivityReply,
    previous_event_id: i64,
    tasks: &dyn integration::Tasks,
) -> Option<Planned> {
    // What the execution has already spent, which a definition-level `TimeoutSeconds`
    // counts: a task that stayed outstanding was part of this execution's run.
    let elapsed = now.duration_since(start).unwrap_or_default();
    let run = asl::resume(
        definition,
        input,
        start,
        context,
        reply,
        elapsed,
        first_walked_event_id(previous_event_id, false),
        EXECUTION_SETTLE_DELAY,
        tasks,
    )?;
    let mut planned = plan_from_run(run, now, EXECUTION_SETTLE_DELAY, None, previous_event_id);
    // The event that records the task SUCCEEDING carries what the worker answered:
    // "the JSON output of the task", which is the reply this walk continued from.
    let answered = planned
        .opening
        .iter_mut()
        .chain(
            planned
                .pending
                .iter_mut()
                .flat_map(|pending| pending.events.iter_mut()),
        )
        .find(|event| event.event_type == EVENT_ACTIVITY_SUCCEEDED);
    if let Some(event) = answered {
        event.details = EventDetails {
            output: Some(reply.result.to_string()),
            ..EventDetails::default()
        };
    }
    Some(planned)
}

/// Plan the CONTINUATION of an execution whose callback (`.waitForTaskToken`) task a
/// `SendTaskSuccess` or `SendTaskFailure` has answered.
///
/// The definition is walked on FROM the state whose integration call carried the
/// token, with the raw input that state was entered with and what the answer reported
/// as its outcome (see [`asl::resume_callback`]), so what this reports is what the
/// execution does from that answer onward: the events it has produced since, and the
/// terminal outcome it reached, owed to the clock one [`EXECUTION_SETTLE_DELAY`] after
/// the answer just as a start's outcome is owed after the start.
///
/// `start` is the execution's own start, which the context object (`$$`) reports and a
/// `Wait` state's `Timestamp` is measured against; `now` is the instant the answer
/// arrived, which the continuation's own events are stamped from.
///
/// `tasks` is what the states AFTER the answer make their service integration calls
/// through — including the state being answered itself, whose `Retry` makes the call
/// again for its next attempt.
///
/// `None` when the definition no longer declares the state that was waiting, so there
/// is no continuation to record.
#[allow(clippy::too_many_arguments)]
pub fn plan_callback_reply(
    definition: &str,
    input: &str,
    start: SystemTime,
    now: SystemTime,
    context: &asl::Context,
    reply: &asl::CallbackReply,
    previous_event_id: i64,
    tasks: &dyn integration::Tasks,
) -> Option<Planned> {
    // What the execution has already spent, which a definition-level `TimeoutSeconds`
    // counts: a token that stayed unanswered was part of this execution's run.
    let elapsed = now.duration_since(start).unwrap_or_default();
    let run = asl::resume_callback(
        definition,
        input,
        start,
        context,
        reply,
        elapsed,
        first_walked_event_id(previous_event_id, false),
        EXECUTION_SETTLE_DELAY,
        tasks,
    )?;
    Some(plan_from_run(
        run,
        now,
        EXECUTION_SETTLE_DELAY,
        None,
        previous_event_id,
    ))
}

/// Plan the CONTINUATION of an execution whose parked job has ENDED.
///
/// The definition is walked on FROM the state that started the job, with the raw
/// input that state was entered with and what the job ended as its outcome (see
/// [`asl::resume_job`]), so what this reports is what the execution does from that
/// outcome onward: the events it has produced since, and the terminal outcome it
/// reached, owed to the clock one [`EXECUTION_SETTLE_DELAY`] after the job ended just
/// as a worker's reply's outcome is owed after the reply.
///
/// `start` is the execution's own start, which the context object (`$$`) reports and a
/// `Wait` state's `Timestamp` is measured against; `now` is the instant the job was
/// found finished, which the continuation's own events are stamped from.
///
/// `None` when the definition no longer declares the state that started the job, so
/// there is no continuation to record.
#[allow(clippy::too_many_arguments)]
pub fn plan_job_reply(
    definition: &str,
    input: &str,
    start: SystemTime,
    now: SystemTime,
    context: &asl::Context,
    reply: &asl::JobReply,
    previous_event_id: i64,
    tasks: &dyn integration::Tasks,
) -> Option<Planned> {
    // What the execution has already spent, which a definition-level `TimeoutSeconds`
    // counts: a job that stayed unfinished was part of this execution's run.
    let elapsed = now.duration_since(start).unwrap_or_default();
    let run = asl::resume_job(
        definition,
        input,
        start,
        context,
        reply,
        elapsed,
        first_walked_event_id(previous_event_id, false),
        EXECUTION_SETTLE_DELAY,
        tasks,
    )?;
    Some(plan_from_run(
        run,
        now,
        EXECUTION_SETTLE_DELAY,
        None,
        previous_event_id,
    ))
}

/// What a worker's reply that reports its activity task FAILED leaves for the store
/// to record, once no `Retry` of the scheduling state answers for the error.
///
/// There is no walk to interpret: the state has no retrier left for the error
/// ([`ComAmazonawsSfnAwsstepfunctionsState::retry_failed_activity_task`] is what
/// answers one that has) and declares no `Catch` this emulator interprets (see
/// [`asl`]), so the execution waiting at the task fails, carrying the reply's own
/// `error` and `cause`. That outcome is owed to the clock one
/// [`EXECUTION_SETTLE_DELAY`] after the reply, exactly as the outcome a SUCCESSFUL
/// reply continues to is ([`plan_reply`]), so one rule covers both replies: the
/// execution is `RUNNING` until the instant the reply made its outcome due.
///
/// `state_name` is the state that scheduled the task, as the stored task names it,
/// and `previous_event_id` the last event the execution's history holds, so the two
/// events this appends continue that numbering.
pub fn plan_task_failure(
    state_name: &str,
    now: SystemTime,
    error: Option<String>,
    cause: Option<String>,
    previous_event_id: i64,
) -> Planned {
    let due_at = after(now, EXECUTION_SETTLE_DELAY);
    // The task failed when the worker said so; the execution ends when its outcome
    // comes due — the same two instants [`plan_from_run`] stamps a walked state and a
    // terminal event with.
    let failed = HistoryEvent {
        id: previous_event_id + 1,
        previous_event_id,
        event_type: EVENT_ACTIVITY_FAILED.to_string(),
        timestamp: now,
        state_name: Some(state_name.to_string()),
        details: EventDetails::of_failure(error.clone(), cause.clone()),
    };
    let ended = HistoryEvent {
        id: failed.id + 1,
        previous_event_id: failed.id,
        event_type: EVENT_EXECUTION_FAILED.to_string(),
        timestamp: due_at,
        state_name: None,
        details: EventDetails::of_failure(error.clone(), cause.clone()),
    };
    Planned {
        opening: vec![failed],
        pending: Some(PendingOutcome {
            due_at,
            status: EXECUTION_STATUS_FAILED.to_string(),
            output: None,
            error,
            cause,
            events: vec![ended],
        }),
        waiting: None,
        parked: None,
        // Nothing was walked here, so no job was started and none abandoned: this
        // plan is the closure of a task the clock closed.
        abandoned: Vec::new(),
        map_runs: Vec::new(),
        // The attempt stopped at the state that scheduled the task, which the caller
        // records from the task itself.
        stopped_at: None,
    }
}

/// What an attempt that could not be RUN AT ALL leaves for the store to record: the
/// event the attempt opens with, and the FAILURE `failed` names, owed to the clock one
/// [`EXECUTION_SETTLE_DELAY`] after the attempt began.
///
/// There is no walk to interpret: an execution's work is done with credentials of its
/// state machine's execution role, so an attempt this service cannot act as that role for
/// enters not one of the definition's states — nothing of the run happens, and what the
/// execution ends as is the failure the caller states rather than an outcome a walk
/// computed. The outcome is owed to the clock exactly as a walked attempt's is, so the
/// execution reports `RUNNING` until the instant it comes due.
///
/// `opening` is the event the attempt opens its history with
/// ([`EVENT_EXECUTION_STARTED`] for a start, [`EVENT_EXECUTION_REDRIVEN`] for a redrive)
/// and `previous_event_id` the last id that history already holds, so the two events this
/// plans continue that numbering exactly as [`plan_from_run`] numbers a walk's.
pub fn plan_attempt_that_cannot_run(
    start: SystemTime,
    opening: &str,
    previous_event_id: i64,
    failed: &integration::Failed,
) -> Planned {
    let due_at = after(start, EXECUTION_SETTLE_DELAY);
    let opened = HistoryEvent {
        id: previous_event_id + 1,
        previous_event_id,
        event_type: opening.to_string(),
        timestamp: start,
        state_name: None,
        details: EventDetails::default(),
    };
    let details = EventDetails::of_failure(Some(failed.error.clone()), Some(failed.cause.clone()));
    let ended = HistoryEvent {
        id: opened.id + 1,
        // An `ExecutionStarted` is no predecessor, so the event after it reports the
        // sentinel; a redriven attempt's own opening event IS one ([`follows_event`]).
        previous_event_id: follows_event(opened.id, opening),
        event_type: EVENT_EXECUTION_FAILED.to_string(),
        timestamp: due_at,
        state_name: None,
        details,
    };
    Planned {
        opening: vec![opened],
        pending: Some(PendingOutcome {
            due_at,
            status: EXECUTION_STATUS_FAILED.to_string(),
            output: None,
            error: Some(failed.error.clone()),
            cause: Some(failed.cause.clone()),
            events: vec![ended],
        }),
        waiting: None,
        parked: None,
        // Nothing was walked, so no job was started and none abandoned, and no Map state
        // was entered.
        abandoned: Vec::new(),
        map_runs: Vec::new(),
        // No state was entered either, so a later redrive of this execution walks the
        // definition from its `StartAt` rather than continuing from a step.
        stopped_at: None,
    }
}

/// The id the FIRST history event a walk records will carry, which is what a cause
/// naming the event a state was entered at reports.
///
/// The events of one execution are numbered from [`asl::FIRST_EVENT_ID`] in order, and
/// an attempt CONTINUES that numbering: a walk's own events follow the last id the
/// history already holds (`previous_event_id`), after the event the attempt OPENS with
/// where it writes one (`opening`) — which is exactly how [`plan_from_run`] numbers
/// them once the walk is over.
fn first_walked_event_id(previous_event_id: i64, opening: bool) -> i64 {
    previous_event_id + asl::FIRST_EVENT_ID + i64::from(opening)
}

/// What one walk left for the store to record: its events numbered after
/// `previous_event_id`, its outcome owed `settle` after the work that produced it was
/// done, and the activity task or Map Runs it produced.
///
/// `opening` is the event the attempt OPENS with, for a walk that is an attempt of its
/// own (`ExecutionStarted`, `ExecutionRedriven`); a walk that CONTINUES an attempt — a
/// worker's reply to the task it was suspended at — opens with none, because the
/// attempt it continues was already opened.
fn plan_from_run(
    mut run: asl::Run,
    start: SystemTime,
    settle: Duration,
    opening: Option<&str>,
    previous_event_id: i64,
) -> Planned {
    let stopped_at = run.stopped_at.take();
    // The jobs the walk started and abandoned, with the walk's offsets turned into
    // instants exactly as the parked one's is: the caller stops each of them.
    let abandoned: Vec<PlannedJob> = std::mem::take(&mut run.abandoned)
        .into_iter()
        .map(|job| planned_job(job, start))
        .collect();
    let opened: Vec<HistoryEvent> = opening
        .map(|event_type| HistoryEvent {
            id: previous_event_id + 1,
            previous_event_id,
            event_type: event_type.to_string(),
            timestamp: start,
            state_name: None,
            details: EventDetails::default(),
        })
        .into_iter()
        .collect();
    let first_id = previous_event_id + 1 + i64::try_from(opened.len()).unwrap_or(0);
    // What the FIRST event of this walk names as its predecessor: the event the walk
    // opened with, or — for a walk that continues an attempt — the event the history
    // already ended on. An `ExecutionStarted` is no predecessor, so the event after it
    // reports the sentinel ([`follows_event`]).
    let first_previous = match opening {
        Some(event_type) => follows_event(first_id - 1, event_type),
        None => first_id - 1,
    };
    // The offsets below are accumulated from the `Wait` states of a CALLER'S
    // definition, so every instant is computed with checked arithmetic: an
    // offset no instant can represent is held at the furthest instant this clock
    // carries, rather than reached through the panicking `Add`.
    //
    // The outcome is owed `settle` after the WORK that reached it was done, and the work
    // of a run performed in the call that starts it is done when this walk is — which is
    // later than the walk's own offsets say whenever its states' calls really took time,
    // because those offsets are the logical clock the definition's `Wait` states advance
    // and no call may move ([`asl::Interpreter::perform_call`]). Measuring the span from
    // the run's START instead would owe the outcome of a run whose work took longer than
    // `settle` at an instant already past, so the caller that started it would be
    // answered and find the run already over — and a `StopExecution` it sends straight
    // after would have nothing running to stop.
    //
    // What it does NOT change is WHEN the work happened: a run is interpreted inside the
    // call that starts it, so every event of the walk is stamped at an instant already
    // past by the time that call answers, and the only thing the clock still owes is the
    // outcome. A caller that stops such a run therefore stops a run whose states have
    // all been performed — the history says so, and a trace of it says so too
    // ([`super::xray_tracing`]) — where the real service would have had work in flight.
    // Reporting work in flight after the start has answered is a property of the
    // INTERPRETATION, not of this span: it needs the walk to be performed past that
    // answer, which is a change to [`asl::run`] and its shared logical clock (the
    // branches of a `Parallel` share it, so no call may move it) rather than a further
    // clamp here.
    //
    // The Map Runs this walk started and their items are held to the same floor, for the
    // same reason and from the same reading of the clock: their items' work was all done
    // inside this walk, so neither may be reported finished at an instant that was already
    // past when the walk ended. A CHILD execution's own instant is raised by the walk
    // itself ([`asl::Interpreter::settle`]), because the results written for a Map Run
    // report it as well and the two cannot disagree.
    let no_sooner_than = after(super::now(), settle);
    let due_at = after(start, settle.saturating_add(run.elapsed)).max(no_sooner_than);
    // What this attempt ENDED with, which is what the event it ends on reports: a
    // later attempt of the same execution computes its own outcome, and the event
    // this one wrote stays in the history.
    let outcome = EventDetails::of_outcome(
        run.output.clone(),
        run.error.clone(),
        run.cause.clone(),
    );
    let events: Vec<HistoryEvent> = run
        .steps
        .into_iter()
        .zip(first_id..)
        .map(|(step, id)| HistoryEvent {
            id,
            previous_event_id: if id == first_id {
                first_previous
            } else {
                id - 1
            },
            event_type: step.event_type.to_string(),
            // Each state was entered and left at its own offset into the run; the
            // terminal event is stamped at the instant the outcome came due.
            timestamp: if step.terminal {
                due_at
            } else {
                after(start, step.after)
            },
            state_name: step.state_name,
            details: if step.terminal {
                outcome.clone()
            } else {
                step.details
            },
        })
        .collect();
    // Every Map Run the walk started, with its offsets turned into instants against
    // the execution's own start: it started when its `Map` state was entered, and
    // reaches the status the walk computed one `settle` after its items finished —
    // the same delay the execution's own outcome is owed after, and never sooner than
    // the floor every instant this walk owes is held to, so a Map Run never reports a
    // terminal status before the execution around it could nor a window shorter than
    // the work its items really did.
    let map_runs: Vec<PlannedMapRun> = run
        .map_runs
        .into_iter()
        .map(|map_run| PlannedMapRun {
            arn: map_run.arn,
            label: map_run.label,
            status: map_run.status.to_string(),
            start_date: after(start, map_run.started_at),
            due_at: after(start, settle.saturating_add(map_run.stopped_at)).max(no_sooner_than),
            max_concurrency: map_run.max_concurrency,
            tolerated_failure_count: map_run.tolerated_failure_count,
            tolerated_failure_percentage: map_run.tolerated_failure_percentage,
            total_items: map_run.total_items,
            // Each ITEM is owed the same way the Map Run around it is: its iteration
            // finished at its own offset into the walk, and is counted one `settle`
            // after that instant, under the same floor — so the last item to finish
            // comes due exactly when the Map Run's own status does, and no item is
            // counted in the call that started the execution.
            items: map_run
                .items
                .into_iter()
                .map(|item| MapRunItem {
                    due_at: after(start, settle.saturating_add(item.finished_at))
                        .max(no_sooner_than),
                    failed: item.failed,
                })
                .collect(),
            // And each CHILD execution: it started when its own iteration did, and the
            // terminal status that iteration produced is owed one `settle` after the
            // instant it finished — an offset the WALK already raised to the time its
            // items really took, because the results written for a child report it too
            // ([`asl::Interpreter::settle`]) — so a caller reading a child straight after
            // the start sees it RUNNING, exactly as the Map Run and the execution around
            // it are, and the window the child reports covers the work its own iteration
            // really did.
            children: map_run
                .children
                .into_iter()
                .map(|child| PlannedChildExecution {
                    name: child.name,
                    arn: child.arn,
                    input: child.input,
                    output: child.output,
                    error: child.error,
                    cause: child.cause,
                    status: child.status.to_string(),
                    start_date: after(start, child.started_at),
                    due_at: after(start, settle.saturating_add(child.finished_at)),
                    item_count: child.item_count,
                    preserved: child.kept.is_some(),
                })
                .collect(),
        })
        .collect();
    // A walk that SUSPENDED computed no outcome: what it produced is the task it is
    // waiting at, available to a worker once the clock reaches the offset the walk
    // got to — every `Wait` before the state is still owed, and nothing slept for
    // them. The events of the states it walked are history, so they are recorded now
    // rather than parked.
    if let Some(waiting) = run.waiting {
        let mut history = opened;
        history.extend(events);
        let scheduled = planned_activity_task(waiting, start, false);
        record_what_was_scheduled(&mut history, &scheduled);
        return Planned {
            opening: history,
            pending: None,
            waiting: Some(scheduled),
            parked: None,
            abandoned,
            map_runs,
            stopped_at,
        };
    }
    // A walk that PARKED at a job computed no outcome either: the job was started at
    // the offset the walk got to, and what happens next is that job finishing. The
    // events of the states it walked — including the one recording the submission —
    // are history, so they are recorded now rather than parked.
    if let Some(parked) = run.parked {
        let mut history = opened;
        history.extend(events);
        return Planned {
            opening: history,
            pending: None,
            waiting: None,
            parked: Some(planned_job(parked, start)),
            abandoned,
            map_runs,
            stopped_at,
        };
    }
    // The task a sibling unit's failure ABORTED was already queued under its activity,
    // so it is recorded beside the outcome the clock owes: a worker polling that
    // activity is handed it until the outcome comes due, when the task goes with the
    // execution it belonged to.
    let mut events = events;
    let mut opening = opened;
    let aborted = run
        .aborted_task
        .map(|task| planned_activity_task(task, start, true));
    if let Some(aborted) = &aborted {
        record_what_was_scheduled(&mut events, aborted);
        // The events of the states this walk ALREADY RAN have happened, so they are
        // recorded now rather than left owed, exactly as a walk that suspended at a task
        // records them ([`Planned::opening`]). That is what makes the ids they carry
        // final: the one event a worker's claim appends (`ActivityStarted`) is numbered
        // after them, so no event a diagnosis of this run NAMES can be moved by it
        // ([`super::Execution::push_state_event`]).
        let happened = events
            .iter()
            .take(super::in_flight::reportable_before_outcome(&events))
            .take_while(|event| event.timestamp <= super::now())
            .count();
        let owed = events.split_off(happened);
        opening.extend(events);
        events = owed;
    }
    Planned {
        opening,
        pending: Some(PendingOutcome {
            due_at,
            status: run.status.to_string(),
            output: run.output,
            error: run.error,
            cause: run.cause,
            events,
        }),
        waiting: aborted,
        parked: None,
        abandoned,
        map_runs,
        stopped_at,
    }
}

/// One activity task a walk scheduled, ready for the store to queue:
/// [`asl::ScheduledActivity`] with the walk's offset turned into an instant against the
/// execution's own start.
///
/// `aborted` is whether the concurrent state around the scheduling state has already
/// failed, so nothing waits at the task any more ([`PlannedActivityTask::aborted`]).
fn planned_activity_task(
    scheduled: asl::ScheduledActivity,
    start: SystemTime,
    aborted: bool,
) -> PlannedActivityTask {
    PlannedActivityTask {
        activity_arn: scheduled.activity_arn,
        token: scheduled.token,
        state_name: scheduled.state_name,
        input: scheduled.input,
        state_input: scheduled.raw_input.to_string(),
        available_at: after(start, scheduled.at),
        timeout_seconds: scheduled.timeout_seconds,
        heartbeat_seconds: scheduled.heartbeat_seconds,
        retries_taken: scheduled.retries_taken,
        frames: scheduled.frames,
        aborted,
    }
}

/// Write what was scheduled onto the event that RECORDS the scheduling.
///
/// The walk computed the activity, the input and the bounds, so the event the history
/// keeps reports them rather than leaving its detail member unset. A CALLBACK task
/// records no such event: its own events are the `Task` state's, which the walk wrote
/// with the work and the document its call carried.
fn record_what_was_scheduled(events: &mut [HistoryEvent], scheduled: &PlannedActivityTask) {
    let Some(activity_arn) = scheduled.activity_arn.clone() else {
        return;
    };
    let Some(event) = events
        .iter_mut()
        .rev()
        .find(|event| event.event_type == EVENT_ACTIVITY_SCHEDULED)
    else {
        return;
    };
    event.details = EventDetails {
        resource: Some(activity_arn),
        input: Some(scheduled.input.clone()),
        timeout_seconds: scheduled.timeout_seconds,
        heartbeat_seconds: scheduled.heartbeat_seconds,
        ..EventDetails::default()
    };
}

/// One job a walk started, ready for the store to record or for the caller to stop:
/// [`asl::ParkedJob`] with the walk's offset turned into an instant against the
/// execution's own start.
fn planned_job(parked: asl::ParkedJob, start: SystemTime) -> PlannedJob {
    PlannedJob {
        state_name: parked.state_name,
        state_input: parked.raw_input.to_string(),
        job: parked.job,
        api: parked.api,
        family: parked.family,
        parsed_output: parked.parsed_output,
        job_id: parked.job_id,
        naming: parked.naming,
        acts_on: parked.acts_on,
        parked_at: after(start, parked.at),
        timeout_seconds: parked.timeout_seconds,
        frames: parked.frames,
    }
}
