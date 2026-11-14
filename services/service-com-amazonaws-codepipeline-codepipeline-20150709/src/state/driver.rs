// SPDX-License-Identifier: CC0-1.0

//! The driver that MOVES a pipeline execution forward: what is due now, and the real
//! work that materialising it performs.
//!
//! [`advance`] is the entry point a request takes, and [`advance_pipeline`] the one the
//! clock takes; both run the same walk. It alternates between the store (under the
//! lock, planning the next due step) and the emulated services an action's work runs
//! through (without the lock: S3 for artifact I/O, KMS for the data key an artifact
//! store's customer managed key encrypts an object with, CodeCommit for a source
//! revision, Lambda for an invoke action — each over the framework's in-process
//! callback), so no step is ever reported as done without the work having been
//! performed.
//!
//! See [`super::execution`] for the data model and for why a reporting operation is
//! allowed to run this at all (every step carries the instant the CLOCK made it due),
//! and [`super::clock`] for how the steps a client makes no further call for are
//! materialised. Every status a step writes is handed to [`super::events`], which
//! publishes it to EventBridge: the diff is taken once per step rather than at each
//! transition site, so no transition can be made without its event.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::time::SystemTime;

use super::execution::{
    is_terminal_status, transition_held, ArtifactRef, EnvironmentOverrides, Job, JobOutcome,
    ACTION_STEP, CLAIM_TIMEOUT, FAILURE_CONFIGURATION_ERROR, FAILURE_JOB_FAILED,
    FAILURE_REVISION_UNAVAILABLE, MAX_ADVANCE_STEPS, MAX_ARCHIVE_ENTRIES, MAX_EXTRACTED_BYTES,
    MAX_EXTRACTED_ENTRY_BYTES,
    MAX_SOURCE_BYTES, MAX_SOURCE_FILES,
    STATUS_FAILED, STATUS_STOPPED, STATUS_STOPPING, STATUS_SUCCEEDED, STATUS_SUPERSEDED,
    TRANSITION_INBOUND, TRANSITION_OUTBOUND,
};
use super::rules::{
    ConditionOutcome, CONDITION_TYPE_BEFORE_ENTRY, CONDITION_TYPE_ON_SUCCESS, RESULT_FAIL,
    RESULT_SKIP,
};
use super::{
    random_uuid_v4, ComAmazonawsCodepipelineCodepipeline20150709State, PipelineExecutionRecord,
};
use crate::ops::types::{Condition, SourceRevision};

/// The work a CodeBuild action's build is started and settled by, in
/// `state/driver/build_action.rs`.
mod build_action;
/// The work an Amazon ECR source action's image is resolved and packed by, in
/// `state/driver/image_source.rs`.
mod image_source;
pub use image_source::{resolved_ecr_source_revisions, StartedSourceRevision};

/// The `ActionCategory` of an action that waits for a human decision.
///
/// Public to the crate: it is the category `PutApprovalResult` answers an action of, so
/// the handler that judges whether the addressed action is an approval action at all
/// reads the same value this engine publishes a token for.
pub const CATEGORY_APPROVAL: &str = "Approval";

/// The `ActionCategory` values whose S3 provider this engine performs.
const CATEGORY_SOURCE: &str = "Source";
/// See [`CATEGORY_SOURCE`].
const CATEGORY_DEPLOY: &str = "Deploy";

/// The `provider` of the action types this engine performs through S3.
const PROVIDER_S3: &str = "S3";

/// The `provider` of a source action whose revisions live in a CodeCommit repository.
const PROVIDER_CODE_COMMIT: &str = "CodeCommit";

/// The `provider` of a source action whose revisions are the images of an Amazon ECR
/// repository.
const PROVIDER_ECR: &str = "ECR";

/// The `ActionCategory` of an action that builds its input.
const CATEGORY_BUILD: &str = "Build";

/// The `provider` of an action whose work is a build of an AWS CodeBuild project.
const PROVIDER_CODE_BUILD: &str = "CodeBuild";

/// The `provider` of an action whose work is an invocation of a Lambda function.
const PROVIDER_LAMBDA: &str = "Lambda";

/// The configuration key naming the repository a CodeCommit source action reads
/// (CodePipeline's action-structure reference for the CodeCommit source action).
const CONFIG_REPOSITORY_NAME: &str = "RepositoryName";

/// The configuration key naming the branch a CodeCommit source action reads.
const CONFIG_BRANCH_NAME: &str = "BranchName";

/// The configuration key naming the function a Lambda action invokes.
const CONFIG_FUNCTION_NAME: &str = "FunctionName";

/// The configuration key naming the tag an Amazon ECR source action sources the image of
/// (CodePipeline's action-structure reference for the Amazon ECR source action).
const CONFIG_IMAGE_TAG: &str = "ImageTag";

/// The tag an Amazon ECR source action sources when its declaration names none: the
/// action's `ImageTag` is "optional" and "if not specified, the value defaults to
/// `latest`".
const DEFAULT_IMAGE_TAG: &str = "latest";

/// The configuration key naming the project a CodeBuild action builds (that same
/// reference, for the CodeBuild build and test actions).
const CONFIG_PROJECT_NAME: &str = "ProjectName";

/// The configuration key carrying the environment variables a CodeBuild action overrides
/// the project's with: "a set of environment variables" as a JSON ARRAY of
/// `{name, value, type}` objects, in the action's own configuration.
const CONFIG_ENVIRONMENT_VARIABLES: &str = "EnvironmentVariables";

/// The configuration key carrying the text a Lambda action's event passes through.
const CONFIG_USER_PARAMETERS: &str = "UserParameters";

/// The configuration key naming the customer managed key an S3 deploy action encrypts
/// the object it delivers under (that same action-structure reference, for the Amazon S3
/// deploy action).
const CONFIG_KMS_ENCRYPTION_KEY_ARN: &str = "KMSEncryptionKeyARN";

/// `ActionExecutionStatus` values.
const ACTION_IN_PROGRESS: &str = "InProgress";
/// See [`ACTION_IN_PROGRESS`].
const ACTION_SUCCEEDED: &str = "Succeeded";
/// See [`ACTION_IN_PROGRESS`].
const ACTION_FAILED: &str = "Failed";

/// `StageExecutionStatus` values.
const STAGE_IN_PROGRESS: &str = "InProgress";
/// See [`STAGE_IN_PROGRESS`].
const STAGE_SUCCEEDED: &str = "Succeeded";
/// See [`STAGE_IN_PROGRESS`].
const STAGE_FAILED: &str = "Failed";
/// See [`STAGE_IN_PROGRESS`]. The status of a stage an entry condition's `SKIP` result
/// kept the execution out of.
const STAGE_SKIPPED: &str = "Skipped";

/// Materialise every step of `pipeline_name`'s executions the clock has already made
/// due, performing each step's real work, and keep this store's clock running for the
/// steps that are still to fall due.
///
/// Returns once nothing more is due. Bounded by [`MAX_ADVANCE_STEPS`]: the state is
/// durable, so the next call continues where this one stopped — and the clock
/// ([`super::clock`]) is what continues it when no further call comes.
pub async fn advance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    pipeline_name: &str,
) {
    let region = super::request_region(&ctx.parts().headers);
    advance_in_region(engine, ctx.callback(), &region, pipeline_name).await;
}

/// [`advance`] for a caller that states the Region itself rather than having it read
/// off a signed request's credential scope.
///
/// The trigger plane a webhook publishes is reached UNSIGNED — a third-party provider
/// POSTs the payload it was configured to send and carries no AWS credential — so the
/// Region the execution it starts is driven in is the one the webhook was defined in
/// (`crate::service::webhook_trigger`), which is the Region that execution's artifact
/// store and state-change events belong to.
pub async fn advance_in_region<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    pipeline_name: &str,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    advance_pipeline(&state, callback, region, pipeline_name).await;
    super::clock::drive(&state, callback, region);
}

/// The body of [`advance`] for ONE pipeline, over the store and the callback rather than
/// over a request: the clock drives the same steps with no request to borrow them from.
///
/// Answers how many units of real WORK this call performed, which is what tells the clock
/// whether the pipeline is moving or parked.
pub(super) async fn advance_pipeline(
    state: &::engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    pipeline_name: &str,
) -> usize {
    let mut performed = 0;
    for _ in 0..MAX_ADVANCE_STEPS {
        let planned = {
            // ONE reading of the clock per critical section, so a status the store
            // records and the event that reports it carry the same instant.
            let now = SystemTime::now();
            let mut store = state.lock();
            let planned = plan(&mut store, pipeline_name, now);
            // Every status the planning step just wrote — an execution entering a stage,
            // a stage rolling up, a stop settling — owes an event, and it is recorded
            // here rather than at each of those sites so none of them can forget.
            super::events::record_state_changes(&mut store, region, now);
            planned
        };
        super::events::publish(state, callback, region).await;
        let Some(job) = planned else { break };
        let outcome = run_job(callback, region, &job).await;
        {
            let now = SystemTime::now();
            let mut store = state.lock();
            record_outcome(&mut store, &job, outcome, now);
            super::events::record_state_changes(&mut store, region, now);
        }
        super::events::publish(state, callback, region).await;
        performed += 1;
    }
    performed
}

/// Apply every transition `pipeline_name` is owed at `now`, and claim the next unit of
/// real work if one is due.
pub fn plan(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    pipeline_name: &str,
    now: SystemTime,
) -> Option<Job> {
    // An execution a later one has caught up with is superseded before anything moves,
    // so the stage ahead of them takes the newest one.
    supersede_waiting(store, pipeline_name, now);
    // Which execution holds each stage of the pipeline, before anything moves: one stage
    // runs ONE execution at a time, so this is what decides which execution may enter a
    // stage next and which of the executions waiting to start may enter the pipeline.
    let mut claims = stage_claims(store, pipeline_name);
    admit_queued(store, pipeline_name, &mut claims);
    // The pipelines and the executions are DISJOINT fields of the store, so the
    // declaration an execution runs is borrowed for reading while that execution's
    // record is written — no clone of the declaration is made per step.
    let ComAmazonawsCodepipelineCodepipeline20150709State {
        pipelines,
        executions,
        executions_by_pipeline,
        jobs,
        rule_executions,
        ..
    } = store;
    let pipeline = pipelines.get(pipeline_name)?;
    let disabled = super::execution::disabled_transitions(pipeline);
    let ids = executions_by_pipeline
        .get(pipeline_name)
        .cloned()
        .unwrap_or_default();
    for id in ids {
        let Some(record) = executions.get_mut(&id) else {
            continue;
        };
        // The execution runs the pipeline VERSION it pinned at start, so a later
        // `UpdatePipeline` does not rewrite a running execution.
        let Some(declaration) = pipeline.declaration_at(Some(record.pipeline_version)) else {
            continue;
        };
        // The job queue is a THIRD disjoint field of the store, so an execution that
        // reaches a custom action can be written while its job is queued.
        let mut context = StepContext {
            declaration,
            pipeline_arn: &pipeline.arn,
            jobs: &mut *jobs,
            rules: &mut *rule_executions,
            disabled: &disabled,
            claims: &mut claims,
        };
        if let Some(job) = step_execution(record, &mut context, now) {
            return Some(job);
        }
    }
    None
}

/// What one step of an execution needs besides the execution's own record: the
/// pipeline it belongs to, and the custom-action job queue it may have to write.
///
/// Each field is borrowed from a DIFFERENT field of the store, which is what lets a
/// step read the pinned declaration and queue a job while it writes the execution.
struct StepContext<'a> {
    /// The pipeline declaration version the execution pinned at start.
    declaration: &'a crate::ops::types::PipelineDeclaration,
    /// That pipeline's ARN, which a custom action's job reports to its worker.
    pipeline_arn: &'a str,
    /// The jobs waiting for custom actions' workers: written when an execution
    /// reaches an action CodePipeline does not run itself.
    jobs: &'a mut super::JobQueue,
    /// The rule executions the stage-condition plane records: written when an
    /// execution reaches a stage whose conditions declare rules.
    rules: &'a mut super::RuleExecutionLog,
    /// The `(stage, StageTransitionType)` pairs a caller has disabled.
    disabled: &'a ::std::collections::BTreeSet<(String, String)>,
    /// Which execution holds each stage of the pipeline: a stage runs one execution at a
    /// time, so an execution may enter a stage only where this names it or nobody.
    claims: &'a mut BTreeMap<String, String>,
}

/// Supersede the executions a later one has caught up with.
///
/// `PipelineDeclaration$executionMode` documents `SUPERSEDED` — the default — as the mode
/// in which a more recent execution supersedes one that has not entered the stage yet. So
/// at most one execution waits at a stage boundary: the newest that reached it, with the
/// ones it caught up with moved to the terminal `Superseded` status, whether they were
/// waiting at the pipeline's first stage or at a boundary further in (behind a disabled
/// transition, or behind the execution that holds the stage). `QUEUED` keeps every waiting
/// execution — they enter in the order they arrived — and a `PARALLEL` execution waits for
/// no other one at all, so neither mode supersedes anything here.
///
/// An execution that is RUNNING a stage is not waiting for anything and is never
/// superseded, and neither is a rollback, which re-runs one stage of an execution that
/// already ran rather than taking the pipeline's next turn.
fn supersede_waiting(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    pipeline_name: &str,
    now: SystemTime,
) {
    let ids = store
        .executions_by_pipeline
        .get(pipeline_name)
        .cloned()
        .unwrap_or_default();
    // Where each execution is waiting, oldest first.
    let mut waiting: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in &ids {
        let Some(record) = store.executions.get(id) else {
            continue;
        };
        if record.execution_mode != super::DEFAULT_EXECUTION_MODE
            || record.execution_type != super::EXECUTION_TYPE_STANDARD
            || record.status != super::STATUS_IN_PROGRESS
            || record.stages.iter().any(|run| run.is_in_progress())
        {
            continue;
        }
        if let Some(run) = record.stages.iter().find(|run| run.status.is_none()) {
            waiting.entry(run.name.clone()).or_default().push(id.clone());
        }
    }
    for at_boundary in waiting.into_values() {
        // The newest execution at the boundary is the one that enters the stage.
        let superseded = at_boundary.len().saturating_sub(1);
        for id in at_boundary.iter().take(superseded) {
            if let Some(record) = store.executions.get_mut(id) {
                record.status = STATUS_SUPERSEDED.to_string();
                record.status_summary =
                    Some("superseded by a later execution of the pipeline".to_string());
                record.last_update_time = now;
            }
        }
    }
}

/// Which execution of `pipeline_name` holds each of its stages.
///
/// CodePipeline runs ONE execution per stage: the execution that entered a stage holds it
/// until its run of that stage finishes, and the next execution to reach that boundary
/// waits there — it is held OUT of that one stage, not out of the pipeline, so every
/// earlier stage of it still runs. An execution occupying no stage holds the boundary it
/// is waiting at, so the executions enter a stage in the order they reached it; an
/// execution that has not entered the pipeline at all holds nothing yet, which is what
/// lets [`admit_queued`] decide whether it may.
///
/// `PARALLEL` executions are independent of one another
/// (`PipelineDeclaration$executionMode`), so one neither holds a stage against another
/// execution nor waits for one.
fn stage_claims(
    store: &ComAmazonawsCodepipelineCodepipeline20150709State,
    pipeline_name: &str,
) -> BTreeMap<String, String> {
    let mut claims: BTreeMap<String, String> = BTreeMap::new();
    for id in store
        .executions_by_pipeline
        .get(pipeline_name)
        .into_iter()
        .flatten()
    {
        let Some(record) = store.executions.get(id) else {
            continue;
        };
        if record.execution_mode == super::EXECUTION_MODE_PARALLEL
            || is_terminal_status(&record.status)
        {
            continue;
        }
        let held = record
            .stages
            .iter()
            .find(|run| run.is_in_progress())
            .or_else(|| {
                (!record.queued)
                    .then(|| record.stages.iter().find(|run| run.status.is_none()))
                    .flatten()
            });
        if let Some(run) = held {
            // The executions are listed oldest first, so the one that reached the stage
            // first is the one that holds it.
            claims.entry(run.name.clone()).or_insert_with(|| id.clone());
        }
    }
    claims
}

/// Let the executions waiting to enter the pipeline start, as the stage they would enter
/// allows.
///
/// An execution waits only for the STAGE it would enter, never for the whole pipeline to
/// empty: the stage it starts in takes one execution at a time ([`stage_claims`]), so an
/// execution waits exactly while another one holds that boundary — whatever is happening
/// in the stages beyond it, and however long an execution parked there (at a manual
/// approval, at a custom action's job, at a disabled transition) stays. In `PARALLEL` mode
/// every execution runs on its own (`PipelineDeclaration$executionMode`), so it waits for
/// nothing.
fn admit_queued(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    pipeline_name: &str,
    claims: &mut BTreeMap<String, String>,
) {
    let ids = store
        .executions_by_pipeline
        .get(pipeline_name)
        .cloned()
        .unwrap_or_default();
    for id in ids {
        let Some(record) = store.executions.get_mut(&id) else {
            continue;
        };
        if !record.queued || is_terminal_status(&record.status) {
            continue;
        }
        let parallel = record.execution_mode == super::EXECUTION_MODE_PARALLEL;
        // The stage this execution would enter first. Its own plan says which that is: a
        // rollback's plan holds one stage, not the pipeline's first.
        let entering = record
            .stages
            .iter()
            .find(|run| run.status.is_none())
            .map(|run| run.name.clone());
        let Some(entering) = entering else {
            // A plan with no stage left to enter waits for nothing.
            record.queued = false;
            continue;
        };
        if parallel {
            record.queued = false;
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(free) = claims.entry(entering) {
            record.queued = false;
            // It is now the execution waiting at that boundary, so the next one in the
            // queue waits behind it rather than beside it.
            free.insert(id.clone());
        }
    }
}

/// Move one execution forward, returning the work it claimed, if any.
fn step_execution(
    record: &mut PipelineExecutionRecord,
    context: &mut StepContext<'_>,
    now: SystemTime,
) -> Option<Job> {
    let disabled = context.disabled;
    if record.queued || is_terminal_status(&record.status) {
        return None;
    }

    // A graceful stop lets the work already in flight finish and then settles: nothing
    // further is started, and every action still waiting is abandoned.
    if record.status == STATUS_STOPPING {
        let in_flight = record
            .stages
            .iter()
            .flat_map(|stage| &stage.actions)
            .any(|action| action.claimed.is_some());
        if !in_flight {
            settle_stopped(record, now);
        }
        return None;
    }

    for index in 0..record.stages.len() {
        match record.stages[index].status.as_deref() {
            // A stage the execution is past: it either ran to success or an entry
            // condition that was met kept the execution out of it with `SKIP`. Either
            // way the execution carries on, so the only thing that holds it here is
            // the outbound transition.
            Some(STAGE_SUCCEEDED | STAGE_SKIPPED) => {
                // "Disabling the outbound transition" holds the execution IN the stage
                // it has finished, so it never enters the stage after it.
                if transition_held(
                    disabled,
                    &record.stages[index].name,
                    TRANSITION_OUTBOUND,
                ) && index + 1 < record.stages.len()
                {
                    return None;
                }
                continue;
            }
            // A stage that failed or was stopped ends the execution; the rollup below
            // has already recorded that, so there is nothing left to do.
            Some(_) if record.stages[index].is_terminal() => return None,
            Some(_) => {}
            None => {
                // One stage runs ONE execution at a time: an execution that reaches a
                // stage another execution is still in waits at that boundary — and
                // "disabling the inbound transition" holds an execution OUT of the stage
                // the same way, so the execution stays InProgress and the stage is not
                // entered.
                let held = context
                    .claims
                    .get(&record.stages[index].name)
                    .is_some_and(|holder| *holder != record.pipeline_execution_id)
                    || transition_held(disabled, &record.stages[index].name, TRANSITION_INBOUND);
                if held {
                    // Being let in later is a step of its own, so the boundary records
                    // when the execution may enter once it is released.
                    record.stages[index].entry_due = Some(now + ACTION_STEP);
                    return None;
                }
                // The release of a boundary that DID hold this execution is that
                // clock-owed step: until it falls due the stage still reports the run
                // that left it.
                if record.stages[index]
                    .entry_due
                    .is_some_and(|due| due > now)
                {
                    return None;
                }
                // A stage's ENTRY conditions are evaluated at the instant the
                // execution would enter it, and a condition that was MET decides
                // whether it does.
                match settle_conditions(record, context, index, CONDITION_TYPE_BEFORE_ENTRY, now) {
                    ConditionGate::Proceed => {}
                    ConditionGate::Skipped => {
                        // A skipped stage is one the execution is PAST, so the same
                        // outbound hold the arm above applies decides whether it may
                        // leave it for the next stage.
                        if transition_held(
                            disabled,
                            &record.stages[index].name,
                            TRANSITION_OUTBOUND,
                        ) && index + 1 < record.stages.len()
                        {
                            return None;
                        }
                        continue;
                    }
                    ConditionGate::Failed => return None,
                }
                record.stages[index].status = Some(STAGE_IN_PROGRESS.to_string());
                record.last_update_time = now;
                context.claims.insert(
                    record.stages[index].name.clone(),
                    record.pipeline_execution_id.clone(),
                );
            }
        }
        return step_stage(record, context, index, now);
    }

    // Every stage succeeded or was skipped, so the execution has.
    record.status = STATUS_SUCCEEDED.to_string();
    record.last_update_time = now;
    None
}

/// What a stage's condition set decided about the execution at that stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionGate {
    /// No condition was met — or the stage declares none at this site — so the
    /// execution carries on.
    Proceed,
    /// The stage is skipped: an entry condition that was met declared `SKIP`, so the
    /// execution moves to the next stage without entering this one.
    Skipped,
    /// The stage failed, and the execution with it: the record already carries both.
    Failed,
}

/// Evaluate one condition site of one stage and apply what its outcome decides.
///
/// Evaluating the rules and recording them is the condition plane's
/// ([`super::rules`]); what belongs here is the EXECUTION's part: a condition that was
/// MET is acted on through the `Result` it declares, and a `Result` this emulator would
/// have to perform work for (restarting a stage, rolling one back) is not claimed as
/// done — the stage fails with the reason, exactly as an action whose work this
/// emulator cannot perform does.
fn settle_conditions(
    record: &mut PipelineExecutionRecord,
    context: &mut StepContext<'_>,
    stage: usize,
    condition_type: &str,
    now: SystemTime,
) -> ConditionGate {
    // Cloned out of the pinned declaration first, so the conditions can be evaluated
    // while the execution's own record is written.
    let conditions: Vec<Condition> = record
        .stages
        .get(stage)
        .and_then(|run| super::execution::declared_stage(context.declaration, run))
        .and_then(|declared| match condition_type {
            CONDITION_TYPE_BEFORE_ENTRY => declared
                .before_entry
                .as_ref()
                .map(|before_entry| before_entry.conditions.clone()),
            CONDITION_TYPE_ON_SUCCESS => declared
                .on_success
                .as_ref()
                .map(|on_success| on_success.conditions.clone()),
            _ => None,
        })
        .unwrap_or_default();
    if conditions.is_empty() {
        return ConditionGate::Proceed;
    }
    // A site a caller OVERRODE is not evaluated again for this run of the stage: the
    // override decided the gate, so the execution carries on past it.
    if record.stages[stage].overridden.contains(condition_type) {
        return ConditionGate::Proceed;
    }

    let stage_name = record.stages[stage].name.clone();
    let variables = record.resolution_variables();
    let outcome = super::rules::evaluate_conditions(
        context.rules,
        &record.pipeline_name,
        &record.pipeline_execution_id,
        record.pipeline_version,
        &stage_name,
        condition_type,
        &conditions,
        &variables,
        now,
    );
    record.last_update_time = now;
    let ConditionOutcome::Met { result, detail } = outcome else {
        return ConditionGate::Proceed;
    };

    if result == RESULT_SKIP && condition_type == CONDITION_TYPE_BEFORE_ENTRY {
        record.stages[stage].status = Some(STAGE_SKIPPED.to_string());
        return ConditionGate::Skipped;
    }
    let detail = if result == RESULT_FAIL {
        detail
    } else if result == RESULT_SKIP {
        format!(
            "{detail}; the condition declares the result {result}, which applies to a \
             stage that has not run, so the stage is failed instead"
        )
    } else {
        format!(
            "{detail}; the condition declares the result {result}, and this emulator \
             starts no such remediation of a stage on its own, so the stage is failed \
             instead"
        )
    };
    for action in &mut record.stages[stage].actions {
        if action.is_in_progress() {
            action.status = Some("Abandoned".to_string());
            action.token = None;
            action.last_status_change = now;
        }
    }
    record.stages[stage].status = Some(STAGE_FAILED.to_string());
    // The site whose met condition decided this run of the stage, which is what an
    // override of the gate acts on.
    record.stages[stage].blocked_by = Some(condition_type.to_string());
    record.status = STATUS_FAILED.to_string();
    record.status_summary = Some(format!(
        "the {condition_type} condition of stage {stage_name} was met: {detail}"
    ));
    ConditionGate::Failed
}

/// Move one stage of one execution forward.
fn step_stage(
    record: &mut PipelineExecutionRecord,
    context: &mut StepContext<'_>,
    stage: usize,
    now: SystemTime,
) -> Option<Job> {
    // The actions are grouped by run order in ONE pass, so no part of this walk
    // rescans the stage per run order.
    let mut groups: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (index, action) in record.stages[stage].actions.iter().enumerate() {
        groups.entry(action.run_order).or_default().push(index);
    }

    for positions in groups.into_values() {
        let complete = positions
            .iter()
            .all(|index| record.stages[stage].actions[*index].is_terminal());
        if complete {
            let failed = positions.iter().find(|index| {
                record.stages[stage].actions[**index].status.as_deref() == Some(ACTION_FAILED)
            });
            if let Some(index) = failed {
                let action = &record.stages[stage].actions[*index];
                let summary = format!(
                    "action {} in stage {} failed: {}",
                    action.name,
                    record.stages[stage].name,
                    action
                        .error
                        .as_ref()
                        .map(|(_, message)| message.clone())
                        .unwrap_or_else(|| "no detail reported".to_string())
                );
                record.stages[stage].status = Some(STAGE_FAILED.to_string());
                record.status = STATUS_FAILED.to_string();
                record.status_summary = Some(summary);
                record.last_update_time = now;
                return None;
            }
            // This run order is done; the next one becomes due below.
            continue;
        }

        // Every action of ONE run order runs CONCURRENTLY, so they all become due at the
        // same instant: a span after the run order before them finished (or after the
        // execution started, for the first one). The instant is derived from the newest
        // completed action of this execution rather than from the call that happens to
        // observe it, and it is assigned to the whole run order in one pass — so a
        // sibling finishing its work never pushes the rest of its run order back, and
        // only the NEXT run order waits for them all.
        if positions
            .iter()
            .any(|index| record.stages[stage].actions[*index].due.is_none())
        {
            let anchor = record
                .stages
                .iter()
                .flat_map(|stage| &stage.actions)
                .filter(|action| action.is_terminal())
                .map(|action| action.last_status_change)
                .max()
                .unwrap_or(record.start_time);
            let due = anchor + ACTION_STEP;
            for index in &positions {
                let action = &mut record.stages[stage].actions[*index];
                if action.due.is_none() {
                    action.due = Some(due);
                }
            }
            record.last_update_time = now;
        }

        for index in positions {
            let action = &record.stages[stage].actions[index];
            if action.is_terminal() {
                continue;
            }
            if action.is_in_progress() {
                // An action WAITING for a run it started in another service is not stuck:
                // its next step is to READ that run, which falls due like every other
                // step of this engine rather than being waited for. A claim another call
                // is still inside is left alone; one that ran out is reclaimed HERE,
                // rather than falling through to a second start of the same work.
                if action.external_run.is_some() {
                    let claim_in_flight = action.claimed.is_some_and(|claimed| {
                        now.duration_since(claimed).unwrap_or_default() < CLAIM_TIMEOUT
                    });
                    if claim_in_flight || action.due.is_some_and(|due| due > now) {
                        continue;
                    }
                    return poll_action(record, stage, index, now);
                }
                // Either a human has not answered this action's approval yet, or another
                // call is performing its work (and may still be within its claim).
                // Either way it is THIS action that waits and not the rest of its run
                // order, so the walk moves on to its siblings; a claim nobody completed
                // is reclaimed below.
                match action.claimed {
                    Some(claimed)
                        if now.duration_since(claimed).unwrap_or_default() >= CLAIM_TIMEOUT => {}
                    _ => continue,
                }
            }
            // The whole run order shares one due instant, so an action whose instant has
            // not arrived means none of its siblings' has either.
            if action.due.is_some_and(|due| due > now) {
                continue;
            }
            return start_action(record, context, stage, index, now);
        }
        return None;
    }

    // Every run order of the stage is complete, so its SUCCESS conditions are
    // evaluated at that instant, and only a stage no condition was met for is
    // reported `Succeeded`.
    if settle_conditions(record, context, stage, CONDITION_TYPE_ON_SUCCESS, now)
        != ConditionGate::Proceed
    {
        return None;
    }
    record.stages[stage].status = Some(STAGE_SUCCEEDED.to_string());
    record.last_update_time = now;
    None
}

/// Claim the next step of an action that is WAITING for a run in another service: reading
/// that run, which is what settles the action once the run has finished.
///
/// The only work this engine starts in another service and comes back to is a BUILD (see
/// [`ActionRun::external_run`](super::execution::ActionRun::external_run)), so the step is
/// the read of that build. Nothing about the action's status changes here: it was and
/// stays `InProgress` until the build it names reaches a terminal status.
fn poll_action(
    record: &mut PipelineExecutionRecord,
    stage: usize,
    index: usize,
    now: SystemTime,
) -> Option<Job> {
    let execution_id = record.pipeline_execution_id.clone();
    let action = &mut record.stages[stage].actions[index];
    let build_id = action.external_run.clone()?;
    action.claimed = Some(now);
    Some(Job::CodeBuildPoll {
        execution_id,
        stage,
        action: index,
        build_id,
    })
}

/// Start one action: either it waits for a decision, or it claims the work it needs to
/// perform, or it fails because this emulator cannot perform that work at all.
fn start_action(
    record: &mut PipelineExecutionRecord,
    context: &mut StepContext<'_>,
    stage: usize,
    index: usize,
    now: SystemTime,
) -> Option<Job> {
    let declaration = context.declaration;
    let pipeline_arn = context.pipeline_arn.to_string();
    let variables = record.resolution_variables();
    let pipeline_name = record.pipeline_name.clone();
    let stage_name = record.stages[stage].name.clone();
    let artifact_bucket = record.artifact_bucket.clone();
    let artifact_encryption_key = record.artifact_encryption_key.clone();
    let execution_id = record.pipeline_execution_id.clone();
    // The artifact store this execution writes to, as the DECLARATION states it: the
    // bucket alone is on the record, and a job worker also needs the customer managed
    // key the store declares to read and write those objects.
    let artifact_encryption = declaration
        .artifact_store
        .iter()
        .chain(
            declaration
                .artifact_stores
                .iter()
                .flatten()
                .map(|(_, store)| store),
        )
        .find(|store| store.location == artifact_bucket)
        .and_then(|store| store.encryption_key.clone());
    // What a source action of THIS action's name is to read, when the start that created
    // the execution overrode it: each override's `revisionType` says what its value
    // pins, so only an S3 one names an object or a version (see
    // `execution::s3_source_pin`).
    let pin = super::execution::s3_source_pin(
        &record.source_overrides,
        &record.stages[stage].actions[index].name,
    );
    // The revision this execution was STARTED for, as its own record carries it: an
    // Amazon ECR source action sources that image rather than whatever its tag names by
    // the time the action runs.
    let pinned_revision = record
        .source_revisions
        .iter()
        .find(|revision| revision.action_name == record.stages[stage].actions[index].name)
        .and_then(|revision| revision.revision_id.clone());
    let produced = record.artifacts.clone();
    // What the action IS comes from the pinned declaration, not from a copy in the
    // execution record, and from the declared stage the PLAN names rather than from the
    // run's position in it (a rollback's plan holds one stage). A plan whose declaration
    // no longer carries that action cannot be run at all, which the action's own failure
    // reports.
    let declared = record
        .stages
        .get(stage)
        .and_then(|run| super::execution::declared_action(declaration, run, index))
        .cloned();
    let action = &mut record.stages[stage].actions[index];
    // The instant the execution REACHED this action, which is what the action
    // execution's `startTime` reports however long its work then takes: the instant the
    // clock made this step DUE, not the instant a call happened to materialise it. So
    // every action of one run order reports the same start — they run concurrently — and
    // a caller that polls late does not move the start it then reads. It is never before
    // the action's own last recorded change either, so a re-run action (which a retry
    // resets at the instant it asks for it) does not report a start from before the
    // request that re-ran it.
    let floor = action.last_status_change.min(now);
    let began = action.due.map_or(now, |due| due.clamp(floor, now));
    action.status = Some(ACTION_IN_PROGRESS.to_string());
    action.last_status_change = began;
    action.start_time = Some(began);
    let Some(declared) = declared else {
        fail_action(
            action,
            FAILURE_CONFIGURATION_ERROR,
            "the pipeline version this execution runs no longer declares this action",
            now,
        );
        return None;
    };
    let category = declared.action_type_id.category.clone();
    let provider = declared.action_type_id.provider.clone();
    let namespace = declared.namespace.clone();
    let input_artifacts: Vec<String> = declared
        .input_artifacts
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|artifact| artifact.name)
        .collect();
    let output_artifacts: Vec<String> = declared
        .output_artifacts
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|artifact| artifact.name)
        .collect();
    // One helper resolves a configuration for the engine and for the job worker alike,
    // so the two cannot read one action's configuration differently.
    let config: BTreeMap<String, String> =
        super::execution::resolved_configuration(&declared, &variables);
    let _ = namespace;

    if category == CATEGORY_APPROVAL {
        // An approval action does no work: it holds the stage until
        // `PutApprovalResult` answers the token it publishes here.
        action.token = Some(random_uuid_v4());
        action.summary = config.get("CustomData").cloned();
        return None;
    }

    if declared.action_type_id.owner == super::ACTION_OWNER_CUSTOM {
        // CodePipeline does not run a CUSTOM action: the account's own job worker does
        // ("PollForJobs is valid only for action types with Custom in the owner
        // field"). So reaching the action does not perform work and does not fail it
        // either — it QUEUES a job for the worker to poll for, and the action stays
        // InProgress until the worker reports a result. The job is queued exactly once
        // per action, because an action is started once: every later step sees it
        // InProgress and waits.
        //
        // The artifacts the worker reads are the ones the earlier actions produced, at
        // the locations they produced them at; the ones it is to write are the
        // locations this execution assigns them in the artifact store.
        let job_inputs: Vec<crate::ops::types::Artifact> = input_artifacts
            .iter()
            .filter_map(|name| {
                produced
                    .get(name)
                    .map(|location| super::job_artifact(name, location))
            })
            .collect();
        let job_outputs: Vec<crate::ops::types::Artifact> = output_artifacts
            .iter()
            .map(|name| {
                super::job_artifact(
                    name,
                    &ArtifactRef {
                        bucket: artifact_bucket.clone(),
                        key: artifact_key(&pipeline_name, name),
                        revision: None,
                        encryption: artifact_encryption_key.clone(),
                    },
                )
            })
            .collect();
        let job = super::JobRecord {
            pipeline_name,
            pipeline_arn,
            pipeline_execution_id: execution_id,
            stage_name,
            action_name: action.name.clone(),
            action_execution_id: action.action_execution_id.clone(),
            stage,
            action: index,
            input_artifacts: job_inputs,
            output_artifacts: job_outputs,
            configuration: config,
            encryption_key: artifact_encryption,
            ..super::JobRecord::new(random_uuid_v4(), declared.action_type_id.clone(), now)
        };
        super::enqueue_job_into(context.jobs, &declared.action_type_id, job);
        return None;
    }

    if category == CATEGORY_SOURCE && provider == PROVIDER_CODE_COMMIT {
        // A CodeCommit source action's revision is a commit of the repository its
        // configuration names, on the branch it names: the work is reading that
        // branch's tip and producing the files at it as the output artifact. Both
        // keys are what CodePipeline's CodeCommit source action declares, so a
        // declaration missing either cannot name a revision at all.
        let (Some(repository), Some(branch)) = (
            config.get(CONFIG_REPOSITORY_NAME),
            config.get(CONFIG_BRANCH_NAME),
        ) else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                &format!(
                    "a CodeCommit source action must declare the configuration keys \
                     {CONFIG_REPOSITORY_NAME} and {CONFIG_BRANCH_NAME}"
                ),
                now,
            );
            return None;
        };
        if output_artifacts.is_empty() {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                "a CodeCommit source action must declare an output artifact to produce",
                now,
            );
            return None;
        }
        let outputs = output_artifacts
            .iter()
            .map(|name| ArtifactRef {
                bucket: artifact_bucket.clone(),
                key: artifact_key(&pipeline_name, name),
                revision: None,
                encryption: artifact_encryption_key.clone(),
            })
            .collect();
        action.claimed = Some(now);
        return Some(Job::CodeCommitSource {
            execution_id,
            stage,
            action: index,
            repository: repository.clone(),
            branch: branch.clone(),
            outputs,
        });
    }

    if provider == PROVIDER_LAMBDA {
        // A Lambda action's work IS the invocation of the function its configuration
        // names ("the function to invoke" is the action's `FunctionName`), with
        // `UserParameters` passed through to it, so the action is performed by calling
        // that function and its outcome is what the invocation answered.
        let Some(function_name) = config.get(CONFIG_FUNCTION_NAME) else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                &format!(
                    "a {provider} {category} action must declare the configuration key \
                     {CONFIG_FUNCTION_NAME}"
                ),
                now,
            );
            return None;
        };
        // The artifacts the function is handed are the ones the earlier actions
        // produced, at the locations they produced them at; a declared input nothing
        // produced means the action cannot run as declared.
        let mut inputs = Vec::with_capacity(input_artifacts.len());
        for name in &input_artifacts {
            let Some(location) = produced.get(name).cloned() else {
                fail_action(
                    action,
                    FAILURE_CONFIGURATION_ERROR,
                    &format!("the input artifact {name} was not produced by an earlier action"),
                    now,
                );
                return None;
            };
            inputs.push((name.clone(), location));
        }
        let outputs = output_artifacts
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    ArtifactRef {
                        bucket: artifact_bucket.clone(),
                        key: artifact_key(&pipeline_name, name),
                        revision: None,
                        encryption: artifact_encryption_key.clone(),
                    },
                )
            })
            .collect();
        action.claimed = Some(now);
        return Some(Job::LambdaInvoke {
            execution_id,
            stage,
            action: index,
            function_name: function_name.clone(),
            user_parameters: config.get(CONFIG_USER_PARAMETERS).cloned(),
            // The id the event publishes for this run. CodePipeline does not queue a
            // pollable job for an action it owns ("PollForJobs is valid only for action
            // types with Custom in the owner field"), so the handle the event carries is
            // the action execution's own id, which is how every reporting operation of
            // this service addresses this run.
            job_id: action.action_execution_id.clone(),
            inputs,
            outputs,
        });
    }

    if category == CATEGORY_SOURCE && provider == PROVIDER_S3 {
        let (Some(bucket), Some(declared_key)) = (config.get("S3Bucket"), config.get("S3ObjectKey"))
        else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                "an S3 source action must declare the configuration keys S3Bucket and \
                 S3ObjectKey",
                now,
            );
            return None;
        };
        if output_artifacts.is_empty() {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                "an S3 source action must declare an output artifact to produce",
                now,
            );
            return None;
        }
        let outputs = output_artifacts
            .iter()
            .map(|name| ArtifactRef {
                bucket: artifact_bucket.clone(),
                key: artifact_key(&pipeline_name, name),
                revision: None,
                encryption: artifact_encryption_key.clone(),
            })
            .collect();
        // The object this run reads: the key the declaration states, under the key an
        // `S3_OBJECT_KEY` override names instead, at the version an
        // `S3_OBJECT_VERSION_ID` override pins — and otherwise at the object's CURRENT
        // version, because nothing pinned one.
        action.claimed = Some(now);
        return Some(Job::S3Source {
            execution_id,
            stage,
            action: index,
            bucket: bucket.clone(),
            key: pin.key.unwrap_or_else(|| declared_key.clone()),
            version_id: pin.version_id,
            outputs,
        });
    }

    if category == CATEGORY_DEPLOY && provider == PROVIDER_S3 {
        // "Extract file before deploy": the configuration value is the string "true" or
        // "false" on the wire, and it decides which of the provider's other keys the
        // declaration has to carry.
        let extract = config
            .get("Extract")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        let Some(bucket) = config.get("BucketName") else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                "an S3 deploy action must declare the configuration key BucketName",
                now,
            );
            return None;
        };
        // An extracting deploy writes one object per archive entry, so there is no
        // single object key for it: `ObjectKey` is then OPTIONAL and, when declared, is
        // the prefix the entries land beneath. Without extraction the artifact is
        // delivered as one object, which has to have a key.
        let key = config.get("ObjectKey").cloned();
        if !extract && key.is_none() {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                "an S3 deploy action that does not extract the artifact must declare the \
                 configuration key ObjectKey",
                now,
            );
            return None;
        }
        let Some(name) = input_artifacts.first().cloned() else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                "an S3 deploy action must declare the input artifact it deploys",
                now,
            );
            return None;
        };
        let Some(input) = produced.get(&name).cloned() else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                &format!("the input artifact {name} was not produced by an earlier action"),
                now,
            );
            return None;
        };
        action.claimed = Some(now);
        return Some(Job::S3Deploy {
            execution_id,
            stage,
            action: index,
            input,
            bucket: bucket.clone(),
            key,
            extract,
            // "KMSEncryptionKeyARN" is the S3 deploy action's own configuration key for
            // the customer managed key the DELIVERED object is encrypted under; an
            // action that names none delivers under the key the pipeline's artifact
            // store declares, which is the key every other object of this execution
            // carries.
            encryption_key: config
                .get(CONFIG_KMS_ENCRYPTION_KEY_ARN)
                .filter(|key| !key.is_empty())
                .cloned()
                .or_else(|| artifact_encryption_key.clone()),
        });
    }

    if category == CATEGORY_SOURCE && provider == PROVIDER_ECR {
        // An Amazon ECR source action's revision is an IMAGE of the repository its
        // configuration names, at the tag it names: the work is resolving that tag to
        // the image the registry holds and producing the image's detail as the output
        // artifact, which is what a deploy action downstream reads the image URI from.
        let Some(repository) = config.get(CONFIG_REPOSITORY_NAME) else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                &format!(
                    "an {provider} {category} action must declare the configuration key \
                     {CONFIG_REPOSITORY_NAME}"
                ),
                now,
            );
            return None;
        };
        if output_artifacts.is_empty() {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                &format!(
                    "an {provider} {category} action must declare an output artifact to \
                     produce"
                ),
                now,
            );
            return None;
        }
        let outputs = output_artifacts
            .iter()
            .map(|name| ArtifactRef {
                bucket: artifact_bucket.clone(),
                key: artifact_key(&pipeline_name, name),
                revision: None,
                encryption: artifact_encryption_key.clone(),
            })
            .collect();
        action.claimed = Some(now);
        return Some(Job::EcrSource {
            execution_id,
            stage,
            action: index,
            repository: repository.clone(),
            // The declaration's tag, or the one the action documents as its default.
            image_tag: config
                .get(CONFIG_IMAGE_TAG)
                .filter(|tag| !tag.is_empty())
                .cloned()
                .unwrap_or_else(|| DEFAULT_IMAGE_TAG.to_string()),
            image_digest: pinned_revision,
            outputs,
        });
    }

    if category == CATEGORY_BUILD && provider == PROVIDER_CODE_BUILD {
        // A CodeBuild action's work IS a build of the project its configuration names:
        // the action is performed by starting that build and is settled from the build's
        // own terminal status, which the next step of the action reads. So nothing here
        // reports an outcome — the build has not run yet.
        let Some(project_name) = config.get(CONFIG_PROJECT_NAME) else {
            fail_action(
                action,
                FAILURE_CONFIGURATION_ERROR,
                &format!(
                    "a {provider} {category} action must declare the configuration key \
                     {CONFIG_PROJECT_NAME}"
                ),
                now,
            );
            return None;
        };
        let environment_overrides = match config
            .get(CONFIG_ENVIRONMENT_VARIABLES)
            .map(|declared| declared_environment_overrides(declared))
        {
            None => Vec::new(),
            Some(Ok(overrides)) => overrides,
            Some(Err(detail)) => {
                fail_action(
                    action,
                    FAILURE_CONFIGURATION_ERROR,
                    &format!(
                        "the configuration key {CONFIG_ENVIRONMENT_VARIABLES} of this \
                         action {detail}"
                    ),
                    now,
                );
                return None;
            }
        };
        action.claimed = Some(now);
        return Some(Job::CodeBuildStart {
            execution_id,
            stage,
            action: index,
            project_name: project_name.clone(),
            environment_overrides,
        });
    }

    // Work this emulator cannot perform. Nothing is reported as having run: the action
    // fails with the reason, and the execution reaches the modelled `Failed` status.
    let detail = format!(
        "this emulator does not run {category} actions provided by {provider}, so the \
         action cannot be performed"
    );
    fail_action(action, FAILURE_CONFIGURATION_ERROR, &detail, now);
    None
}

/// The environment variables a CodeBuild action's `EnvironmentVariables` configuration
/// overrides the project's with, as `(name, value, type)`.
///
/// The configuration value is a JSON ARRAY of objects, each with a `name` and a `value`
/// and optionally a `type` — the action's own documented form — so a value that is not
/// that is a declaration this action cannot be run as, which the caller reports as the
/// `ConfigurationError` it is. `Err` carries the clause that says what is wrong with it.
///
/// Nothing is judged about the variables themselves here: the entry count and the names
/// are CodeBuild's to accept or refuse, and that service's refusal is what the action then
/// reports.
fn declared_environment_overrides(declared: &str) -> Result<EnvironmentOverrides, &'static str> {
    let document: ::serde_json::Value =
        ::serde_json::from_str(declared).map_err(|_| "is not a JSON document")?;
    let entries = document
        .as_array()
        .ok_or("is not a JSON array of environment variables")?;
    let mut overrides = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(::serde_json::Value::as_str)
            .ok_or("carries an entry that states no name")?;
        let value = entry
            .get("value")
            .and_then(::serde_json::Value::as_str)
            .ok_or("carries an entry that states no value")?;
        let kind = entry
            .get("type")
            .and_then(::serde_json::Value::as_str)
            .map(str::to_string);
        overrides.push((name.to_string(), value.to_string(), kind));
    }
    Ok(overrides)
}

/// Record a failure against one action.
fn fail_action(
    action: &mut super::execution::ActionRun,
    code: &str,
    message: &str,
    now: SystemTime,
) {
    action.status = Some(ACTION_FAILED.to_string());
    action.claimed = None;
    // The action is settled, so it is no longer waiting for anything.
    action.external_run = None;
    // Each of the two members carrying this failure is held to the length it is
    // modelled with, so a relayed answer cannot take either past it.
    action.summary = Some(within(message.to_string(), MAX_EXECUTION_SUMMARY));
    action.error = Some((
        code.to_string(),
        within(message.to_string(), MAX_ERROR_MESSAGE),
    ));
    action.last_status_change = now;
}

/// The artifact-store key one produced artifact's bytes are written to.
///
/// CodePipeline's own layout is `<pipeline name>/<artifact name>/<unique id>`, so an
/// artifact of one execution never overwrites another's.
fn artifact_key(pipeline_name: &str, artifact_name: &str) -> String {
    let unique: String = random_uuid_v4().chars().filter(|c| *c != '-').take(11).collect();
    format!("{pipeline_name}/{artifact_name}/{unique}")
}

/// Settle a graceful stop: nothing is left running, so the execution is `Stopped` and
/// every action that was still waiting is `Abandoned`.
pub fn settle_stopped(record: &mut PipelineExecutionRecord, now: SystemTime) {
    for stage in &mut record.stages {
        for action in &mut stage.actions {
            if action.is_in_progress() {
                action.status = Some("Abandoned".to_string());
                action.token = None;
                action.last_status_change = now;
            }
        }
        if stage.status.as_deref() == Some(STAGE_IN_PROGRESS) {
            stage.status = Some("Stopped".to_string());
        }
    }
    record.status = STATUS_STOPPED.to_string();
    record.last_update_time = now;
}

/// Record what performing a claimed job produced.
fn record_outcome(
    store: &mut ComAmazonawsCodepipelineCodepipeline20150709State,
    job: &Job,
    outcome: JobOutcome,
    now: SystemTime,
) {
    let (execution_id, stage, index) = job.target();
    // Disjoint field borrows again: the declaration is read while the record is written
    // (see `plan`), so what the action DECLARED — its namespace, its output artifact
    // names — is taken from the pinned declaration rather than from a copy.
    let ComAmazonawsCodepipelineCodepipeline20150709State {
        pipelines,
        executions,
        ..
    } = store;
    let Some(record) = executions.get_mut(execution_id) else {
        return;
    };
    let declared = pipelines
        .get(&record.pipeline_name)
        .and_then(|pipeline| pipeline.declaration_at(Some(record.pipeline_version)))
        .zip(record.stages.get(stage))
        .and_then(|(declaration, run)| {
            super::execution::declared_action(declaration, run, index)
        });
    let namespace = declared.and_then(|action| action.namespace.clone());
    let output_names: Vec<String> = declared
        .and_then(|action| action.output_artifacts.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|artifact| artifact.name)
        .collect();
    let Some(action) = record
        .stages
        .get_mut(stage)
        .and_then(|stage| stage.actions.get_mut(index))
    else {
        return;
    };
    let action_name = action.name.clone();
    match outcome {
        JobOutcome::Done {
            summary,
            revision,
            variables,
            external_id,
        } => {
            action.status = Some(ACTION_SUCCEEDED.to_string());
            action.claimed = None;
            action.external_run = None;
            action.summary = Some(within(summary, MAX_EXECUTION_SUMMARY));
            action.last_status_change = now;
            action.external_execution_id = external_id;
            let produced = match job {
                Job::S3Source { outputs, .. }
                | Job::CodeCommitSource { outputs, .. }
                | Job::EcrSource { outputs, .. } => outputs.as_slice(),
                _ => &[],
            };
            for (name, location) in output_names.iter().zip(produced.iter()) {
                let mut location = location.clone();
                location.revision = revision.clone();
                record.artifacts.insert(name.clone(), location);
            }
            if let Some(namespace) = namespace {
                for (key, value) in &variables {
                    record
                        .variables_map
                        .insert(format!("{namespace}.{key}"), value.clone());
                }
            }
            // `SourceRevision` is what a SOURCE action resolved — `sourceRevisions` is
            // "the source revisions for a pipeline execution" — so only a source
            // action's revision is recorded there; an action that merely consumed an
            // artifact produced from a revision did not resolve one.
            let sourced = matches!(
                job,
                Job::S3Source { .. } | Job::CodeCommitSource { .. } | Job::EcrSource { .. }
            );
            if let (true, Some(revision)) = (sourced, revision) {
                let existing = record
                    .source_revisions
                    .iter_mut()
                    .find(|recorded| recorded.action_name == action_name);
                match existing {
                    Some(recorded) => recorded.revision_id = Some(revision),
                    None => record.source_revisions.push(SourceRevision {
                        action_name,
                        revision_id: Some(revision),
                        revision_summary: None,
                        revision_url: None,
                    }),
                }
            }
        }
        JobOutcome::Failed {
            code,
            message,
            external_id,
        } => {
            fail_action(action, &code, &message, now);
            action.external_execution_id = external_id;
        }
        // The work is RUNNING in another service: the action keeps the `InProgress`
        // status it already reports, the run it is waiting for is recorded on it, and the
        // step that reads that run again becomes due a span from now — the same span every
        // other step of this engine waits, stored rather than slept on. The claim is
        // released, so whichever call or clock tick comes next performs that read.
        JobOutcome::Running {
            external_run,
            summary,
        } => {
            action.claimed = None;
            action.summary = Some(within(summary, MAX_EXECUTION_SUMMARY));
            action.due = Some(now + ACTION_STEP);
            // What a client sees as the run performing this action, which is the handle
            // the next step reads it by.
            action.external_execution_id = Some(external_run.clone());
            action.external_run = Some(external_run);
        }
    }
    record.last_update_time = now;
}

/// An S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and this
/// service names no other service's crate. Path-style addressing is what the emulated
/// S3 service routes on (its operations are bound under a `{Bucket}` path label).
fn s3_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        // The endpoint is a FIXED literal (see `state::in_process_endpoint`): no
        // request-derived text reaches the host, so nothing a caller sends can
        // re-point this call, which carries the emulator's own credentials.
        .endpoint_url(super::in_process_endpoint("s3"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// A CodeCommit client over the in-process callback into this same emulator.
///
/// Built exactly like [`s3_client`], for the same reasons: a fixed endpoint literal, the
/// emulator's own credentials, and no retry, timeout or stalled-stream machinery, because
/// the transport is in process.
fn code_commit_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_codecommit::Client {
    let config = ::aws_sdk_codecommit::Config::builder()
        .behavior_version(::aws_sdk_codecommit::config::BehaviorVersion::latest())
        .region(::aws_sdk_codecommit::config::Region::new(region.to_string()))
        .endpoint_url(super::in_process_endpoint("codecommit"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_codecommit::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_codecommit::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_codecommit::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_codecommit::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_codecommit::Client::from_conf(config)
}

/// A Lambda client over the in-process callback into this same emulator.
///
/// Built exactly like [`s3_client`], for the same reasons.
fn lambda_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(super::in_process_endpoint("lambda"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

/// A KMS client over the in-process callback into this same emulator.
///
/// Built exactly like [`s3_client`], for the same reasons.
fn kms_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(super::in_process_endpoint("kms"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}

/// Take the data key one object's server-side encryption under `key_id` needs, and
/// answer why that key cannot be used when KMS refuses it.
///
/// `ArtifactStore$encryptionKey` is "the encryption key used to encrypt the data in the
/// artifact store... for example, an AWS Key Management Service key", and encrypting an
/// object under a customer managed key is a use of that key: KMS issues a data key for
/// it per object. So the key is really used here — through the same `GenerateDataKey`
/// S3 performs for SSE-KMS — and whatever KMS says about it decides the action: a key
/// that is disabled, pending deletion or not there at all refuses the call, and an
/// artifact that cannot be encrypted as the store declares is an artifact that cannot
/// be produced.
async fn artifact_data_key(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    key_id: &str,
) -> Result<(), String> {
    kms_client(callback, region)
        .generate_data_key()
        .key_id(key_id)
        // The data key an S3 object's SSE-KMS encryption takes.
        .key_spec(::aws_sdk_kms::types::DataKeySpec::Aes256)
        .send()
        .await
        .map(|_| ())
        .map_err(|error| service_error_detail(&error))
}

/// One artifact write, carrying the server-side encryption the artifact store's
/// customer managed key asks for.
///
/// The members are S3's own (`PutObjectRequest$ServerSideEncryption` and
/// `$SSEKMSKeyId`), so the object a later `HeadObject`/`GetObject` reports is reported
/// as encrypted under exactly the key the store declares. A write with no key declared
/// names neither member and inherits whatever the bucket's own default configuration is.
fn encrypted_put(
    request: ::aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder,
    encryption: Option<&str>,
) -> ::aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder {
    match encryption {
        Some(key_id) => request
            .server_side_encryption(::aws_sdk_s3::types::ServerSideEncryption::AwsKms)
            .ssekms_key_id(key_id),
        None => request,
    }
}

/// CodePipeline's own detail for a failure it does not surface the cause of: the fixed
/// text, and the reference code that names THIS occurrence of it.
///
/// A failure the pipeline's own work ran into inside a service CodePipeline called on
/// the account's behalf is not a fact about the caller's declaration, and CodePipeline
/// does not relay it: the action's `errorDetails` carry this shape, with a reference
/// code an operator correlates against the service's own records. The cause is written
/// to this emulator's log under the same code, which is where that correlation lands
/// here.
fn internal_error_detail(reference: &str) -> String {
    format!("InternalError. Error reference code: {reference}.")
}

/// What an action reports about an error the service it called answered with: that
/// error's own CODE and MESSAGE, the two facts the answering service published about it.
///
/// Those two are what an operator reading the failure needs and all the answering
/// service stated. The client object this emulator holds the answer in is NOT part of
/// the failure: formatting it renders Rust type names, the raw response document and
/// the request and host ids of one call, none of which is a fact about the pipeline,
/// and all of which would spend the characters the members carrying the failure are
/// modelled for ([`MAX_ERROR_MESSAGE`], [`MAX_EXECUTION_SUMMARY`]).
fn service_error_detail<E, R>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>,
) -> String
where
    ::aws_smithy_runtime_api::client::result::SdkError<E, R>:
        ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(error);
    let message = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::message(error);
    match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        // An answer that published neither — a call that never reached the service, or
        // whose answer could not be read — states WHICH of those it was, from the
        // variant the client holds it as. That is one bounded sentence per kind, and
        // nothing of the object itself.
        (None, None) => unanswered_call_detail(error).to_string(),
    }
}

/// Why a call the answering service published nothing about did not produce an answer.
///
/// The variants are the client's own classes of unanswered call, and each is a
/// different thing for an operator to do about it, so they are told apart rather than
/// collapsed into one sentence. The enum is `#[non_exhaustive]`, so a class added later
/// falls back to the fact they all share.
fn unanswered_call_detail<E, R>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>,
) -> &'static str {
    use ::aws_smithy_runtime_api::client::result::SdkError;
    match error {
        SdkError::ConstructionFailure(_) => "the call could not be constructed",
        SdkError::TimeoutError(_) => "the call timed out",
        SdkError::DispatchFailure(_) => "the call could not be dispatched",
        SdkError::ResponseError(_) => "the answer to the call could not be read",
        _ => "the call did not complete",
    }
}

/// The most characters an action's summary carries: `ActionExecution$summary` and
/// `ActionExecutionResult$externalExecutionSummary` target `ExecutionSummary`,
/// `@length(min: 1, max: 2048)`
/// (`smithy_query query="shape" shape="ExecutionSummary"`).
const MAX_EXECUTION_SUMMARY: usize = 2_048;

/// The most characters a failure's message carries: `ErrorDetails$message` targets
/// `Message`, `@length(min: 1, max: 5000)`
/// (`smithy_query query="shape" shape="ErrorDetails"`).
const MAX_ERROR_MESSAGE: usize = 5_000;

/// `text` held to the `limit` characters the member carrying it is modelled for, cut on
/// a character boundary so what is recorded is still text.
fn within(text: String, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        None => text,
        Some((cut, _)) => text[..cut].to_string(),
    }
}

/// Perform one claimed job's real work.
async fn run_job(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    job: &Job,
) -> JobOutcome {
    // An artifact the pipeline's artifact store encrypts is written under a data key KMS
    // issues for that store's key, so the key is taken BEFORE any of the job's work is
    // performed: a key KMS will not let this account use is an artifact that cannot be
    // produced, whichever action was about to produce it.
    if let Some(key_id) = job.artifact_encryption() {
        if let Err(detail) = artifact_data_key(callback, region, key_id).await {
            // The work the action was running — producing or delivering its artifact —
            // is what failed, so the failure is the RUN's own ([`FAILURE_JOB_FAILED`])
            // and not a declaration that could never run: a store whose key cannot be
            // used right now is one whose key can be used again later, as re-enabling it
            // shows. What KMS said is not relayed to the caller (see
            // [`internal_error_detail`]) and is logged under the reference code instead.
            let reference = random_uuid_v4();
            ::tracing::warn!(
                reference = %reference,
                key_id = %key_id,
                detail = %detail,
                "an artifact object could not be encrypted under the artifact store's \
                 customer managed key"
            );
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: internal_error_detail(&reference),
                external_id: None,
            };
        }
    }
    match job {
        Job::S3Source {
            bucket,
            key,
            version_id,
            outputs,
            ..
        } => {
            run_source(
                &s3_client(callback, region),
                bucket,
                key,
                version_id.as_deref(),
                outputs,
            )
            .await
        }
        Job::CodeCommitSource {
            repository,
            branch,
            outputs,
            ..
        } => {
            run_code_commit_source(
                &code_commit_client(callback, region),
                &s3_client(callback, region),
                repository,
                branch,
                outputs,
            )
            .await
        }
        Job::LambdaInvoke {
            function_name,
            user_parameters,
            job_id,
            inputs,
            outputs,
            ..
        } => {
            run_lambda_invoke(
                &lambda_client(callback, region),
                function_name,
                user_parameters.as_deref(),
                job_id,
                inputs,
                outputs,
            )
            .await
        }
        Job::S3Deploy {
            input,
            bucket,
            key,
            extract,
            encryption_key,
            ..
        } => {
            run_deploy(
                &s3_client(callback, region),
                input,
                bucket,
                key.as_deref(),
                *extract,
                encryption_key.as_deref(),
            )
            .await
        }
        Job::EcrSource {
            repository,
            image_tag,
            image_digest,
            outputs,
            ..
        } => {
            image_source::run_ecr_source(
                callback,
                &s3_client(callback, region),
                region,
                repository,
                image_tag,
                image_digest.as_deref(),
                outputs,
            )
            .await
        }
        Job::CodeBuildStart {
            project_name,
            environment_overrides,
            ..
        } => {
            build_action::run_codebuild_start(
                callback,
                region,
                project_name,
                environment_overrides,
            )
            .await
        }
        Job::CodeBuildPoll { build_id, .. } => {
            build_action::run_codebuild_poll(callback, region, build_id).await
        }
    }
}

/// Resolve the branch's tip commit and write the repository's files at it into the
/// artifact store.
///
/// The revision a CodeCommit source action ran is that commit: it is the artifact's
/// revision, the output variable the later actions resolve, and the external execution
/// the action reports. A repository or a branch the declaration names and the service
/// does not hold is a declaration this pipeline cannot source from at all, which is what
/// the `ConfigurationError` failure type reports; anything that goes wrong while the
/// bytes are being moved is the run's own failure (`JobFailed`).
async fn run_code_commit_source(
    code_commit: &::aws_sdk_codecommit::Client,
    s3: &::aws_sdk_s3::Client,
    repository: &str,
    branch: &str,
    outputs: &[ArtifactRef],
) -> JobOutcome {
    let tip = code_commit
        .get_branch()
        .repository_name(repository)
        .branch_name(branch)
        .send()
        .await;
    let commit_id = match tip {
        Ok(answer) => answer
            .branch
            .and_then(|info| info.commit_id)
            .unwrap_or_default(),
        Err(err) => {
            let service = err.as_service_error();
            let unreachable = service.is_some_and(|err| {
                err.is_repository_does_not_exist_exception()
                    || err.is_branch_does_not_exist_exception()
            });
            // A repository or a branch the declaration names and the service does not
            // hold is a declaration this pipeline cannot source from at all, not a
            // revision that is missing from a source it can.
            let code = if unreachable {
                FAILURE_CONFIGURATION_ERROR
            } else {
                FAILURE_JOB_FAILED
            };
            return JobOutcome::Failed {
                code: code.to_string(),
                message: format!(
                    "the branch {branch} of the repository {repository} could not be \
                     resolved: {}",
                    service_error_detail(&err)
                ),
                external_id: None,
            };
        }
    };
    if commit_id.is_empty() {
        return JobOutcome::Failed {
            code: FAILURE_REVISION_UNAVAILABLE.to_string(),
            message: format!(
                "the branch {branch} of the repository {repository} reports no commit, so \
                 there is no revision to source"
            ),
            external_id: None,
        };
    }

    // Every file the commit holds, as the comparison against the empty tree reports it:
    // with no `beforeCommitSpecifier` "all changes before the afterCommitSpecifier value
    // are shown", so each of the commit's files is an addition. The comparison is PAGED,
    // so the pages are followed to the end — a revision packed from the first page alone
    // would be a short artifact reported as the whole commit — and the file count is
    // checked as the pages accumulate, so the walk is bounded by [`MAX_SOURCE_FILES`]
    // however many pages the answer has.
    let mut files: Vec<(String, String)> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let compared = code_commit
            .get_differences()
            .repository_name(repository)
            .after_commit_specifier(&commit_id)
            .set_next_token(page_token.clone())
            .send()
            .await;
        let answer = match compared {
            Ok(answer) => answer,
            Err(err) => {
                return JobOutcome::Failed {
                    code: FAILURE_JOB_FAILED.to_string(),
                    message: format!(
                        "the files of the commit {commit_id} could not be read: {}",
                        service_error_detail(&err)
                    ),
                    external_id: Some(commit_id),
                }
            }
        };
        files.extend(
            answer
                .differences
                .unwrap_or_default()
                .into_iter()
                .filter_map(|difference| {
                    let blob = difference.after_blob?;
                    Some((blob.path?, blob.blob_id?))
                }),
        );
        if files.len() > MAX_SOURCE_FILES {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the commit {commit_id} carries more than the {MAX_SOURCE_FILES} files \
                     this emulator packs into a source artifact"
                ),
                external_id: Some(commit_id),
            };
        }
        page_token = answer.next_token;
        if page_token.is_none() {
            break;
        }
    }

    // The bytes of each file, read one blob at a time and accounted against the budget
    // as they arrive, so no commit can drive an allocation past the stated figure.
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    let mut packed = 0usize;
    for (path, blob_id) in files {
        let blob = code_commit
            .get_blob()
            .repository_name(repository)
            .blob_id(&blob_id)
            .send()
            .await;
        let content = match blob {
            Ok(answer) => answer.content.into_inner(),
            Err(err) => {
                return JobOutcome::Failed {
                    code: FAILURE_JOB_FAILED.to_string(),
                    message: format!(
                        "the file {path} of the commit {commit_id} could not be read: {}",
                        service_error_detail(&err)
                    ),
                    external_id: Some(commit_id),
                }
            }
        };
        packed = packed.saturating_add(content.len());
        if packed > MAX_SOURCE_BYTES {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the commit {commit_id} carries more than the {MAX_SOURCE_BYTES} bytes \
                     this emulator packs into a source artifact"
                ),
                external_id: Some(commit_id),
            };
        }
        entries.push((path, content));
    }

    // The artifact a source action produces is the archive of the revision's files,
    // which is what a later deploy action extracts. Building it is synchronous CPU work,
    // so it runs on the blocking pool rather than on the worker serving this request.
    let count = entries.len();
    let archive = match ::tokio::task::spawn_blocking(move || zip_of(&entries)).await {
        Ok(Ok(archive)) => archive,
        Ok(Err(detail)) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!("the source artifact could not be packed: {detail}"),
                external_id: Some(commit_id),
            }
        }
        Err(err) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!("the source artifact was not packed: {err}"),
                external_id: Some(commit_id),
            }
        }
    };
    for output in outputs {
        if let Err(err) = encrypted_put(
            s3.put_object()
                .bucket(&output.bucket)
                .key(&output.key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(archive.clone())),
            output.encryption.as_deref(),
        )
        .send()
        .await
        {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the artifact could not be written to s3://{}/{}: {}",
                    output.bucket,
                    output.key,
                    service_error_detail(&err)
                ),
                external_id: Some(commit_id),
            };
        }
    }

    // The output variables a CodeCommit source action publishes for the later actions of
    // its execution, under the namespace the action declares.
    let mut variables = BTreeMap::new();
    variables.insert("CommitId".to_string(), commit_id.clone());
    variables.insert("RepositoryName".to_string(), repository.to_string());
    variables.insert("BranchName".to_string(), branch.to_string());
    JobOutcome::Done {
        summary: format!(
            "Packed {count} file(s) of the commit {commit_id} on {branch} of {repository} as \
             the output artifact"
        ),
        revision: Some(commit_id.clone()),
        variables,
        external_id: Some(commit_id),
    }
}

/// Invoke the function a Lambda action names, and report what the invocation answered.
///
/// The event is the one CodePipeline publishes for such an action: the job's id, the
/// action's `UserParameters` and the artifacts the function is handed and is to write.
/// A function the service does not hold cannot be invoked as the declaration states it
/// (`ConfigurationError`); a function it holds but whose invocation did not run is the
/// run's own failure (`JobFailed`), and NOTHING is reported as having run in that case.
async fn run_lambda_invoke(
    lambda: &::aws_sdk_lambda::Client,
    function_name: &str,
    user_parameters: Option<&str>,
    job_id: &str,
    inputs: &[(String, ArtifactRef)],
    outputs: &[(String, ArtifactRef)],
) -> JobOutcome {
    let artifact_list = |artifacts: &[(String, ArtifactRef)]| {
        artifacts
            .iter()
            .map(|(name, location)| {
                ::serde_json::json!({
                    "name": name,
                    "location": {
                        "type": "S3",
                        "s3Location": {
                            "bucketName": location.bucket,
                            "objectKey": location.key,
                        },
                    },
                })
            })
            .collect::<Vec<::serde_json::Value>>()
    };
    let mut action_configuration = ::serde_json::Map::new();
    action_configuration.insert(
        CONFIG_FUNCTION_NAME.to_string(),
        ::serde_json::Value::String(function_name.to_string()),
    );
    if let Some(parameters) = user_parameters {
        action_configuration.insert(
            CONFIG_USER_PARAMETERS.to_string(),
            ::serde_json::Value::String(parameters.to_string()),
        );
    }
    let event = ::serde_json::json!({
        "CodePipeline.job": {
            "id": job_id,
            "accountId": ::engine::emulator_account_id(),
            "data": {
                "actionConfiguration": {"configuration": action_configuration},
                "inputArtifacts": artifact_list(inputs),
                "outputArtifacts": artifact_list(outputs),
            },
        }
    });
    let invoked = lambda
        .invoke()
        .function_name(function_name)
        .payload(::aws_smithy_types::Blob::new(event.to_string().into_bytes()))
        .send()
        .await;
    match invoked {
        Ok(answer) => {
            // A function that ran and reported a handler error did not do the action's
            // work, so the action failed with what the function reported.
            if let Some(error) = answer.function_error {
                return JobOutcome::Failed {
                    code: FAILURE_JOB_FAILED.to_string(),
                    message: format!(
                        "the function {function_name} reported {error} for the action's \
                         invocation"
                    ),
                    external_id: Some(function_name.to_string()),
                };
            }
            JobOutcome::Done {
                summary: format!("Invoked the function {function_name}"),
                // The invocation is not a revision of anything: a Lambda action
                // produces no source revision.
                revision: None,
                variables: BTreeMap::new(),
                external_id: Some(function_name.to_string()),
            }
        }
        Err(err) => {
            let absent = err
                .as_service_error()
                .is_some_and(::aws_sdk_lambda::operation::invoke::InvokeError::is_resource_not_found_exception);
            let code = if absent {
                FAILURE_CONFIGURATION_ERROR
            } else {
                FAILURE_JOB_FAILED
            };
            JobOutcome::Failed {
                code: code.to_string(),
                message: format!(
                    "the function {function_name} could not be invoked for this action: {}",
                    service_error_detail(&err)
                ),
                external_id: Some(function_name.to_string()),
            }
        }
    }
}

/// The archive of `entries`, as a source action packs a revision's files.
///
/// Writing the archive format is the `zip` crate's job, not this emulator's; the entry
/// count and the total size are bounded by the caller before anything is read
/// ([`MAX_SOURCE_FILES`], [`MAX_SOURCE_BYTES`]).
fn zip_of(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut cursor = ::std::io::Cursor::new(Vec::new());
    {
        let mut writer = ::zip::ZipWriter::new(&mut cursor);
        for (path, body) in entries {
            writer
                .start_file(path.as_str(), ::zip::write::SimpleFileOptions::default())
                .map_err(|err| err.to_string())?;
            ::std::io::Write::write_all(&mut writer, body).map_err(|err| err.to_string())?;
        }
        writer.finish().map_err(|err| err.to_string())?;
    }
    Ok(cursor.into_inner())
}

/// The error code S3 answers when the BUCKET a request names is not there at all.
///
/// From S3's own error catalog, where it is the code of "the specified bucket does not
/// exist" — the one answer to a read of an artifact's source that says the source
/// itself, rather than the revision in it, is missing.
const S3_NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// The `FailureType` a source action's failed READ of its object is reported under.
///
/// The OBJECT (at the version the execution pinned, or at its current version) IS the
/// revision an S3 source action sources, so an object the bucket does not hold is a
/// revision that is not available — [`FAILURE_REVISION_UNAVAILABLE`]. A bucket the
/// service does not hold at all is the other fact: the declaration names a source this
/// pipeline cannot read from, which is a configuration error, the same way a repository
/// no registry holds is for the other source providers.
fn source_failure_code<E, R>(error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>) -> &'static str
where
    ::aws_smithy_runtime_api::client::result::SdkError<E, R>:
        ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    match ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(error) {
        Some(S3_NO_SUCH_BUCKET) => FAILURE_CONFIGURATION_ERROR,
        _ => FAILURE_REVISION_UNAVAILABLE,
    }
}

/// Read the source object and write it into the artifact store.
async fn run_source(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
    outputs: &[ArtifactRef],
) -> JobOutcome {
    let read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version_id.map(str::to_string))
        .send()
        .await;
    let object = match read {
        Ok(object) => object,
        Err(err) => {
            return JobOutcome::Failed {
                code: source_failure_code(&err).to_string(),
                message: format!(
                    "the source object s3://{bucket}/{key} could not be read: {}",
                    service_error_detail(&err)
                ),
                external_id: None,
            }
        }
    };
    let version = object.version_id().map(str::to_string);
    let etag = object.e_tag().map(|etag| etag.trim_matches('"').to_string());
    let bytes = match object.body.collect().await {
        Ok(bytes) => bytes.into_bytes(),
        Err(err) => {
            return JobOutcome::Failed {
                code: FAILURE_REVISION_UNAVAILABLE.to_string(),
                message: format!("the source object s3://{bucket}/{key} could not be read: {err}"),
                external_id: None,
            }
        }
    };
    for output in outputs {
        if let Err(err) = encrypted_put(
            client
                .put_object()
                .bucket(&output.bucket)
                .key(&output.key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(bytes.clone())),
            output.encryption.as_deref(),
        )
        .send()
        .await
        {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the artifact could not be written to s3://{}/{}: {}",
                    output.bucket,
                    output.key,
                    service_error_detail(&err)
                ),
                external_id: None,
            };
        }
    }
    let mut variables = BTreeMap::new();
    variables.insert("BucketName".to_string(), bucket.to_string());
    variables.insert("ObjectKey".to_string(), key.to_string());
    if let Some(version) = &version {
        variables.insert("VersionId".to_string(), version.clone());
    }
    if let Some(etag) = &etag {
        variables.insert("ETag".to_string(), etag.clone());
    }
    JobOutcome::Done {
        summary: format!("Source object s3://{bucket}/{key} delivered as the output artifact"),
        revision: version.or(etag),
        variables,
        // The object's own version is the revision, not an external run: no system
        // outside this emulator performed the action's work.
        external_id: None,
    }
}

/// Read the input artifact and write the delivered object(s).
async fn run_deploy(
    client: &::aws_sdk_s3::Client,
    input: &ArtifactRef,
    bucket: &str,
    key: Option<&str>,
    extract: bool,
    encryption: Option<&str>,
) -> JobOutcome {
    let read = client
        .get_object()
        .bucket(&input.bucket)
        .key(&input.key)
        .send()
        .await;
    let object = match read {
        Ok(object) => object,
        Err(err) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the input artifact s3://{}/{} could not be read: {}",
                    input.bucket,
                    input.key,
                    service_error_detail(&err)
                ),
                external_id: None,
            }
        }
    };
    let bytes = match object.body.collect().await {
        Ok(bytes) => bytes.into_bytes(),
        Err(err) => {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!("the input artifact could not be read: {err}"),
                external_id: None,
            }
        }
    };

    // The prefix an extracting deploy delivers its entries beneath, or the ONE key an
    // unextracted artifact is delivered under — which a declaration has to state,
    // because nothing else says where that object goes. The planner refuses that case
    // before the job is claimed, so this is the same refusal rather than a panic.
    let prefix = key.unwrap_or_default();
    let entries: Vec<(String, Vec<u8>)> = if extract {
        // Decoding an archive is synchronous CPU work over caller content, so it runs on
        // the blocking pool rather than on the worker serving this request: the async
        // task stays cancellable while it runs.
        let archive = bytes.clone();
        let decoded = match ::tokio::task::spawn_blocking(move || unzip(&archive)).await {
            Ok(decoded) => decoded,
            Err(err) => Err(format!("the extraction did not complete: {err}")),
        };
        match decoded {
            Ok(entries) => entries,
            Err(detail) => {
                return JobOutcome::Failed {
                    code: FAILURE_JOB_FAILED.to_string(),
                    message: format!(
                        "the artifact could not be extracted before deploy: {detail}"
                    ),
                    external_id: None,
                }
            }
        }
        .into_iter()
        .map(|(name, body)| (extracted_key(prefix, &name), body))
        .collect()
    } else {
        let Some(key) = key else {
            return JobOutcome::Failed {
                code: FAILURE_CONFIGURATION_ERROR.to_string(),
                message: "an S3 deploy action that does not extract the artifact must \
                          declare the configuration key ObjectKey"
                    .to_string(),
                external_id: None,
            };
        };
        vec![(key.to_string(), bytes.to_vec())]
    };

    let count = entries.len();
    for (object_key, body) in entries {
        if let Err(err) = encrypted_put(
            client
                .put_object()
                .bucket(bucket)
                .key(&object_key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(body)),
            encryption,
        )
        .send()
        .await
        {
            return JobOutcome::Failed {
                code: FAILURE_JOB_FAILED.to_string(),
                message: format!(
                    "the artifact could not be deployed to s3://{bucket}/{object_key}: {}",
                    service_error_detail(&err)
                ),
                external_id: None,
            };
        }
    }
    JobOutcome::Done {
        summary: format!("Deployed {count} object(s) to s3://{bucket}/{prefix}"),
        revision: input.revision.clone(),
        variables: BTreeMap::new(),
        // The delivery ran here, not in a system outside this emulator.
        external_id: None,
    }
}

/// Every file entry of a zip archive, as `(name, bytes)`.
///
/// Reading the archive format is the `zip` crate's job, not this emulator's. What this
/// function owns is the BOUNDS, because the archive is caller content and a deflate
/// stream expands by up to ~1000:1: the entry count is capped by
/// [`MAX_ARCHIVE_ENTRIES`], each entry is read through a LIMITED reader so no single
/// entry may exceed [`MAX_EXTRACTED_ENTRY_BYTES`] however its header describes itself,
/// and the running total is capped by [`MAX_EXTRACTED_BYTES`]. Past any bound the
/// archive is refused, so nothing larger than the bound is ever allocated.
///
/// An entry whose name would escape the deploy prefix (an absolute path, or one
/// containing `..`) is refused rather than written somewhere the declaration did not
/// name.
fn unzip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes))
        .map_err(|err| err.to_string())?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "the archive carries {} entries, past the {MAX_ARCHIVE_ENTRIES} entries this \
             emulator extracts",
            archive.len()
        ));
    }
    let mut entries = Vec::new();
    let mut extracted = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| err.to_string())?;
        // zip 9 reports the entry's name as a `Result`, because an entry name that is
        // not valid UTF-8 cannot be a key here at all: such an entry is refused rather
        // than delivered under a lossily decoded name.
        let name = entry
            .name()
            .map_err(|err| format!("an archive entry's name could not be read: {err}"))?
            .to_string();
        if name.ends_with('/') {
            continue;
        }
        if name.starts_with('/') || name.split('/').any(|segment| segment == "..") {
            return Err(format!("the archive entry {name} names a path outside the prefix"));
        }
        // The remaining budget plus one byte: reading MORE than the budget is what says
        // the entry is over it, and the limited reader stops there whatever the entry's
        // header claimed.
        let remaining = MAX_EXTRACTED_BYTES.saturating_sub(extracted);
        let allowed = remaining.min(MAX_EXTRACTED_ENTRY_BYTES);
        let mut body = Vec::new();
        ::std::io::Read::take(&mut entry, allowed as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|err| err.to_string())?;
        if body.len() > allowed {
            return Err(format!(
                "extracting the archive entry {name} would take the extraction past the \
                 {MAX_EXTRACTED_BYTES} bytes this emulator extracts from one artifact"
            ));
        }
        extracted += body.len();
        entries.push((name, body));
    }
    Ok(entries)
}

/// The key one extracted entry is delivered under: the declared `ObjectKey` is the
/// prefix every entry lands beneath, and a declaration that names none delivers each
/// entry under its own name at the root of the bucket.
fn extracted_key(prefix: &str, name: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}
