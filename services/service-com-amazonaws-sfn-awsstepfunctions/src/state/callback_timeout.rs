// SPDX-License-Identifier: CC0-1.0

//! The CONTINUATION of an execution whose task ran out of the time its state allowed it
//! — the task of an ACTIVITY a worker claimed, or a callback (`.waitForTaskToken`) task.
//!
//! "If the task runs longer than the specified seconds, this state fails with a
//! States.Timeout error name" — and that failure is the `Task` state's own, so the state
//! answers for it exactly as it answers for a failure a `SendTaskFailure` reported: a
//! `Retry` whose retrier names the error makes the state's work's next attempt (which for
//! an ACTIVITY is a task the store queues itself, so it never reaches here), a `Catch`
//! whose catcher names it carries the execution on at that catcher's `Next`, and with
//! neither the failure is the execution's.
//!
//! ## Why it is not the settle that applies it
//!
//! The bound itself IS a transition the clock owes, and
//! [`super::ComAmazonawsSfnAwsstepfunctionsState::settle`] is what materialises it. But
//! what the state DOES with the failure is a WALK of the definition — the next attempt
//! makes a service-integration call, and a catcher's branch runs states of its own —
//! which cannot be run with the store locked and cannot be run inside a settle at all.
//! So the two are split:
//!
//! * the settle LEAVES such a task alone while its continuation is owed
//!   ([`MAX_CONTINUATION_DELAY`]), because the execution is still running: the state has
//!   not failed, it has work left to do;
//! * this module CLAIMS the timed-out task — one claimant, in the critical section that
//!   reads it — interprets the continuation with no lock held, and records what it
//!   produced, exactly as a worker's reply and a finished job are recorded
//!   ([`super::plan_callback_reply`], [`super::run_a_job`]);
//! * a claim nothing could continue, and a bound whose continuation never came, fail the
//!   execution with the timeout after all, so no execution waits at a closed task for
//!   ever.
//!
//! ## Who drives it
//!
//! Whoever reaches the store: every operation that publishes what its own settle
//! materialised hands the continuation to a task of its own ([`drive`], so no request
//! waits for a walk), and the in-process clock that keeps publishing for an execution
//! nobody is asking about runs it on its own wake-ups ([`continue_due`]) — the same two
//! drivers every other transition this service owes has.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use smithy_server_runtime::EmbeddedEmulator;

use super::asl;
use super::integration;

/// How long a task whose bound has run out waits to be CONTINUED before the store fails
/// its execution with the timeout instead.
///
/// Emulator bound, NOT an AWS limit: on the service the state's error handling is
/// applied at the instant the bound is reached. Here a continuation is a walk, which
/// only a driver outside the settle can run ([`drive`], [`continue_due`]), so this is
/// the span that is allowed for one to arrive — generous next to how often this
/// service's own clock wakes up, so a loaded host cannot turn a caught timeout into a
/// failed execution, and bounded so an execution whose continuation never came still
/// reaches the terminal status its timed-out task owes it.
pub const MAX_CONTINUATION_DELAY: Duration = Duration::from_secs(30);

/// How many timed-out tasks ONE pass continues, so a driver cannot be held
/// walking a store full of them: what is left over is claimed by the next pass, which
/// the clock makes on its own.
const MAX_CONTINUATIONS_PER_PASS: usize = 8;

/// Hand the continuation of every timed-out task this store owes one for to a
/// task of its own, and return at once.
///
/// For a driver that must ANSWER: the walk a continuation runs makes the state's own
/// integration calls, so it is not work a request can be held for — exactly as the poll
/// of a parked job is not ([`super::run_a_job::watch`]). With no async runtime to spawn
/// onto nothing is handed over, and the continuation is left to the clock.
pub fn drive<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, callback: &EmbeddedEmulator) {
    if ::tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let engine = Arc::clone(engine);
    let callback = callback.clone();
    ::tokio::spawn(async move {
        // The hand-over below is for what THIS pass's own continuations queued: a pass
        // that claimed nothing has settled nothing and queued nothing, and handing over
        // then would take the publications the caller that drove it is about to make
        // itself.
        if continue_due(&engine, &callback).await > 0 {
            super::events::hand_over_settled(&engine, &callback).await;
        }
    });
}

/// Continue every timed-out task this store owes a continuation for, here, and
/// answer how many were continued.
///
/// For a driver that is already background work — this service's own clock — and the one
/// implementation both drivers run. It takes this service's store only in the critical
/// sections it reads and records in, and never while a walk is running.
pub async fn continue_due<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
) -> usize {
    let Some(state) = super::service_state(&**engine) else {
        return 0;
    };
    let mut continued = 0;
    while continued < MAX_CONTINUATIONS_PER_PASS {
        let now = super::now();
        // Phase 1, under the lock: CLAIM one timed-out task and read everything
        // its continuation needs. The claim and the reading are one critical section, so
        // two drivers cannot continue one task twice — and nothing ELSE of the store is
        // touched here: the bound a claim is made on is the task's own, which
        // [`ComAmazonawsSfnAwsstepfunctionsState::claim_timed_out_task`] reads
        // without settling, so this pass does not materialise — nor queue the
        // publications of — transitions the caller that drove it settles and publishes
        // itself.
        let claimed = {
            let mut store = state.lock();
            let Some((task, due)) = store.claim_timed_out_task(now) else {
                break;
            };
            let facts = Facts::read(&store, &task);
            (task, due, facts)
        };
        let (task, due, facts) = claimed;
        continued += 1;
        let Some(facts) = facts else {
            // The execution went while the claim was taken, or its state machine is no
            // longer there: there is nothing left to continue, and nothing to fail.
            continue;
        };
        if !continue_one(engine, callback, &state, &task, due, facts).await {
            // Nothing could be continued from the claim, so the bound the clock reached
            // is the execution's failure after all — the same failure a state that
            // answers the timeout in neither way takes.
            let mut store = state.lock();
            store.fail_timed_out_task(&task, due);
        }
    }
    continued
}

/// What one continuation needs out of the execution the claimed task was waiting at.
struct Facts {
    /// The definition the execution RUNS, which for one started through a version ARN is
    /// that version's immutable snapshot.
    definition: String,
    /// The execution's own input, which the context object (`$$`) reports.
    input: String,
    start_date: SystemTime,
    machine_arn: String,
    context: asl::Context,
    /// The last id the preserved history holds, which the continuation's events
    /// continue from.
    previous_event_id: i64,
    /// The id of the event that history holds the ENTRY of the waiting state at, which
    /// the continuation does not record again.
    entered_event_id: Option<i64>,
}

impl Facts {
    /// Read them out of the store, or `None` for an execution that is gone or terminal —
    /// or whose state machine is, which leaves no definition to continue.
    fn read(
        store: &super::ComAmazonawsSfnAwsstepfunctionsState,
        task: &super::ActivityTask,
    ) -> Option<Self> {
        let execution = store
            .executions
            .get(&task.execution_arn)
            .filter(|execution| !execution.is_terminal())?;
        let machine = store.state_machines.get(&execution.state_machine_arn)?;
        // The context object (`$$`) the continuation reads is the SAME execution's
        // identity: the timeout continues this execution rather than starting one.
        let context = asl::Context {
            execution_arn: execution.arn.clone(),
            execution_name: execution.name.clone(),
            state_machine_arn: machine.arn.clone(),
            state_machine_name: machine.name.clone(),
            role_arn: machine.role_arn.clone(),
            // The continuation CONTINUES the attempt the task was handed over on rather
            // than beginning another, so the states it runs read the count the record
            // already carries.
            redrive_count: execution.redrive_count,
            express: machine.sm_type == super::STATE_MACHINE_TYPE_EXPRESS,
        };
        let definition = execution
            .state_machine_version_arn
            .as_deref()
            .and_then(|version_arn| store.state_machine_versions.get(version_arn))
            .map_or_else(
                || machine.definition.clone(),
                |version| version.definition.clone(),
            );
        Some(Self {
            definition,
            input: execution.input.clone(),
            start_date: execution.start_date,
            machine_arn: machine.arn.clone(),
            context,
            previous_event_id: execution.last_event_id(),
            // The continuation BEGINS at the outermost concurrent state the task is
            // nested inside, so that is the state whose preserved entry it names
            // ([`asl::job_entry_state`]).
            entered_event_id: execution
                .entered_event_id(asl::job_entry_state(&task.state_name, &task.frames)),
        })
    }
}

/// Interpret and record the continuation of ONE claimed task, and report whether it was
/// recorded.
///
/// `false` for a claim nothing could be continued from: the definition no longer
/// declares the state that was waiting, the interpretation could not be run, or the
/// execution took none of it.
async fn continue_one<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    state: &::engine::ServiceState<super::ComAmazonawsSfnAwsstepfunctionsState>,
    task: &super::ActivityTask,
    due: SystemTime,
    facts: Facts,
) -> bool {
    let Facts {
        definition,
        input,
        start_date,
        machine_arn,
        context,
        previous_event_id,
        entered_event_id,
    } = facts;
    let execution_arn = task.execution_arn.clone();
    let reply = asl::CallbackReply {
        state_name: task.state_name.clone(),
        // The state's RAW input, which its `ResultPath` and its `Catch` are applied
        // against: the task carries it, so the failure is placed against the document
        // the state was really entered with.
        state_input: task.state_input.clone(),
        // The bound that ran out IS the task's outcome, under the name the language
        // gives it and with nothing beside it ([`asl::timed_out`]).
        outcome: Err(asl::timed_out()),
        // The CLOCK closed it, which is what the event this closure records reports.
        by_the_clock: true,
        // A HEARTBEAT window that lapsed answers to the second name the language states
        // for that closure as well, so a catcher naming either takes it over.
        also_answers: asl::task_closure_errors(task.closed_by_heartbeat()),
        // What this state's retriers have already spent, so the continuation takes that
        // budget up rather than restarting it.
        retries_taken: task.retries_taken.clone(),
        // The concurrent states that state is nested inside, which this continuation
        // re-enters to reach it.
        frames: task.frames.clone(),
        entered_event_id,
    };

    // Phase 2, OUTSIDE the lock: the continuation is interpreted, from the state whose
    // call carried the token. Its `Retry` makes that call again and its `Catch`
    // transitions to the catcher it names, so those states call through the same seam a
    // request's own walk calls through, in the state machine's own Region.
    let region = super::arn_region(&machine_arn);
    let interpreted = {
        let definition = definition.clone();
        let input = input.clone();
        let region = region.clone();
        // The states after the timeout call as the SAME execution role the states before
        // it did, assumed again for this continuation.
        let unassumable = super::task_authorization::unassumable_role(
            callback,
            &region,
            &context.role_arn,
            &context.state_machine_arn,
        )
        .await;
        // The continuation is run by the CLOCK rather than by a request, so there is no
        // credential store to record a fresh assume in: it acts as the session this
        // emulator already holds for the role
        // ([`super::execution_role_session::role_session`]).
        let acting = super::task_authorization::acting(
            engine,
            &region,
            &context.role_arn,
            unassumable,
            None,
        );
        // The continuation is work this service owes the clock rather than a step of
        // whatever call happened to drive it, so it opens a chain of in-process calls of
        // its own — bounded, as every chain is, by how deep the states it runs go.
        integration::interpret_through(callback, 0, &region, None, acting, move |tasks| {
            super::plan_callback_reply(
                &definition,
                &input,
                start_date,
                due,
                &context,
                &reply,
                previous_event_id,
                tasks,
            )
        })
        .await
    };
    let Ok(Some(planned)) = interpreted else {
        return false;
    };
    // A job this continuation STARTED and then abandoned is stopped through its own
    // integration, as a start's own is. Done with no store of this service locked.
    integration::stop_abandoned(
        callback,
        &region,
        &planned.abandoned,
        planned.abandoned_stop_at(),
    )
    .await;

    // Phase 3, under the lock again: record it. Everything is re-read, because the
    // execution could have been stopped while the walk ran. The lock is held for THIS
    // block alone, because the job a continuation parked at is watched with no store of
    // this service locked.
    let (parked_a_job, held_at) = {
        let mut store = state.lock();
        let waiting = planned.waiting;
        let parked = planned.parked;
        let parked_wait = planned.parked_wait;
        let map_runs = planned.map_runs;
        let extra_bytes = waiting.as_ref().map_or(0, super::PlannedActivityTask::measure)
            + parked.as_ref().map_or(0, super::PlannedJob::measure)
            + parked_wait.as_ref().map_or(0, super::PlannedWait::measure)
            + super::planned_map_run_bytes(&map_runs);
        if store
            .continue_execution(
                &execution_arn,
                planned.opening,
                planned.pending,
                extra_bytes,
                planned.stopped_at,
            )
            .is_err()
        {
            return false;
        }
        // The continuation is waiting again — the state's `Retry` made its call for the
        // next attempt, which handed out a token of its own, or a state after a `Catch`
        // scheduled an activity task — or it parked at a job: each is recorded exactly as
        // the first one was.
        if let Some(waiting) = waiting {
            store.schedule_activity_task(&execution_arn, waiting);
        }
        let parked_a_job = parked.is_some();
        if let Some(parked) = parked {
            store.park_job(&execution_arn, parked, &region);
        }
        store.append_map_runs(&execution_arn, &machine_arn, map_runs);
        (parked_a_job, parked_wait)
    };
    // A job this continuation parked at is polled until it ends, exactly as the job a
    // request's own walk parks at is.
    if parked_a_job {
        super::run_a_job::watch(
            Arc::clone(engine),
            callback.clone(),
            0,
            execution_arn.clone(),
        );
    }
    // And a `Wait` state it parked at is held until its span runs out, exactly as the
    // wait a request's own walk parks at is ([`super::wait_clock`]).
    if let Some(parked_wait) = held_at {
        super::wait_clock::watch(
            Arc::clone(engine),
            callback.clone(),
            0,
            execution_arn.clone(),
            parked_wait,
        );
    }
    true
}
