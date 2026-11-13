// SPDX-License-Identifier: CC0-1.0

//! The second half of the run-a-job (`.sync`) integration pattern: the job a `Task`
//! state started is POLLED until the invoked service reports it finished, and what it
//! finished as resolves the task.
//!
//! The first half is the walk's: a `Task` state whose `Resource` asks for the pattern
//! makes the integration's own call, records what came back as the task's submission
//! and PARKS there ([`super::asl::ParkedJob`]), so the execution is `RUNNING` with no
//! outcome yet. That is where this module takes over.
//!
//! ## Why it is not the request that waits
//!
//! The call that started the job has to ANSWER — `StartExecution` reports an
//! execution that is running, not one that has finished — so the job cannot be waited
//! for inside it. The job is also not a transition the clock owes: how long it takes
//! is the invoked service's business, and only that service can say whether it is
//! done. So the poll is work of its own, held by the parked job rather than by any
//! request: [`watch`] hands it to a task that outlives the call, exactly as the work
//! behind an asynchronous submission does, and it ends when the job does — or when
//! the execution it belongs to is stopped, deleted or runs out of the time its task
//! was allowed ([`super::JobTask::times_out_at`], applied by
//! [`super::ComAmazonawsSfnAwsstepfunctionsState::settle`] whether or not this is
//! running).
//!
//! ## What it writes
//!
//! Only what the invoked service answered. The job's status, its report and the
//! reason a failed one failed are read from that service's own reporting call
//! ([`super::integration::RunAJob`]), and the task's result IS that report: a job that
//! did its work makes the task succeed with it, and one that ended without doing it
//! makes the task fail, which the state's `Catch` then answers for. The continuation
//! is the SAME walk a worker's reply continues an execution with
//! ([`super::plan_job_reply`]), so the states after the task run, their own
//! integration calls are made, and a second parked job is watched exactly as the first
//! was.
//!
//! ## Who it polls as
//!
//! The execution role, as the starting call was made as: the reporting call is the
//! execution's own call, so the policies in force on that role decide whether it is
//! made ([`super::task_authorization::job_report_denial`]). A role that may not report
//! the job leaves nothing to read about it, and the task was already submitted, so it
//! fails with that refusal rather than with a status nothing answered.

use std::sync::Arc;

use smithy_server_runtime::EmbeddedEmulator;

use super::asl;
use super::integration;

/// Poll the job the execution `execution_arn` is parked at until it ends, and resolve
/// the task that started it.
///
/// Returns at once: the polling is handed to a task of its own, because the call that
/// parked the job must answer while the job is still running. A call that parks a job
/// — a start, a redrive, or the continuation of either — calls this with its own
/// clone of the in-process callback, so the reporting call is made exactly where the
/// starting call was made, and with the chain depth that call arrived at, so a chain
/// of executions starting executions is bounded as any other chain of calls is.
pub fn watch<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    depth: u8,
    execution_arn: String,
) {
    ::tokio::spawn(async move {
        poll(&engine, &callback, depth, &execution_arn).await;
    });
}

/// Ask the job's own service how it is doing, every
/// [`super::PARKED_JOB_POLL_INTERVAL`], until there is no job to ask about any more.
///
/// The loop ENDS ON ITS OWN, without anybody calling: each pass settles the store
/// first, which is what applies the time the task was allowed
/// ([`super::JobTask::times_out_at`]) and removes the job record when that span has
/// run out — so the longest this can run is that span plus one interval, whatever the
/// invoked service does. It also ends as soon as the job is resolved, the execution is
/// stopped, or the store is gone.
async fn poll<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    depth: u8,
    execution_arn: &str,
) {
    loop {
        ::tokio::time::sleep(super::PARKED_JOB_POLL_INTERVAL).await;
        let Some(state) = super::service_state(&**engine) else {
            return;
        };
        // The job this execution is parked at, read under ONE lock that settles
        // first: a job whose execution was stopped, or whose task ran out of its
        // time, is gone by the time it is read, and there is then nothing to poll.
        // The execution role is read with it, because the call that reports the job is
        // made as that role — the role of the configuration this execution RAN, read
        // at the moment of the call, exactly as the starting call's was — together with
        // the state machine's own name, which is what the session the role is assumed
        // under is named after.
        // What the execution's own KEY owes an outcome the clock has already made due,
        // decided before this pass settles and with no store of this service locked
        // ([`super::encryption::fail_due_runs_the_key_refuses`]): the decision is the
        // key's and the clock's together, so it is taken wherever a due outcome is
        // materialised rather than only where a caller reaches it.
        super::encryption::fail_due_runs_the_key_refuses(&**engine, super::now());
        let (task, role_arn, machine_name) = {
            let mut store = state.lock();
            store.settle(super::now());
            let task = store.job_tasks.get(execution_arn).cloned();
            let machine = store
                .executions
                .get(execution_arn)
                .and_then(|execution| store.state_machines.get(&execution.state_machine_arn));
            let role_arn = machine.map_or_else(String::new, |machine| machine.role_arn.clone());
            let machine_name = machine.map_or_else(String::new, |machine| machine.name.clone());
            (task, role_arn, machine_name)
        };
        let Some(task) = task else {
            return;
        };

        let (call, request) =
            integration::describing(task.api, task.job, &task.job_id, &task.naming);
        // The poll is one of the execution's own calls, so the policies in force on
        // its role decide whether it is made: a role that may not report the job
        // cannot be told what the job did, and the task it belongs to has already been
        // submitted — so the task fails with that refusal instead of with a status
        // nothing answered. Decided with no store of this service locked.
        let denied = task
            .acts_on
            .as_deref()
            .and_then(|resource| {
                super::task_authorization::job_report_denial(
                    &**engine,
                    &role_arn,
                    &machine_name,
                    &call,
                    resource,
                )
            })
            .map(|refused| asl::Failure {
                error: Some(refused.error),
                cause: Some(refused.cause),
            });
        if let Some(refusal) = denied {
            resolve(engine, callback, depth, execution_arn, &task, Err(refusal)).await;
            continue;
        }
        let reported = integration::call_api(callback, &task.region, &call, &request).await;
        let outcome = match reported {
            Ok(report) => match task.job.outcome_of(&report, task.parsed_output) {
                // The job is still doing its work, so the task is not finished and
                // the execution is still running: nothing is written.
                None => continue,
                Some(Ok(report)) => Ok(integration::task_result(&report)),
                // The job ended without doing its work, which is the task's own
                // failure: the ASL error name for a failed task, carrying the reason
                // the invoked service gave.
                Some(Err(reason)) => Err(asl::Failure {
                    error: Some(integration::TASK_FAILED_ERROR.to_string()),
                    cause: Some(reason),
                }),
            },
            // The call that reports the job was refused, or could not be made at all:
            // what the job is doing cannot be known, so the task fails with what the
            // service answered rather than with a status nothing reported.
            Err(integration::Failed { error, cause }) => Err(asl::Failure {
                error: Some(error),
                cause: Some(cause),
            }),
        };

        // Resolved: the continuation of this execution is interpreted and recorded.
        // The loop goes round again rather than returning, because that continuation
        // may itself have parked at another job — the next pass finds it, and finds
        // nothing to poll when there is none.
        resolve(engine, callback, depth, execution_arn, &task, outcome).await;
    }
}

/// Record what the job ENDED as: the execution carries on from the state that started
/// it, with that outcome as the state's own.
async fn resolve<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    depth: u8,
    execution_arn: &str,
    task: &super::JobTask,
    outcome: Result<::serde_json::Value, asl::Failure>,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let now = super::now();
    let failed = outcome.is_err();
    let reply = asl::JobReply {
        state_name: task.state_name.clone(),
        state_input: task.state_input.clone(),
        // The concurrent states that state is nested inside, as the parked record
        // kept them: the continuation re-enters them to reach it.
        frames: task.frames.clone(),
        outcome,
    };

    // Phase 1, under the lock: read what the continuation needs, and stop if there is
    // nothing left to continue. The lock is RELEASED before the walk, so no other
    // request waits on an interpretation.
    let Some((definition, input, start_date, machine_arn, context, previous_event_id)) = ({
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
        machine.map(|(execution, machine)| {
            // The context object (`$$`) the continuation reads is the SAME
            // execution's identity: the job's outcome continues this execution
            // rather than starting one.
            let context = asl::Context {
                execution_arn: execution.arn.clone(),
                execution_name: execution.name.clone(),
                state_machine_arn: machine.arn.clone(),
                state_machine_name: machine.name.clone(),
                role_arn: machine.role_arn.clone(),
                redrive_count: execution.redrive_count,
                // The workflow type the continuation is interpreted under. A parked
                // job belongs to a STANDARD execution — the run-a-job pattern is the
                // one EXPRESS does not serve — and the type is read off the record
                // rather than assumed from that.
                express: machine.sm_type == super::STATE_MACHINE_TYPE_EXPRESS,
            };
            // The execution runs the definition it was STARTED on, which for one
            // started through a version ARN is that version's immutable snapshot.
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
            )
        })
    }) else {
        return;
    };

    // Phase 2, OUTSIDE the lock: the continuation is interpreted, from the state that
    // started the job. The states after it may themselves be service integrations, so
    // their calls are made through the same seam a request's own walk makes them
    // through.
    let region = super::arn_region(&machine_arn);
    let interpreted = {
        let definition = definition.clone();
        let input = input.clone();
        let region = region.clone();
        // The states after the job call as the execution role the states before it
        // did, which is the role the state machine carries — assumed again for this
        // continuation, so a role that cannot be assumed fails the task.
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
            &context.state_machine_name,
            unassumable,
        );
        integration::interpret_through(callback, depth, &region, acting, move |tasks| {
            super::plan_job_reply(
                &definition,
                &input,
                start_date,
                now,
                &context,
                &reply,
                previous_event_id,
                tasks,
            )
        })
        .await
    };
    // The definition no longer declares the state that started the job — the state
    // machine was updated under an execution started through an unqualified ARN — so
    // there is nothing to continue it with, and the execution is left exactly as it
    // was rather than carried to an outcome no definition describes.
    let Ok(Some(planned)) = interpreted else {
        return;
    };
    // A job this continuation STARTED and then abandoned is stopped through its own
    // integration, as a start's own is: no job is left running behind a task that
    // will never finish. Done with no store of this service locked.
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
    {
        let mut store = state.lock();
        store.settle(now);
        let waiting = planned.waiting;
        let parked = planned.parked;
        let extra_bytes = waiting
            .as_ref()
            .map_or(0, super::PlannedActivityTask::measure)
            + parked.as_ref().map_or(0, super::PlannedJob::measure)
            + super::planned_map_run_bytes(&planned.map_runs);
        // The attempt STOPPED at the state that started the job when that job failed and
        // no `Catch` of the state carried the execution on, which is the step a later
        // `RedriveExecution` continues from.
        let stopped_at_task = failed
            && planned
                .pending
                .as_ref()
                .is_some_and(|pending| pending.status == super::EXECUTION_STATUS_FAILED);
        let walked_stopped_at = planned.stopped_at.clone();
        if store
            .resolve_job_task(
                execution_arn,
                planned.opening,
                planned.pending,
                extra_bytes,
                walked_stopped_at,
                stopped_at_task,
            )
            .is_err()
        {
            return;
        }
        // The continuation reached an activity task, or parked at another job of its own:
        // each is recorded exactly as the first one was.
        if let Some(waiting) = waiting {
            store.schedule_activity_task(execution_arn, waiting);
        }
        if let Some(parked) = parked {
            store.park_job(execution_arn, parked, &region);
        }
        store.append_map_runs(execution_arn, &machine_arn, planned.map_runs);
    }
    // The job's outcome may have carried the execution to its terminal status, which
    // is a status change this service publishes onto the account's default event bus.
    // Published with no store of this service locked.
    super::events::publish(engine, callback).await;
}
