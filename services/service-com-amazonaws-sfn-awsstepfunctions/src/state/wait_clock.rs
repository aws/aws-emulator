// SPDX-License-Identifier: CC0-1.0

//! The second half of a `Wait` state: the span it delays the execution for runs out, and
//! the states after it are run THEN.
//!
//! The first half is the walk's: a `Wait` state whose span is still ahead suspends the
//! walk at it ([`super::asl::ParkedWait`]), so the call that planned the run answers with
//! the execution `RUNNING` and no outcome yet. That is where this module takes over.
//!
//! ## Why the states after a wait are not run with the states before it
//!
//! "A `Wait` state delays the state machine from continuing for a specified time." What
//! the states after it do is therefore done at the END of that span, and a `Task` state
//! among them makes its service-integration call there: what the call carries, and what it
//! observes of the service it calls, is the world as the wait left it. A run that computed
//! those states in the call that started it would make their calls at the START of the
//! wait instead — so a definition whose control flow waits for a change made while it
//! waits could never see one, and a workflow that reads, waits and writes back would write
//! before the wait rather than after it.
//!
//! The wait is also not a transition a request can perform: `StartExecution` answers an
//! execution that is running, so the span cannot be waited out inside it. It is handed to
//! a task that outlives the call, exactly as the poll of a run-a-job job is
//! ([`super::run_a_job`]), and that task sleeps until the instant the walk reported and
//! then interprets the continuation.
//!
//! ## What it writes
//!
//! Only what the continuation computed. The events the states after the wait produce are
//! APPENDED to the history the states before it left, and the outcome they reached is owed
//! to the clock one [`super::EXECUTION_SETTLE_DELAY`] later — the same continuation a
//! worker's reply writes ([`super::plan_wait_reply`]), so a `Wait` inside the states after
//! this one parks again and is held exactly as this one was, an activity task after it
//! suspends the execution again, and a job it starts is watched as any other is.
//!
//! An execution that is no longer there to continue — stopped, deleted, or carried to an
//! outcome by a bound of its own — is left exactly as it is: the continuation is refused
//! by the one place a continuation reaches an execution
//! ([`super::ComAmazonawsSfnAwsstepfunctionsState::continue_execution`]), and this task
//! ends.

use std::sync::Arc;
use std::time::Duration;

use smithy_server_runtime::EmbeddedEmulator;

use super::asl;
use super::integration;

/// Hold the execution `execution_arn` at the `Wait` state `parked` names until its span
/// runs out, and run the states after it then.
///
/// Returns at once: the holding is handed to a task of its own, because the call that
/// parked the wait must answer while the execution is still running. The caller hands
/// over its own clone of the in-process callback and the chain depth its request arrived
/// at, so a call a state after the wait makes is made exactly where a call of the states
/// before it was made, and a chain of executions starting executions stays as bounded as
/// it was.
pub fn watch<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    depth: u8,
    execution_arn: String,
    parked: super::PlannedWait,
) {
    ::tokio::spawn(async move {
        hold(&engine, &callback, depth, &execution_arn, parked).await;
    });
}

/// Sleep out each span in turn: the one this was handed, and the one every `Wait` the
/// continuation goes on to park at.
///
/// The span is slept in SLICES of at most one [`super::PARKED_JOB_POLL_INTERVAL`], with
/// the execution looked at between them, so a wait the definition declares — up to the
/// [`asl::MAX_WAIT_SECONDS`] a `Wait` state may ask for — holds this task only while
/// there is still an execution to carry on: one that was stopped, deleted or ended by a
/// bound of its own releases it at the next slice rather than at the end of the span.
///
/// The loop ENDS ON ITS OWN, without anybody calling: a continuation that reached an
/// outcome, suspended at a task of another kind or found the execution gone parks no
/// further wait, and there is then nothing left to hold.
async fn hold<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    depth: u8,
    execution_arn: &str,
    parked: super::PlannedWait,
) {
    let mut parked = parked;
    loop {
        // The span still ahead of the instant the walk reported. Nothing is slept for a
        // wait already behind it, which is what a span this task was handed late is.
        let ahead = parked
            .resumes_at
            .duration_since(super::now())
            .unwrap_or_default();
        if ahead > Duration::ZERO {
            ::tokio::time::sleep(ahead.min(super::PARKED_JOB_POLL_INTERVAL)).await;
            if !still_running(engine, execution_arn) {
                return;
            }
            continue;
        }
        match run_on(engine, callback, depth, execution_arn, &parked).await {
            Some(next) => parked = next,
            None => return,
        }
    }
}

/// Whether the execution `execution_arn` is still one a continuation may carry on: there,
/// and not terminal.
fn still_running<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, execution_arn: &str) -> bool {
    let Some(state) = super::service_state(&**engine) else {
        return false;
    };
    let store = state.lock();
    store
        .executions
        .get(execution_arn)
        .is_some_and(|execution| !execution.is_terminal())
}

/// Run the states after the wait, and report the next `Wait` the continuation parked at.
///
/// `None` when there is nothing left to hold: the continuation reached an outcome or
/// suspended at a task of another kind, the definition no longer declares the state, or
/// the execution is no longer one a continuation may reach.
async fn run_on<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    depth: u8,
    execution_arn: &str,
    parked: &super::PlannedWait,
) -> Option<super::PlannedWait> {
    let state = super::service_state(&**engine)?;
    let now = super::now();
    // What an execution's own KEY owes an outcome the clock has already made due, decided
    // before this pass settles and with no store of this service locked, exactly as the
    // poll of a parked job decides it.
    super::encryption::fail_due_runs_the_key_refuses(&**engine, now);

    // Phase 1, under the lock: read what the continuation needs, and stop if there is
    // nothing left to continue. The lock is RELEASED before the walk, so no other request
    // waits on an interpretation.
    let (definition, input, start_date, machine_arn, context, previous_event_id, entered_at) = {
        let mut store = state.lock();
        store.settle(now);
        let execution = store
            .executions
            .get(execution_arn)
            .filter(|execution| !execution.is_terminal());
        let machine = execution.and_then(|execution| {
            store
                .state_machines
                .get(&execution.state_machine_arn)
                .map(|machine| (execution, machine))
        });
        let (execution, machine) = machine?;
        // The context object (`$$`) the continuation reads is the SAME execution's
        // identity: the wait running out carries this execution on rather than starting
        // one.
        let context = asl::Context {
            execution_arn: execution.arn.clone(),
            execution_name: execution.name.clone(),
            state_machine_arn: machine.arn.clone(),
            state_machine_name: machine.name.clone(),
            role_arn: machine.role_arn.clone(),
            redrive_count: execution.redrive_count,
            express: machine.sm_type == super::STATE_MACHINE_TYPE_EXPRESS,
        };
        // The execution runs the definition it was STARTED on, which for one started
        // through a version ARN is that version's immutable snapshot.
        let definition = execution
            .state_machine_version_arn
            .as_deref()
            .and_then(|version_arn| store.state_machine_versions.get(version_arn))
            .map_or_else(
                || machine.definition.clone(),
                |version| version.definition.clone(),
            );
        (
            definition,
            execution.input.clone(),
            execution.start_date,
            machine.arn.clone(),
            context,
            execution.last_event_id(),
            // The event the PRESERVED history holds the entry of the `Wait` state at,
            // which this continuation does not record again.
            execution.entered_event_id(&parked.state_name),
        )
    };

    let from = asl::RedriveStep {
        state_name: parked.state_name.clone(),
        state_input: parked.state_input.clone(),
    };

    // Phase 2, OUTSIDE the lock: the continuation is interpreted, from the `Wait` state
    // itself. The states after it may be service integrations, so their calls are made
    // through the same seam a request's own walk makes them through — as the execution
    // role the states before the wait acted as.
    let region = super::arn_region(&machine_arn);
    let interpreted = {
        let definition = definition.clone();
        let input = input.clone();
        let region = region.clone();
        let unassumable = super::task_authorization::unassumable_role(
            callback,
            &region,
            &context.role_arn,
            &context.state_machine_arn,
        )
        .await;
        let acting = super::task_authorization::acting(
            engine,
            &region,
            &context.role_arn,
            unassumable,
            // The continuation is run by the CLOCK rather than by a request, so there is
            // no credential store to record a fresh assume in.
            None,
        );
        integration::interpret_through(callback, depth, &region, None, acting, move |tasks| {
            super::plan_wait_reply(
                &definition,
                &input,
                start_date,
                now,
                &context,
                &from,
                entered_at,
                previous_event_id,
                tasks,
            )
        })
        .await
    };
    // The definition no longer declares the state that waited — the state machine was
    // updated under an execution started through an unqualified ARN — so there is nothing
    // to continue it with, and the execution is left exactly as it was.
    let Ok(Some(planned)) = interpreted else {
        return None;
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
    // block alone, because the publication below must not hold the store.
    let (watching, waits) = {
        let mut store = state.lock();
        store.settle(now);
        let waiting = planned.waiting;
        let parked_job = planned.parked;
        let parked_wait = planned.parked_wait;
        let extra_bytes = waiting
            .as_ref()
            .map_or(0, super::PlannedActivityTask::measure)
            + parked_job.as_ref().map_or(0, super::PlannedJob::measure)
            + parked_wait.as_ref().map_or(0, super::PlannedWait::measure)
            + super::planned_map_run_bytes(&planned.map_runs);
        if store
            .continue_execution(
                execution_arn,
                planned.opening,
                planned.pending,
                extra_bytes,
                planned.stopped_at.clone(),
            )
            .is_err()
        {
            return None;
        }
        // The continuation reached an activity task, or parked at a job of its own: each
        // is recorded exactly as the call that started the execution records one.
        if let Some(waiting) = waiting {
            store.schedule_activity_task(execution_arn, waiting);
        }
        let watching = parked_job.is_some();
        if let Some(parked_job) = parked_job {
            store.park_job(execution_arn, parked_job, &region);
        }
        store.append_map_runs(execution_arn, &machine_arn, planned.map_runs);
        (watching, parked_wait)
    };
    // A job the continuation started is polled by a watcher of its own, exactly as a
    // start's is.
    if watching {
        super::run_a_job::watch(
            Arc::clone(engine),
            callback.clone(),
            depth,
            execution_arn.to_string(),
        );
    }
    // The states after the wait may have carried the execution to its terminal status,
    // which is a status change this service publishes. Published with no store of this
    // service locked.
    super::events::publish(engine, callback).await;
    waits
}
