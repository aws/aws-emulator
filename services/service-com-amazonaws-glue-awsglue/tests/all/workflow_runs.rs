// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue workflow-run slice: `StartWorkflowRun` and the
//! `GetWorkflowRun` that reports the run it started.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a workflow can be run only if a trigger bound to it can BEGIN a
//! run — one whose firing does not wait on the workflow's own jobs and crawlers. A start
//! on a workflow with such an entry point records a run of it and answers 200 with the id
//! minted for that run, and `GetWorkflowRun` reports it under that id — in a non-terminal
//! `WorkflowRunStatus` until the status the clock owes it falls due, and in a terminal one
//! afterwards. A start on a workflow with NO entry point is refused with 400 and the
//! modelled `InvalidInputException`, and one past a concurrency ceiling the workflow named
//! with `ConcurrentRunsExceededException`. A run comes to rest once the runs its actions
//! began have come to rest, in the modelled status of a run whose actions have run their
//! course — what each action came to is reported by the run's own statistics and not by its
//! status.
//!
//! The recorded behavioural case — a bare workflow refused for having no starting trigger,
//! then the same workflow started once an on-demand trigger is bound to it — is TWO rows
//! of the tables below; the rows differ along the type of the entry-point trigger, how many
//! actions it initiates, which run properties the workflow and the start carry, the form
//! and length of the workflow name, and the concurrency the workflow allows.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather than
//! slept for, so the transient window is deterministic.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::WORKFLOW_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, TERMINAL_WORKFLOW_RUN_STATUSES, WORKFLOW_RUN_ID_PREFIX,
    WORKFLOW_RUN_STATUS_COMPLETED, WORKFLOW_RUN_STATUS_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, far_future};
use crate::wire::{call, error_code, error_message, ok, REGION};

/// The role ARN the job every trigger action below runs is registered with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The job a trigger action names. One job, shared: an action names a job of the Region the
/// create is served in, and which job it is is not an axis of the rule under test.
const JOB: &str = "workflow-run-target-job";

/// The job a conditional trigger's predicate waits on. A conditional trigger may not name
/// one job both in its `Predicate` and in its `Actions` ("Job cannot be a Predicate and
/// Action for Conditional Trigger."), so the predicates here wait on a job no action runs.
const PREDICATE_JOB: &str = "workflow-run-predicate-job";

/// The crawler an action can name instead of a job, so a row can carry two actions of
/// different kinds.
const CRAWLER: &str = "workflow-run-target-crawler";

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=StartWorkflowRun`).
const MAX_NAME_LENGTH: usize = 255;

/// Register the job and the crawler the trigger definitions below name, so their actions
/// resolve.
async fn register_targets(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": JOB,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            // Room for every row's entry point to begin its one run of this shared job,
            // so the action counts a run reports are not bounded by a slot an earlier
            // row's run is still holding. The ceiling's own effect on a firing is the
            // subject of `trigger_action_runs`.
            "ExecutionProperty": { "MaxConcurrentRuns": 10 },
        }),
    )
    .await;
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": PREDICATE_JOB,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
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

/// Bind `trigger` to `workflow` under `name` and create it.
async fn bind_trigger(
    router: &MultiServiceRouter,
    name: &str,
    workflow: &str,
    trigger: &Value,
) {
    let mut body = trigger.clone();
    body["Name"] = json!(name);
    body["WorkflowName"] = json!(workflow);
    ok(router, "CreateTrigger", &body).await;
}

/// Start a run of `workflow` and answer the `RunId` the start reports.
async fn start_run(router: &MultiServiceRouter, workflow: &str, start: &Value) -> String {
    let mut body = start.clone();
    body["Name"] = json!(workflow);
    let started = ok(router, "StartWorkflowRun", &body).await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartWorkflowRun must answer a RunId, got {started}"))
        .to_string()
}

/// The `WorkflowRun` structure `GetWorkflowRun` reports for one run.
async fn get_run(router: &MultiServiceRouter, workflow: &str, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetWorkflowRun",
        &json!({ "Name": workflow, "RunId": run_id }),
    )
    .await;
    body.get("Run")
        .cloned()
        .unwrap_or_else(|| panic!("GetWorkflowRun must report a Run, got {body}"))
}

/// The run properties a response reports, with an absent map read as the empty one awsJson
/// may omit.
fn run_properties(run: &Value) -> Value {
    match run.get("WorkflowRunProperties") {
        None | Some(Value::Null) => json!({}),
        Some(properties) => properties.clone(),
    }
}

/// Move the due instant of every unsettled run of `workflow`, and every due instant the job
/// runs those runs' actions began still owe, to `at` — so a test can hold the transient
/// window open or make the owed transition due without sleeping.
///
/// The actions move WITH the run because a run is a composite of them: it comes to rest only
/// once the runs it began have, so moving its own instant alone would leave it waiting. A job
/// run owes up to TWO states at once — the state it is in while its script runs, and the
/// state its own execution came to — so both of its instants move, as
/// `job_runs::set_due_instant` does for a job's own runs.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, workflow: &str, at: SystemTime) {
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

/// One row of the start table: a workflow, the entry point bound to it, the start made of
/// it, and what the recorded run must then report.
struct StartCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The `CreateWorkflow` body.
    workflow: Value,
    /// The `CreateTrigger` body of the entry point, minus `Name` and `WorkflowName`.
    entry_point: Value,
    /// Further triggers bound to the same workflow, which must not change the outcome.
    beside: Vec<Value>,
    /// The `StartWorkflowRun` body, minus `Name`.
    start: Value,
    /// The run properties the recorded run must carry.
    properties: Value,
    /// `Statistics.TotalActions`: the actions the entry point initiates.
    total_actions: i64,
}

/// An `ON_DEMAND` trigger definition running `JOB`: the entry point a client binds to a
/// workflow it means to start by hand.
fn on_demand() -> Value {
    json!({ "Type": "ON_DEMAND", "Actions": [{ "JobName": JOB }] })
}

/// The rows the start suites drive, one per axis the rule generalises over.
///
/// The trigger types are the modelled `TriggerType` values
/// (`smithy_query operation=CreateTrigger`): `ON_DEMAND`, `SCHEDULED` and `EVENT` each fire
/// on something outside the workflow, so each is an entry point, while `CONDITIONAL` waits
/// on the workflow's own components and is covered by the refusal table below.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            // The recorded case: a bare workflow with an on-demand trigger bound to it.
            case: "an on-demand entry point and no run properties",
            workflow: json!({ "Name": "wf-on-demand" }),
            entry_point: on_demand(),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({}),
            total_actions: 1,
        },
        StartCase {
            case: "a scheduled entry point",
            workflow: json!({ "Name": "wf-scheduled" }),
            entry_point: json!({
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "Actions": [{ "JobName": JOB }],
            }),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({}),
            total_actions: 1,
        },
        StartCase {
            case: "an event entry point",
            workflow: json!({ "Name": "wf-event" }),
            entry_point: json!({ "Type": "EVENT", "Actions": [{ "JobName": JOB }] }),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({}),
            total_actions: 1,
        },
        StartCase {
            case: "run properties named by the start",
            workflow: json!({ "Name": "wf-properties" }),
            entry_point: on_demand(),
            beside: Vec::new(),
            start: json!({ "RunProperties": { "shard": "3", "mode": "backfill" } }),
            properties: json!({ "shard": "3", "mode": "backfill" }),
            total_actions: 1,
        },
        StartCase {
            case: "the workflow's default run properties, with none on the start",
            workflow: json!({
                "Name": "wf-defaults",
                "DefaultRunProperties": { "env": "staging", "mode": "full" },
            }),
            entry_point: on_demand(),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({ "env": "staging", "mode": "full" }),
            total_actions: 1,
        },
        StartCase {
            case: "the start's run properties over the workflow's defaults",
            workflow: json!({
                "Name": "wf-override",
                "DefaultRunProperties": { "env": "staging", "mode": "full" },
            }),
            entry_point: on_demand(),
            beside: Vec::new(),
            start: json!({ "RunProperties": { "env": "prod" } }),
            properties: json!({ "env": "prod", "mode": "full" }),
            total_actions: 1,
        },
        StartCase {
            case: "an entry point initiating two actions of different kinds",
            workflow: json!({ "Name": "wf-two-actions" }),
            entry_point: json!({
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            }),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({}),
            total_actions: 2,
        },
        StartCase {
            case: "an entry point beside a conditional trigger of the same workflow",
            workflow: json!({ "Name": "wf-mixed" }),
            entry_point: on_demand(),
            beside: vec![json!({
                "Type": "CONDITIONAL",
                "Actions": [{ "JobName": JOB }],
                "Predicate": {
                    "Conditions": [{
                        "LogicalOperator": "EQUALS",
                        "JobName": PREDICATE_JOB,
                        "State": "SUCCEEDED",
                    }],
                },
            })],
            start: json!({}),
            properties: json!({}),
            total_actions: 1,
        },
        StartCase {
            case: "a workflow name at the modelled length bound",
            workflow: json!({ "Name": "w".repeat(MAX_NAME_LENGTH) }),
            entry_point: on_demand(),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({}),
            total_actions: 1,
        },
        StartCase {
            case: "a workflow name carrying a space and a tab, with a concurrency ceiling",
            workflow: json!({ "Name": "workflow one\ttwo", "MaxConcurrentRuns": 4 }),
            entry_point: on_demand(),
            beside: Vec::new(),
            start: json!({}),
            properties: json!({}),
            total_actions: 1,
        },
    ]
}

/// Register a row's workflow and the triggers bound to it, answering the workflow's name.
async fn register_case(router: &MultiServiceRouter, index: usize, row: &StartCase) -> String {
    ok(router, "CreateWorkflow", &row.workflow).await;
    let workflow = row.workflow["Name"]
        .as_str()
        .expect("the row carries a Name")
        .to_string();
    for (beside, trigger) in row.beside.iter().enumerate() {
        bind_trigger(
            router,
            &format!("beside-trigger-{index}-{beside}"),
            &workflow,
            trigger,
        )
        .await;
    }
    bind_trigger(
        router,
        &format!("entry-trigger-{index}"),
        &workflow,
        &row.entry_point,
    )
    .await;
    workflow
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// What `StartWorkflowRun` records is what `GetWorkflowRun` reports: a fresh run id, the
/// workflow it is a run of, the run properties the workflow's defaults and the start
/// together name, and the actions the entry point initiates — in the non-terminal status a
/// started run is in while the clock still owes it a transition.
#[tokio::test]
async fn a_started_workflow_run_is_read_back_from_the_store() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;
    let mut minted: Vec<String> = Vec::new();

    for (index, row) in start_cases().iter().enumerate() {
        let case = row.case;
        let workflow = register_case(&router, index, row).await;

        let run_id = start_run(&router, &workflow, &row.start).await;
        // Hold the transient window open, so what is asserted below is the status a start
        // leaves a run in and not whatever the clock has since made due.
        set_due_instant(&engine, &workflow, far_future());
        // The run the entry point's one job action began is held in flight for the same
        // reason: this row asserts the action counts a start leaves, not what the clock has
        // since made of that run.
        crate::job_runs::set_due_instant(&engine, JOB, far_future());

        assert!(
            run_id.starts_with(WORKFLOW_RUN_ID_PREFIX),
            "case {case}: a minted run id carries the workflow-run prefix, got {run_id:?}"
        );
        minted.push(run_id.clone());

        let run = get_run(&router, &workflow, &run_id).await;
        assert_eq!(
            run.get("WorkflowRunId").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case}: the run reports the id the start minted, got {run}"
        );
        assert_eq!(
            run.get("Name").and_then(Value::as_str),
            Some(workflow.as_str()),
            "case {case}: the run reports the workflow it is a run of, got {run}"
        );
        assert_eq!(
            run.get("Status").and_then(Value::as_str),
            Some(WORKFLOW_RUN_STATUS_RUNNING),
            "case {case}: a run the clock still owes a transition is \
             {WORKFLOW_RUN_STATUS_RUNNING}, got {run}"
        );
        assert_eq!(
            run_properties(&run),
            row.properties,
            "case {case}: expected run properties {}, got {run}",
            row.properties
        );
        let statistics = run
            .get("Statistics")
            .unwrap_or_else(|| panic!("case {case}: a run reports Statistics, got {run}"));
        assert_eq!(
            statistics.get("TotalActions").and_then(Value::as_i64),
            Some(row.total_actions),
            "case {case}: expected {} total actions, got {statistics}",
            row.total_actions
        );
        // The one job action every row's entry point initiates began a run of that job,
        // held in flight above, so the run counts that action as running.
        assert_eq!(
            statistics.get("RunningActions").and_then(Value::as_i64),
            Some(1),
            "case {case}: the run the entry point's job action began is in flight, got \
             {statistics}"
        );
        // Nothing here executes an action and nothing has stopped one, so no action of the
        // run has reached any of the states those produce.
        for counted in [
            "TimeoutActions",
            "FailedActions",
            "StoppedActions",
            "SucceededActions",
            "ErroredActions",
            "WaitingActions",
        ] {
            assert_eq!(
                statistics.get(counted).and_then(Value::as_i64).unwrap_or(0),
                0,
                "case {case}: no action of the run has reached {counted}, got {statistics}"
            );
        }
        let started_on = run
            .get("StartedOn")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: StartedOn must be a timestamp, got {run}"));
        assert!(
            started_on > 0.0,
            "case {case}: expected a wall-clock StartedOn, got {started_on}"
        );
        // A run that has not finished, follows no run and has gone wrong at nothing
        // reports none of the members only those produce.
        for absent in ["CompletedOn", "ErrorMessage", "PreviousRunId", "Graph"] {
            assert!(
                run.get(absent).is_none_or(Value::is_null),
                "case {case}: a run in flight reports no {absent}, got {run}"
            );
        }

        // A read does not change what the next caller sees.
        let again = get_run(&router, &workflow, &run_id).await;
        assert_eq!(
            again, run,
            "case {case}: GetWorkflowRun must be repeatable, got {again}"
        );
    }

    // Every start minted its own id, so no run can be addressed as another.
    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "case every row: each start mints its own run id, got {minted:?}"
    );
}

/// The status the clock owes a started run falls due at the stored instant and not when
/// somebody reads it: the run stays in flight while that instant is ahead, and once it is
/// past the next read reports the terminal status, the reason nothing was executed, and
/// when the run finished — the same way twice.
#[tokio::test]
async fn the_owed_status_falls_due_on_the_clock() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, row) in start_cases().iter().enumerate() {
        let case = row.case;
        let workflow = register_case(&router, index, row).await;
        let run_id = start_run(&router, &workflow, &row.start).await;

        // Ahead of the due instant the run is still in flight, however often it is read.
        set_due_instant(&engine, &workflow, far_future());
        for read in 0..3 {
            let run = get_run(&router, &workflow, &run_id).await;
            assert_eq!(
                run.get("Status").and_then(Value::as_str),
                Some(WORKFLOW_RUN_STATUS_RUNNING),
                "case {case}: read {read} must not move a run whose transition is not due, \
                 got {run}"
            );
        }

        // Past it, the next read reports the status the clock had already reached.
        set_due_instant(&engine, &workflow, already_past());
        let settled = get_run(&router, &workflow, &run_id).await;
        let status = settled.get("Status").and_then(Value::as_str).unwrap_or("");
        assert!(
            TERMINAL_WORKFLOW_RUN_STATUSES.contains(&status),
            "case {case}: expected one of {TERMINAL_WORKFLOW_RUN_STATUSES:?}, got {settled}"
        );
        assert_eq!(
            status, WORKFLOW_RUN_STATUS_COMPLETED,
            "case {case}: the run's actions have run their course, so it comes to rest in \
             {WORKFLOW_RUN_STATUS_COMPLETED}, got {settled}"
        );
        assert!(
            settled.get("ErrorMessage").is_none_or(Value::is_null),
            "case {case}: a run that was started and ran its course reports no error, got \
             {settled}"
        );
        assert!(
            settled.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {case}: a finished run reports when it finished, got {settled}"
        );

        // A settled run is settled: reading it again reports the same run.
        let again = get_run(&router, &workflow, &run_id).await;
        assert_eq!(
            again, settled,
            "case {case}: a settled run must read the same twice; first {settled}, then \
             {again}"
        );

        // The workflow's own read reports that run as its last execution.
        let body = ok(&router, "GetWorkflow", &json!({ "Name": workflow })).await;
        let last_run = body["Workflow"].get("LastRun").cloned().unwrap_or(Value::Null);
        assert_eq!(
            last_run.get("WorkflowRunId").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case}: the workflow reports its last execution, got {body}"
        );
        assert_eq!(
            last_run.get("Status").and_then(Value::as_str),
            Some(WORKFLOW_RUN_STATUS_COMPLETED),
            "case {case}: the last execution carries the status the clock reached, got {body}"
        );
    }
}

/// `IncludeGraph` decides whether the run read carries the workflow graph at all: a reader
/// that asked for none is answered without one, and one that asked for it gets the graph of
/// the components the workflow holds.
#[tokio::test]
async fn the_run_graph_is_reported_only_when_the_read_asks_for_it() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, row) in start_cases().iter().enumerate() {
        let case = row.case;
        let workflow = register_case(&router, index, row).await;
        let run_id = start_run(&router, &workflow, &row.start).await;

        for (variant, include_graph) in [
            ("no IncludeGraph member", Value::Null),
            ("IncludeGraph false", json!(false)),
        ] {
            let mut request = json!({ "Name": workflow, "RunId": run_id });
            if !include_graph.is_null() {
                request["IncludeGraph"] = include_graph;
            }
            let body = ok(&router, "GetWorkflowRun", &request).await;
            assert!(
                body["Run"].get("Graph").is_none_or(Value::is_null),
                "case {case} / {variant}: expected no Graph, got {body}"
            );
        }

        let body = ok(
            &router,
            "GetWorkflowRun",
            &json!({ "Name": workflow, "RunId": run_id, "IncludeGraph": true }),
        )
        .await;
        let graph = body["Run"]
            .get("Graph")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: IncludeGraph must report a Graph, got {body}"));
        for member in ["Nodes", "Edges"] {
            assert!(
                graph.get(member).and_then(Value::as_array).is_some(),
                "case {case}: Graph.{member} must be a list, got {graph}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A workflow nothing can begin a run of is refused: the rows differ along HOW the entry
/// point is missing — no trigger at all, only a trigger that waits on the workflow's own
/// components, and a trigger that could begin a run but belongs to another workflow — and
/// along the form of the workflow name the refusal has to carry.
#[tokio::test]
async fn starting_a_workflow_with_no_entry_point_is_refused() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;
    // The workflow a trigger can be bound to instead of the one being started.
    ok(&router, "CreateWorkflow", &json!({ "Name": "other-workflow" })).await;

    let conditional = json!({
        "Type": "CONDITIONAL",
        "Actions": [{ "JobName": JOB }],
        "Predicate": {
            "Conditions": [{
                "LogicalOperator": "EQUALS",
                "JobName": PREDICATE_JOB,
                "State": "SUCCEEDED",
            }],
        },
    });

    let cases: Vec<(&str, String, Vec<Value>, bool)> = vec![
        ("no trigger at all", "bare-workflow".to_string(), Vec::new(), false),
        (
            "only a conditional trigger",
            "conditional-only-workflow".to_string(),
            vec![conditional.clone()],
            false,
        ),
        (
            "two conditional triggers and nothing else",
            "two-conditionals-workflow".to_string(),
            vec![conditional.clone(), conditional.clone()],
            false,
        ),
        (
            "an on-demand trigger bound to another workflow",
            "unbound-workflow".to_string(),
            Vec::new(),
            true,
        ),
        (
            "a name carrying a space and a tab",
            "bare workflow\tthree".to_string(),
            Vec::new(),
            false,
        ),
        (
            "a name at the modelled length bound",
            "b".repeat(MAX_NAME_LENGTH),
            Vec::new(),
            false,
        ),
    ];

    for (index, (case, name, triggers, bind_elsewhere)) in cases.iter().enumerate() {
        ok(&router, "CreateWorkflow", &json!({ "Name": name })).await;
        for (bound, trigger) in triggers.iter().enumerate() {
            bind_trigger(
                &router,
                &format!("refused-trigger-{index}-{bound}"),
                name,
                trigger,
            )
            .await;
        }
        if *bind_elsewhere {
            bind_trigger(
                &router,
                &format!("elsewhere-trigger-{index}"),
                "other-workflow",
                &on_demand(),
            )
            .await;
        }

        let (status, body) = call(&router, "StartWorkflowRun", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a workflow with no entry point must be refused with 400, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Workflow {name} doesn't have any starting trigger."),
            "case {case}: the refusal names the workflow the request asked to start, got \
             {body}"
        );
    }

    // Binding an entry point to one of the refused workflows is what makes it runnable, so
    // the refusals above are about the missing entry point and not about the workflow.
    bind_trigger(&router, "late-trigger", "bare-workflow", &on_demand()).await;
    let run_id = start_run(&router, "bare-workflow", &json!({})).await;
    let run = get_run(&router, "bare-workflow", &run_id).await;
    assert_eq!(
        run.get("WorkflowRunId").and_then(Value::as_str),
        Some(run_id.as_str()),
        "case an entry point bound late: the start records a run, got {run}"
    );
}

/// A start or a run read for a `Name` this account holds no workflow under is refused with
/// the modelled `EntityNotFoundException`, which is a different refusal from a workflow
/// that exists and has no entry point.
#[tokio::test]
async fn addressing_an_unknown_workflow_is_refused() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;
    ok(&router, "CreateWorkflow", &json!({ "Name": "StoredWorkflow" })).await;

    for (case, name) in [
        ("never created", "no-such-workflow"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Workflow"),
        ("a stored name lowercased", "storedworkflow"),
    ] {
        for operation in ["StartWorkflowRun", "GetWorkflowRun"] {
            let mut request = json!({ "Name": name });
            if operation == "GetWorkflowRun" {
                request["RunId"] = json!("wr_0000000000000000");
            }
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case}: {operation} on {name:?} must be refused with 400, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case}: expected EntityNotFoundException from {operation}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                WORKFLOW_NOT_FOUND,
                "case {case}: expected the recorded refusal for a workflow that resolved to \
                 nothing, got {body}"
            );
        }
    }
}

/// A run is addressed within its own workflow: a `RunId` the named workflow holds no run
/// under is refused with the modelled `EntityNotFoundException`, including the id of a run
/// of ANOTHER workflow.
#[tokio::test]
async fn reading_an_unknown_workflow_run_is_refused() {
    let (router, _engine) = emulator_with_engine();
    register_targets(&router).await;

    for (index, name) in ["run-owner-workflow", "run-other-workflow"]
        .into_iter()
        .enumerate()
    {
        ok(&router, "CreateWorkflow", &json!({ "Name": name })).await;
        bind_trigger(
            &router,
            &format!("owner-trigger-{index}"),
            name,
            &on_demand(),
        )
        .await;
    }
    let owned = start_run(&router, "run-owner-workflow", &json!({})).await;
    let elsewhere = start_run(&router, "run-other-workflow", &json!({})).await;

    for (case, run_id) in [
        ("never minted", "wr_0000000000000000".to_string()),
        ("a prefix of a minted id", owned[..8].to_string()),
        ("the id without its prefix", owned.replace(WORKFLOW_RUN_ID_PREFIX, "")),
        ("a run of another workflow", elsewhere.clone()),
    ] {
        let (status, body) = call(
            &router,
            "GetWorkflowRun",
            &json!({ "Name": "run-owner-workflow", "RunId": run_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {run_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {run_id:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            WORKFLOW_NOT_FOUND,
            "case {case}: a run that resolved to nothing earns the same fixed entity miss, \
             got {body}"
        );
    }

    // The id as minted still resolves, so the misses above are misses and not a broken
    // lookup.
    let run = get_run(&router, "run-owner-workflow", &owned).await;
    assert_eq!(
        run.get("WorkflowRunId").and_then(Value::as_str),
        Some(owned.as_str()),
        "case the minted id: expected the stored run, got {run}"
    );
}

/// A workflow runs at most as many runs at once as it allows: the start at the ceiling
/// succeeds, the one past it is refused with the modelled
/// `ConcurrentRunsExceededException`, and a ceiling the workflow never named admits every
/// start. The rows differ along the ceiling, including a workflow that named none.
#[tokio::test]
async fn a_start_past_the_workflows_concurrency_ceiling_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_targets(&router).await;

    // Each row: the ceiling the workflow names, and how many runs may be in flight at
    // once. A workflow that names none has no limit, so a row stands in for "more starts
    // than any ceiling here would admit".
    let cases: Vec<(&str, &str, Option<i64>, usize)> = vec![
        ("a ceiling of one", "ceiling-one", Some(1), 1),
        ("a ceiling of two", "ceiling-two", Some(2), 2),
        ("a ceiling of five", "ceiling-five", Some(5), 5),
        ("no ceiling at all", "ceiling-none", None, 6),
    ];

    for (index, (case, name, ceiling, admitted)) in cases.iter().enumerate() {
        let mut workflow = json!({ "Name": name });
        if let Some(ceiling) = ceiling {
            workflow["MaxConcurrentRuns"] = json!(ceiling);
        }
        ok(&router, "CreateWorkflow", &workflow).await;
        bind_trigger(
            &router,
            &format!("ceiling-trigger-{index}"),
            name,
            &on_demand(),
        )
        .await;

        // Every start below is held in flight, so what the ceiling is tested against is
        // how many runs are running and not how many were ever started.
        for _ in 0..*admitted {
            start_run(&router, name, &json!({})).await;
            set_due_instant(&engine, name, far_future());
        }

        let (status, body) = call(&router, "StartWorkflowRun", &json!({ "Name": name })).await;
        match ceiling {
            Some(_) => {
                assert_eq!(
                    status, 400,
                    "case {case}: a start past the ceiling must be refused with 400, got \
                     {status} with {body}"
                );
                assert!(
                    error_code(&body).contains("ConcurrentRunsExceededException"),
                    "case {case}: expected ConcurrentRunsExceededException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("Concurrent runs exceeded for workflow: {name}."),
                    "case {case}: the refusal names the workflow whose ceiling was reached, \
                     got {body}"
                );

                // The ceiling bounds the runs IN FLIGHT: once the clock has finished them,
                // a start is admitted again.
                set_due_instant(&engine, name, already_past());
                let run_id = start_run(&router, name, &json!({})).await;
                let run = get_run(&router, name, &run_id).await;
                assert_eq!(
                    run.get("WorkflowRunId").and_then(Value::as_str),
                    Some(run_id.as_str()),
                    "case {case}: a start after the earlier runs finished records a run, got \
                     {run}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: a workflow that named no ceiling admits every start, got \
                     {status} with {body}"
                );
            }
        }
    }
}
