// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue trigger-start slice: `StartTrigger`, the `GetTrigger`
//! that reports the state it left the trigger in, and the workflow reads that report the
//! run an on-demand trigger's firing began.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a start addresses a trigger by name in the Region that holds it;
//! a trigger that waits on something outside the caller — its schedule, its predicate or
//! the events it batches — is left ARMED in the one `TriggerState` of a started trigger
//! (`ACTIVATED`) and fires nothing here, while a trigger that waits on nothing but a caller
//! — an `ON_DEMAND` one — is FIRED there and then: it initiates the actions it names,
//! beginning a run of each job they name and a run of the workflow it belongs to, and is
//! left in the state it waits in (`CREATED`), ready for the next caller.
//! A start for a name the Region does not hold is refused with `EntityNotFoundException`,
//! and a firing past the concurrency ceiling the workflow named with
//! `ConcurrentRunsExceededException`, leaving the workflow with the runs it already had.
//!
//! The tables are rows along what the rule generalises over: the trigger type (each of the
//! four modelled `TriggerType` values), how many actions the trigger initiates and of
//! which kind, whether it belongs to a workflow at all, and what concurrency that workflow
//! allows.
//!
//! The due instants the run lifecycle turns on are read and moved THROUGH THE STORE rather
//! than slept for, so the transient window is deterministic.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, TRIGGER_STATE_ACTIVATED, TRIGGER_STATE_CREATED,
    WORKFLOW_RUN_ID_PREFIX, WORKFLOW_RUN_STATUS_COMPLETED, WORKFLOW_RUN_STATUS_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::assert_refusal;
use crate::job_runs::{
    already_past, emulator_with_engine, far_future, RUN_AT_REST_DEADLINE, RUN_POLL_INTERVAL,
};
use crate::wire::{call_in_region, ok, REGION};

/// The job a trigger action names. An action names a job of the Region the create is
/// served in, and which job it is is not an axis of the rule under test.
pub(crate) const JOB: &str = "trigger-start-target-job";

/// A second job, so a row can initiate more than one action, or a conditional trigger's
/// predicate can wait on a job none of its actions runs ("Job cannot be a Predicate and
/// Action for Conditional Trigger.").
pub(crate) const OTHER_JOB: &str = "trigger-start-other-job";

/// The crawler an action can name instead of a job, so a row can carry two actions of
/// different kinds.
pub(crate) const CRAWLER: &str = "trigger-start-target-crawler";

/// Register the jobs and the crawler the trigger definitions below name, so their actions
/// resolve.
pub(crate) async fn register_targets(router: &MultiServiceRouter) {
    for job in [JOB, OTHER_JOB] {
        ok(
            router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": "arn:aws:iam::000000000000:role/sim2real-glue-job-role",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            }),
        )
        .await;
    }
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": CRAWLER,
            "Role": "arn:aws:iam::000000000000:role/sim2real-crawler-role",
            "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
        }),
    )
    .await;
}

/// One row of the start table: the workflow the trigger belongs to (none for a trigger
/// bound to no workflow), the trigger's definition, and what starting it must do.
struct StartCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The `CreateWorkflow` body, minus `Name`, or `None` for a trigger that belongs to no
    /// workflow.
    workflow: Option<Value>,
    /// The `CreateTrigger` body, minus `Name` and `WorkflowName`.
    trigger: Value,
    /// Whether starting this trigger begins a run of its workflow: it fires as soon as it
    /// is started AND it belongs to a workflow.
    fires: bool,
    /// The `TriggerState` a read reports once the start has been served: the state the
    /// trigger's own type is left in by a start.
    state_after: &'static str,
    /// `WorkflowRunStatistics$TotalActions` of the run a firing begins: the actions the
    /// trigger initiates.
    total_actions: i64,
}

/// The rows: each modelled `TriggerType`, one and two actions, a job action and a crawler
/// one, a trigger inside a workflow and one outside any.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "on-demand, one job action, in a workflow",
            workflow: Some(json!({})),
            trigger: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
            fires: true,
            state_after: TRIGGER_STATE_CREATED,
            total_actions: 1,
        },
        StartCase {
            case: "on-demand, a job and a crawler action, in a workflow",
            workflow: Some(json!({ "DefaultRunProperties": { "env": "test" } })),
            trigger: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            fires: true,
            state_after: TRIGGER_STATE_CREATED,
            total_actions: 2,
        },
        StartCase {
            case: "on-demand, three actions, in a workflow",
            workflow: Some(json!({})),
            trigger: json!({
                "Type": "ON_DEMAND",
                "Actions": [
                    { "JobName": JOB },
                    { "JobName": OTHER_JOB, "Timeout": 30 },
                    { "CrawlerName": CRAWLER },
                ],
            }),
            fires: true,
            state_after: TRIGGER_STATE_CREATED,
            total_actions: 3,
        },
        StartCase {
            case: "on-demand, outside any workflow",
            workflow: None,
            trigger: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
            fires: false,
            state_after: TRIGGER_STATE_CREATED,
            total_actions: 0,
        },
        StartCase {
            // "This field is required when the trigger type is SCHEDULED."
            case: "scheduled, fires on its cron expression",
            workflow: Some(json!({})),
            trigger: json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "Actions": [{ "JobName": JOB }],
            }),
            fires: false,
            state_after: TRIGGER_STATE_ACTIVATED,
            total_actions: 0,
        },
        StartCase {
            // "This field is required when the trigger type is CONDITIONAL."
            case: "conditional, fires on its predicate",
            workflow: Some(json!({})),
            trigger: json!({
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": JOB,
                        "State": "SUCCEEDED",
                    }],
                },
                "Actions": [{ "CrawlerName": CRAWLER }],
            }),
            fires: false,
            state_after: TRIGGER_STATE_ACTIVATED,
            total_actions: 0,
        },
        StartCase {
            // "Batch condition that must be met … before EventBridge event trigger fires."
            case: "event, fires on the events it batches",
            workflow: Some(json!({})),
            trigger: json!({
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 3, "BatchWindow": 900 },
                "Actions": [{ "JobName": JOB }],
            }),
            fires: false,
            state_after: TRIGGER_STATE_ACTIVATED,
            total_actions: 0,
        },
    ]
}

/// Register the row's workflow (when it has one) and its trigger, and answer the names
/// they were registered under. `index` keeps each row's resources to itself, so one row's
/// runs are never counted for another.
async fn register_case(
    router: &MultiServiceRouter,
    index: usize,
    row: &StartCase,
) -> (Option<String>, String) {
    let workflow = match row.workflow.as_ref() {
        Some(workflow) => {
            let name = format!("trigger-start-workflow-{index}");
            let mut body = workflow.clone();
            body["Name"] = json!(name);
            ok(router, "CreateWorkflow", &body).await;
            Some(name)
        }
        None => None,
    };

    let trigger = format!("trigger-start-{index}");
    let mut body = row.trigger.clone();
    body["Name"] = json!(trigger);
    if let Some(workflow) = workflow.as_deref() {
        body["WorkflowName"] = json!(workflow);
    }
    ok(router, "CreateTrigger", &body).await;
    (workflow, trigger)
}

/// Start `trigger` and answer the HTTP status and body.
async fn start_trigger(router: &MultiServiceRouter, trigger: &str) -> (u16, Value) {
    call_in_region(router, REGION, "StartTrigger", &json!({ "Name": trigger })).await
}

/// The `State` `GetTrigger` reports for `trigger`.
pub(crate) async fn trigger_state(router: &MultiServiceRouter, trigger: &str) -> String {
    let body = ok(router, "GetTrigger", &json!({ "Name": trigger })).await;
    body["Trigger"]
        .get("State")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetTrigger must report a State, got {body}"))
        .to_string()
}

/// The `LastRun` `GetWorkflow` reports for `workflow`, or `Value::Null` when it reports
/// none.
async fn last_run(router: &MultiServiceRouter, workflow: &str) -> Value {
    let body = ok(router, "GetWorkflow", &json!({ "Name": workflow })).await;
    body["Workflow"]
        .get("LastRun")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The ids of the runs `workflow` holds, as the store holds them.
pub(crate) fn stored_run_ids(
    engine: &Arc<engine::InMemoryEngine>,
    workflow: &str,
) -> Vec<String> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store
        .workflow(workflow)
        .map(|entry| entry.runs.iter().map(|run| run.id.clone()).collect())
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"))
}

/// Move EVERY due instant an unsettled run of `workflow` still owes, and every one the job
/// runs those runs' actions began still owe, to `at` — so a test can hold the transient
/// window open or make the owed transition due without sleeping.
///
/// The actions move WITH the run because a run is a composite of them: it comes to rest only
/// once the runs it began have. A job run owes up to TWO states at once — the state it is in
/// while its script runs, and the state its own execution came to — so this moves both, as
/// `job_runs::set_due_instant` does for a job's own runs: a helper that moved only the first
/// would leave the run half way, in flight for ever, and the workflow run waiting on it.
pub(crate) fn set_due_instant(
    engine: &Arc<engine::InMemoryEngine>,
    workflow: &str,
    at: SystemTime,
) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let actions: Vec<(String, String)> = store
        .workflow(workflow)
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"))
        .runs
        .iter()
        .flat_map(|run| run.action_job_runs.iter())
        .map(|begun| (begun.job_name.clone(), begun.run_id.clone()))
        .collect();
    for (job_name, run_id) in actions {
        if let Some(job_run) = store
            .job_mut(REGION, &job_name)
            .and_then(|job| job.run_mut(&run_id))
        {
            if job_run.settles_at.is_some() {
                job_run.settles_at = Some(at);
            }
            if job_run.rests_at.is_some() {
                job_run.rests_at = Some(at);
            }
        }
    }
    let entry = store
        .workflow_mut(workflow)
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"));
    for run in &mut entry.runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(at);
        }
    }
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// A start answers 200 with the trigger's name, leaves the trigger in the state a start of
/// its own type leaves it in, and begins a run of the trigger's workflow exactly when the
/// trigger is one that fires as soon as it is started.
#[tokio::test]
async fn a_start_activates_the_trigger_and_fires_the_on_demand_ones() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, row) in start_cases().iter().enumerate() {
        let case = row.case;
        let (workflow, trigger) = register_case(&router, index, row).await;

        // The create did not ask for the trigger to be started, so it is registered idle:
        // the state the start has to move.
        let before = trigger_state(&router, &trigger).await;
        assert_eq!(
            before, TRIGGER_STATE_CREATED,
            "case {case:?}: a trigger the create did not start is registered in \
             {TRIGGER_STATE_CREATED}, got {before:?}"
        );

        let (status, body) = start_trigger(&router, &trigger).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartTrigger must answer 200, got {status} with {body}"
        );
        assert_eq!(
            body.get("Name").and_then(Value::as_str),
            Some(trigger.as_str()),
            "case {case:?}: the start answers the name of the trigger it started, got {body}"
        );

        let after = trigger_state(&router, &trigger).await;
        assert_eq!(
            after, row.state_after,
            "case {case:?}: a started trigger of this type is in {:?}, got {after:?}",
            row.state_after
        );

        let Some(workflow) = workflow else {
            continue;
        };
        let run = last_run(&router, &workflow).await;
        if !row.fires {
            assert!(
                run.is_null(),
                "case {case:?}: a trigger that waits for something outside the caller begins \
                 no run when started, got {run}"
            );
            continue;
        }

        let run_id = run
            .get("WorkflowRunId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: firing the trigger must begin a run, got {run}")
            })
            .to_string();
        assert!(
            run_id.starts_with(WORKFLOW_RUN_ID_PREFIX),
            "case {case:?}: a workflow run id carries the {WORKFLOW_RUN_ID_PREFIX:?} prefix, \
             got {run_id:?}"
        );
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(WORKFLOW_RUN_STATUS_RUNNING),
            "case {case:?}: the run the firing began is in flight, got {run}"
        );
        assert_eq!(
            run["Statistics"].get("TotalActions").and_then(Value::as_i64),
            Some(row.total_actions),
            "case {case:?}: the run counts the actions its trigger initiates, got {run}"
        );

        // The run the firing began is the run the run read reports under that id.
        let read = ok(
            &router,
            "GetWorkflowRun",
            &json!({ "Name": workflow, "RunId": run_id }),
        )
        .await;
        assert_eq!(
            read["Run"].get("WorkflowRunId").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case:?}: the run must read back under the id it was given, got {read}"
        );

        // The run comes to rest once the runs its actions began have, and what each of them
        // came to is reported by the run's statistics rather than by its status. A run's
        // actions run their course in the store, and the job script an action began is
        // executed off this task, so the run is POLLED for that condition — the owed span
        // made due through the store on every turn rather than slept out — instead of being
        // assumed to have reached it by the first read, which is what a client polling the
        // run does and what keeps the outcome independent of what the host was doing
        // meanwhile.
        let settled = settled_run(&router, &engine, case, &workflow, &run_id).await;
        assert_eq!(
            settled["Run"].get("Status").and_then(Value::as_str),
            Some(WORKFLOW_RUN_STATUS_COMPLETED),
            "case {case:?}: a run whose actions have run their course comes to rest in \
             {WORKFLOW_RUN_STATUS_COMPLETED}, got {settled}"
        );
        assert!(
            settled["Run"].get("ErrorMessage").is_none_or(Value::is_null),
            "case {case:?}: a run that was started and ran its course reports no error, got \
             {settled}"
        );
    }
}

/// Read `run_id` of `workflow` back until the run has come to REST, answering it as
/// `GetWorkflowRun` reports it then.
///
/// A workflow run is a composite: it comes to rest only once the job runs its actions began
/// have, and a job run's script is executed off the task that began it. So the condition is
/// polled for — the owed instants made due through the store on every turn, which is what
/// keeps the suite from sleeping for a span — and a run still in flight when the generous
/// window closes fails the case instead of hanging. The same shape, and the same two
/// constants, as [`crate::job_runs::run_at_rest`].
async fn settled_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    workflow: &str,
    run_id: &str,
) -> Value {
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    loop {
        set_due_instant(engine, workflow, already_past());
        let read = ok(
            router,
            "GetWorkflowRun",
            &json!({ "Name": workflow, "RunId": run_id }),
        )
        .await;
        if read["Run"].get("Status").and_then(Value::as_str) != Some(WORKFLOW_RUN_STATUS_RUNNING) {
            return read;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case:?}: run {run_id:?} of workflow {workflow:?} must come to rest, and it \
             reports {read}"
        );
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
}

/// Every firing of an on-demand trigger begins ANOTHER run: a start is not idempotent, so
/// the workflow holds one run per start, each under its own id.
#[tokio::test]
async fn each_firing_begins_another_run_of_the_workflow() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, row) in start_cases().iter().enumerate() {
        let case = row.case;
        let (workflow, trigger) = register_case(&router, index, row).await;
        let Some(workflow) = workflow else {
            continue;
        };

        for start in 1..=3 {
            let (status, body) = start_trigger(&router, &trigger).await;
            assert_eq!(
                status, 200,
                "case {case:?}: start {start} must answer 200, got {status} with {body}"
            );
            // Hold the runs already begun in flight, so a count is of what the starts
            // recorded and not of what the clock has since settled.
            set_due_instant(&engine, &workflow, far_future());

            let ids = stored_run_ids(&engine, &workflow);
            let expected = if row.fires { start } else { 0 };
            assert_eq!(
                ids.len(),
                expected,
                "case {case:?}: after {start} start(s) the workflow must hold {expected} \
                 run(s), got {ids:?}"
            );
            let mut distinct = ids.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(
                distinct.len(),
                ids.len(),
                "case {case:?}: every run carries its own id, got {ids:?}"
            );
        }
    }
}

/// A firing that would take a workflow past the concurrency ceiling it named is refused
/// with the modelled `ConcurrentRunsExceededException`, and the workflow keeps the runs it
/// already had; once the run in flight has finished, the next start fires again.
#[tokio::test]
async fn a_firing_past_the_workflow_concurrency_ceiling_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;

    // The rows differ along the ceiling the workflow named: one run at a time, two, and a
    // stored zero, which is held at one so a workflow cannot be made unrunnable.
    for (index, (case, ceiling, allowed)) in [
        ("one run at a time", 1, 1),
        ("two runs at a time", 2, 2),
        ("a stored zero is held at one", 0, 1),
    ]
    .into_iter()
    .enumerate()
    {
        let workflow = format!("ceiling-workflow-{index}");
        ok(
            &router,
            "CreateWorkflow",
            &json!({ "Name": workflow, "MaxConcurrentRuns": ceiling }),
        )
        .await;
        let trigger = format!("ceiling-trigger-{index}");
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": trigger,
                "WorkflowName": workflow,
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            }),
        )
        .await;

        // Fill the workflow to its ceiling, holding every run it began in flight.
        for start in 1..=allowed {
            let (status, body) = start_trigger(&router, &trigger).await;
            assert_eq!(
                status, 200,
                "case {case:?}: start {start} is inside the ceiling, got {status} with {body}"
            );
            set_due_instant(&engine, &workflow, far_future());
        }

        assert_refusal(
            &router,
            case,
            "StartTrigger",
            &json!({ "Name": trigger }),
            "ConcurrentRunsExceededException",
            &format!("Concurrent runs exceeded for workflow: {workflow}."),
        )
        .await;
        let ids = stored_run_ids(&engine, &workflow);
        assert_eq!(
            ids.len(),
            allowed,
            "case {case:?}: the refused firing records no run, got {ids:?}"
        );
        // The refusal left the trigger as any start of an on-demand trigger leaves it: it
        // is the RUN that was refused, and such a trigger is never armed by a start.
        let state = trigger_state(&router, &trigger).await;
        assert_eq!(
            state, TRIGGER_STATE_CREATED,
            "case {case:?}: the trigger stays in the state it waits in, got {state:?}"
        );

        // Once the clock has finished the runs in flight, the next firing begins a run
        // again: the ceiling is on runs in flight, not on starts ever made.
        set_due_instant(&engine, &workflow, already_past());
        let (status, body) = start_trigger(&router, &trigger).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a firing with the ceiling free must answer 200, got {status} with \
             {body}"
        );
        let ids = stored_run_ids(&engine, &workflow);
        assert_eq!(
            ids.len(),
            allowed + 1,
            "case {case:?}: the freed slot took another run, got {ids:?}"
        );
    }
}

/// A start for a name the request's Region holds no trigger under is refused with the
/// modelled `EntityNotFoundException` and the service's fixed trigger miss: a trigger is
/// served only through the endpoint that created it, so a trigger of another Region is such
/// a name.
#[tokio::test]
async fn a_start_for_a_trigger_this_region_does_not_hold_is_refused() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": "elsewhere-trigger",
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": JOB }],
        }),
    )
    .await;

    // The names span what the template generalises over: a plain name, a single
    // character, a mixed-case one, punctuation, and the 255-character ceiling
    // `NameString` carries ("len 1..255", `smithy_query operation=StartTrigger`).
    for (case, name) in [
        ("a name nothing created", "no-such-trigger".to_string()),
        ("a single character", "x".to_string()),
        ("mixed case", "MissingTrigger".to_string()),
        ("punctuation and digits", "team.data_lake-2026/raw".to_string()),
        ("at the 255 ceiling", "n".repeat(255)),
    ] {
        assert_refusal(
            &router,
            case,
            "StartTrigger",
            &json!({ "Name": name }),
            "EntityNotFoundException",
            messages::TRIGGER_NOT_FOUND,
        )
        .await;
    }

    // The trigger created in this Region is not startable through another Region's
    // endpoint, and starting it there leaves it untouched here.
    let (status, body) = call_in_region(
        &router,
        "us-west-2",
        "StartTrigger",
        &json!({ "Name": "elsewhere-trigger" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "another Region holds no such trigger, so the start is refused, got {status} with \
         {body}"
    );
    let state = trigger_state(&router, "elsewhere-trigger").await;
    assert_eq!(
        state, TRIGGER_STATE_CREATED,
        "the refused start in another Region must not move this Region's trigger, got \
         {state:?}"
    );
}
