// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a workflow RUN executes, and therefore for the outcome
//! `GetWorkflowRun`, `GetTaskInstance` and `ListTaskInstances` report for it.
//!
//! The rule under test: `StartWorkflowRun` "creates a workflow run that executes the
//! tasks defined in your workflow definition", so a run is the execution of the task
//! graph its version froze — one task instance per task the definition declares, a task
//! running only once the tasks it declared as dependencies have finished — and the status
//! the run finishes in is the one that execution produced: the modelled `SUCCESS` with no
//! `ErrorMessage` when every task instance succeeded ("This value is null if the run
//! completed successfully"), the modelled `FAILED` with the reason when the definition's
//! graph is one no run of it can execute, and the modelled `FAILED` with the task named
//! when a task's own declared program answered failure (the last suite).
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because a
//! definition is a YAML object a run's version snapshotted out of Amazon S3. The table
//! spans what the rule generalises over: how many tasks a definition declares and in which
//! FORM (a sequence of entries, a mapping of them, nested under `dag`, written as JSON),
//! the SHAPE of the dependencies between them (none, a chain, a fan-out and fan-in, a
//! single dependency written as a scalar), and each way a graph can fail to be buildable
//! (a dependency naming no declared task, a cycle, a duplicate identifier, an identifier
//! the modelled `TaskInstanceId` cannot carry).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::task_instance_reads::epoch_seconds;
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router, stage,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::workflow_definition::MAX_DECLARED_TASKS;
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    RUN_ADMITTED_AFTER, RUN_DEQUEUED_AFTER, RUN_STATUS_FAILED, RUN_STATUS_STOPPED,
    RUN_STATUS_SUCCESS, RUN_STOPPED_AFTER, TASK_INSTANCE_STATUS_CANCELLED,
    TASK_INSTANCE_STATUS_FAILED, TASK_INSTANCE_STATUS_RUNNING, TASK_INSTANCE_STATUS_SCHEDULED,
    TASK_INSTANCE_STATUS_UPSTREAM_FAILED, TASK_LEVEL_RUNS_FOR,
};

/// The modelled length of the `GenericString` a run's `ErrorMessage` targets
/// (`smithy_query(query="shape", shape="WorkflowRunDetail")` declares it 1..255), which a
/// reported reason may not exceed however long the task identifier in it is.
const MAX_ERROR_MESSAGE_CHARS: usize = 255;

/// The `TaskInstanceStatus` a task that ran to completion reports
/// (`smithy_query(query="shape", shape="TaskInstanceStatus")`).
const TASK_SUCCESS: &str = "SUCCESS";

/// The `TaskInstanceStatus` a task whose own work answered FAILURE reports, and the one a
/// task that never ran because a task it waits for failed reports — both from the same
/// value set as [`TASK_SUCCESS`], read off this service's own constants so the suite and
/// the emulator cannot spell them differently.
const TASK_FAILED: &str = TASK_INSTANCE_STATUS_FAILED;
const TASK_UPSTREAM_FAILED: &str = TASK_INSTANCE_STATUS_UPSTREAM_FAILED;

/// How closely two timestamps must agree to be the same instant on the wire, in seconds.
const TIMESTAMP_TOLERANCE: f64 = 0.002;

/// One task a row's definition declares, and what the run must therefore report for it.
struct ExpectedTask {
    /// `TaskId`: the task of the definition the run's instance executes. The identifier
    /// that instance is reported UNDER is not this value — it is minted per run, and the
    /// test discovers it through [`instances_by_task`].
    task_id: &'static str,
    /// `OperatorName`, when the definition declares one for the task.
    operator_name: Option<&'static str>,
    /// The tasks this one declared as dependencies: it may not start before each of them
    /// has ended.
    upstream: &'static [&'static str],
}

/// One row: a definition, and the outcome a run of it must report.
struct Row {
    case: &'static str,
    /// The definition content staged in Amazon S3, or `None` for a workflow whose
    /// definition location addresses no object at all.
    definition: Option<&'static str>,
    /// The `RunState` the run must finish in.
    expected_state: &'static str,
    /// Text the reported `ErrorMessage` must contain, for a run that fails; `None` for a
    /// run that must report no `ErrorMessage` at all.
    expected_error: Option<&'static [&'static str]>,
    /// The task instances the run must hold.
    expected_tasks: &'static [ExpectedTask],
}

/// The table. Rows differ along the definition's form, its task count, the shape of its
/// dependency graph and each way that graph can fail to be built — never merely along the
/// workflow's name.
const ROWS: &[Row] = &[
    Row {
        case: "a definition location that addresses no object",
        definition: None,
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[],
    },
    Row {
        case: "a definition declaring an empty task list",
        definition: Some("name: nothingToDo\nschedule: \"@daily\"\ntasks: []\n"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[],
    },
    Row {
        case: "one task",
        definition: Some("tasks:\n  - id: extract\n    operator: EmptyOperator\n"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[ExpectedTask {
            task_id: "extract",
            operator_name: Some("EmptyOperator"),
            upstream: &[],
        }],
    },
    Row {
        case: "a chain of three tasks, one dependency key each",
        definition: Some(
            "tasks:\n  - id: extract\n  - id: transform\n    dependencies: [extract]\n  \
             - id: load\n    depends_on: transform\n",
        ),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[
            ExpectedTask {
                task_id: "extract",
                operator_name: None,
                upstream: &[],
            },
            ExpectedTask {
                task_id: "transform",
                operator_name: None,
                upstream: &["extract"],
            },
            ExpectedTask {
                task_id: "load",
                operator_name: None,
                upstream: &["transform"],
            },
        ],
    },
    Row {
        case: "a fan-out and fan-in over four tasks",
        definition: Some(
            "dag:\n  dag_id: diamond\ntasks:\n  - id: start\n    operator: EmptyOperator\n  \
             - id: left\n    operator: PythonOperator\n    dependencies:\n      - start\n  \
             - id: right\n    operator: BashOperator\n    dependencies:\n      - start\n  \
             - id: join\n    dependencies:\n      - left\n      - right\n",
        ),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[
            ExpectedTask {
                task_id: "start",
                operator_name: Some("EmptyOperator"),
                upstream: &[],
            },
            ExpectedTask {
                task_id: "left",
                operator_name: Some("PythonOperator"),
                upstream: &["start"],
            },
            ExpectedTask {
                task_id: "right",
                operator_name: Some("BashOperator"),
                upstream: &["start"],
            },
            ExpectedTask {
                task_id: "join",
                operator_name: None,
                upstream: &["left", "right"],
            },
        ],
    },
    Row {
        case: "tasks declared under the dag mapping",
        definition: Some(
            "dag:\n  dag_id: nested\n  schedule: \"@hourly\"\n  tasks:\n    - task_id: only\n      \
             type: SqlOperator\n",
        ),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[ExpectedTask {
            task_id: "only",
            operator_name: Some("SqlOperator"),
            upstream: &[],
        }],
    },
    Row {
        case: "tasks written as a mapping from identifier to entry",
        definition: Some(
            "tasks:\n  extract:\n    operator: EmptyOperator\n  load:\n    upstream: extract\n",
        ),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[
            ExpectedTask {
                task_id: "extract",
                operator_name: Some("EmptyOperator"),
                upstream: &[],
            },
            ExpectedTask {
                task_id: "load",
                operator_name: None,
                upstream: &["extract"],
            },
        ],
    },
    Row {
        // YAML 1.2 is a superset of JSON, so a definition written as JSON is one
        // definition more.
        case: "a definition written as JSON",
        definition: Some(
            "{\"tasks\": [{\"name\": \"first\"}, \
             {\"name\": \"second\", \"dependencies\": [\"first\"]}]}",
        ),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[
            ExpectedTask {
                task_id: "first",
                operator_name: None,
                upstream: &[],
            },
            ExpectedTask {
                task_id: "second",
                operator_name: None,
                upstream: &["first"],
            },
        ],
    },
    Row {
        // An entry this reader finds no identifier in is not a task it can run: a task
        // instance is reported BY its identifier.
        case: "an entry declaring no identifier",
        definition: Some("tasks:\n  - operator: EmptyOperator\n"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[],
    },
    Row {
        // The reader follows a document only as far as the parser can read it, so a
        // definition that breaks off mid-way is executed as far as it DECLARED tasks.
        case: "a definition the parser cannot read to the end",
        definition: Some("tasks:\n  - id: alpha\n    operator: EmptyOperator\n  - [one, two"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_error: None,
        expected_tasks: &[ExpectedTask {
            task_id: "alpha",
            operator_name: Some("EmptyOperator"),
            upstream: &[],
        }],
    },
    Row {
        case: "a dependency naming no declared task",
        definition: Some("tasks:\n  - id: transform\n    dependencies: [extract]\n"),
        expected_state: RUN_STATUS_FAILED,
        expected_error: Some(&["extract", "transform"]),
        expected_tasks: &[],
    },
    Row {
        case: "a dependency cycle",
        definition: Some(
            "tasks:\n  - id: first\n    dependencies: [second]\n  - id: second\n    \
             dependencies: [first]\n",
        ),
        expected_state: RUN_STATUS_FAILED,
        expected_error: Some(&["Cycle", "first"]),
        expected_tasks: &[],
    },
    Row {
        case: "a task that depends on itself",
        definition: Some("tasks:\n  - id: loop\n    dependencies: [loop]\n"),
        expected_state: RUN_STATUS_FAILED,
        expected_error: Some(&["Cycle", "loop"]),
        expected_tasks: &[],
    },
    Row {
        case: "two tasks declaring one identifier",
        definition: Some("tasks:\n  - id: twice\n  - id: twice\n    operator: EmptyOperator\n"),
        expected_state: RUN_STATUS_FAILED,
        expected_error: Some(&["twice", "already"]),
        expected_tasks: &[],
    },
    Row {
        // `TaskInstanceId` and `TaskId` both target the modelled `IdString`
        // (^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$, 1..255), so an identifier outside it is one
        // no instance of that task could be reported under.
        case: "a task identifier the model cannot carry",
        definition: Some("tasks:\n  - id: \"-leading-dash\"\n"),
        expected_state: RUN_STATUS_FAILED,
        expected_error: Some(&["-leading-dash"]),
        expected_tasks: &[],
    },
];

/// How long a run of the deepest graph in [`ROWS`] needs, with room to spare: the
/// admission span, the wait for an executor and one span per dependency level.
fn poll_budget() -> Duration {
    let levels = ROWS
        .iter()
        .map(|row| row.expected_tasks.len() + 1)
        .max()
        .unwrap_or(1);
    RUN_ADMITTED_AFTER
        + RUN_DEQUEUED_AFTER
        + TASK_LEVEL_RUNS_FOR * u32::try_from(levels).expect("a handful of levels")
        + Duration::from_secs(10)
}

/// Poll `GetWorkflowRun` the way a client waiting for completion does, and answer the
/// last report once the run has finished.
///
/// `pub(crate)` with [`read_task_instance`] below, so another suite that has to let a run
/// finish before reading its instances polls through the same harness rather than keeping
/// a second copy of it.
pub(crate) async fn run_to_completion(
    router: &smithy_server_runtime::MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    case: &str,
) -> Value {
    let request = json!({"WorkflowArn": workflow_arn, "RunId": run_id});
    let deadline = SystemTime::now() + poll_budget();
    let mut last = Value::Null;
    while SystemTime::now() < deadline {
        let (status, answered) = call(router, "GetWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {answered:?}"
        );
        let state = answered["RunDetail"]["RunState"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        last = answered;
        if state == RUN_STATUS_SUCCESS || state == RUN_STATUS_FAILED {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("case {case:?}: the run did not finish within {:?}, last report {last:?}", poll_budget())
}

/// `GetTaskInstance` for one instance of a run.
pub(crate) async fn read_task_instance(
    router: &smithy_server_runtime::MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    task_instance_id: &str,
    case: &str,
) -> Value {
    let (status, read) = call(
        router,
        "GetTaskInstance",
        json!({
            "WorkflowArn": workflow_arn,
            "RunId": run_id,
            "TaskInstanceId": task_instance_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a task instance the run executed must be readable, got {status} {read:?}"
    );
    read
}

/// Which instance of this run executes which DECLARED task: the `TaskInstanceId` the run
/// reports for each task, keyed by the `TaskId` that instance names.
///
/// A run's instance identifiers are the service's own, minted per run, so a client
/// discovers them from the run itself (`RunDetail.TaskInstances`) and learns which declared
/// task each one executes by READING the instance — `GetTaskInstance` is the only reader
/// that reports `TaskInstanceId` and `TaskId` together, because `TaskInstanceSummary`
/// carries no `TaskId`.
///
/// `pub(crate)` beside the two helpers above, so every suite that has to address a run's
/// instances resolves them the same way.
pub(crate) async fn instances_by_task(
    router: &smithy_server_runtime::MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, report) = call(
        router,
        "GetWorkflowRun",
        json!({"WorkflowArn": workflow_arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetWorkflowRun to succeed, got {status} {report:?}"
    );
    let mut by_task: BTreeMap<String, String> = BTreeMap::new();
    for id in reported_instance_ids(&report) {
        let read = read_task_instance(router, workflow_arn, run_id, &id, case).await;
        let task_id = read["TaskId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: instance {id:?} must name the task it executes, got {read:?}")
            })
            .to_string();
        let already = by_task.insert(task_id.clone(), id.clone());
        assert!(
            already.is_none(),
            "case {case:?}: task {task_id:?} runs once in this run, so one instance may name \
             it, got {already:?} and {id:?}"
        );
    }
    by_task
}

/// The `TaskInstanceId`s a `GetWorkflowRun` answer reports as `RunDetail.TaskInstances`,
/// in the order it reported them.
pub(crate) fn reported_instance_ids(report: &Value) -> Vec<String> {
    report["RunDetail"]["TaskInstances"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_run_executes_the_tasks_its_definition_declares_and_reports_their_outcome() {
    let router = definition_router();
    create_bucket(&router).await;

    // Every run is started FIRST and polled afterwards, so the rows' executions overlap
    // on the one clock they all run on rather than being waited for one after another.
    let mut started = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let key = format!("dags/outcome-{index}.yaml");
        if let Some(definition) = row.definition {
            stage(&router, &key, definition.as_bytes()).await;
        }
        let arn = create_workflow(&router, &format!("runOutcome{index}"), &key).await;
        let (status, run) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected StartWorkflowRun to succeed, got {status} {run:?}",
            row.case
        );
        let run_id = run["RunId"]
            .as_str()
            .expect("a started run has a RunId")
            .to_string();
        started.push((row, arn, run_id));
    }

    for (row, arn, run_id) in started {
        let case = row.case;
        let report = run_to_completion(&router, &arn, &run_id, case).await;
        let detail = &report["RunDetail"];

        assert_eq!(
            detail["RunState"].as_str(),
            Some(row.expected_state),
            "case {case:?}: expected RunState {:?}, got {:?}",
            row.expected_state,
            detail["RunState"]
        );
        match row.expected_error {
            None => assert!(
                detail["ErrorMessage"].is_null(),
                "case {case:?}: a run that completed successfully reports no ErrorMessage, \
                 got {:?}",
                detail["ErrorMessage"]
            ),
            Some(wanted) => {
                let reported = detail["ErrorMessage"].as_str().unwrap_or_default();
                for fragment in wanted {
                    assert!(
                        reported.contains(fragment),
                        "case {case:?}: the reason the run failed must name {fragment:?}, \
                         got {reported:?}"
                    );
                }
                assert!(
                    reported.chars().count() <= MAX_ERROR_MESSAGE_CHARS,
                    "case {case:?}: the reported reason must fit the modelled \
                     {MAX_ERROR_MESSAGE_CHARS} characters, got {} in {reported:?}",
                    reported.chars().count()
                );
            }
        }
        assert!(
            crate::workflow_lifecycle::is_date_time(&detail["CompletedOn"])
                && detail["Duration"].as_i64().is_some(),
            "case {case:?}: a finished run carries CompletedOn and Duration, got {:?} and {:?}",
            detail["CompletedOn"],
            detail["Duration"]
        );

        // The instances the run reports are one per task the definition declared, each
        // under an identifier the run minted for the instance rather than the name the
        // definition declared for the task.
        let instances = instances_by_task(&router, &arn, &run_id, case).await;
        let declared: BTreeSet<&str> = row.expected_tasks.iter().map(|task| task.task_id).collect();
        let executed: BTreeSet<&str> = instances.keys().map(String::as_str).collect();
        assert_eq!(
            executed, declared,
            "case {case:?}: expected the run to execute the declared tasks {declared:?}, got \
             {executed:?}"
        );
        let mut minted: Vec<&str> = instances.values().map(String::as_str).collect();
        minted.sort_unstable();
        let reported_ids: Vec<String> = reported_instance_ids(&report);
        assert_eq!(
            reported_ids, minted,
            "case {case:?}: expected the run to hold task instances {minted:?}, got {:?}",
            detail["TaskInstances"]
        );
        for id in &reported_ids {
            assert!(
                !declared.contains(id.as_str()),
                "case {case:?}: an instance identifier is minted for the instance, so it may \
                 not be a task name the definition declared ({declared:?}), got {id:?}"
            );
        }

        // Each instance, read back through the operation that owns it.
        let mut ended_at: Vec<(&str, f64)> = Vec::with_capacity(row.expected_tasks.len());
        let mut started_at: Vec<(&str, f64)> = Vec::with_capacity(row.expected_tasks.len());
        for task in row.expected_tasks {
            let instance_id = instances
                .get(task.task_id)
                .unwrap_or_else(|| panic!("case {case:?}: task {:?} ran in this run", task.task_id));
            let read = read_task_instance(&router, &arn, &run_id, instance_id, case).await;
            assert_eq!(
                read["TaskInstanceId"].as_str(),
                Some(instance_id.as_str()),
                "case {case:?}: the instance must report the identifier it is addressed by, \
                 got {:?}",
                read["TaskInstanceId"]
            );
            assert_eq!(
                read["Status"].as_str(),
                Some(TASK_SUCCESS),
                "case {case:?}: task {:?} ran to completion, got {:?}",
                task.task_id,
                read["Status"]
            );
            assert_eq!(
                read["TaskId"].as_str(),
                Some(task.task_id),
                "case {case:?}: the instance must name the task it executes, got {:?}",
                read["TaskId"]
            );
            assert_eq!(
                read["OperatorName"].as_str(),
                task.operator_name,
                "case {case:?}: task {:?} must report the operator its definition declared \
                 ({:?}), got {:?}",
                task.task_id,
                task.operator_name,
                read["OperatorName"]
            );
            assert_eq!(
                read["AttemptNumber"].as_i64(),
                Some(1),
                "case {case:?}: task {:?} was executed once, got {:?}",
                task.task_id,
                read["AttemptNumber"]
            );
            let start = epoch_seconds(&read["StartedAt"]).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: task {:?} that ran carries StartedAt, got {:?}",
                    task.task_id, read["StartedAt"]
                )
            });
            let end = epoch_seconds(&read["EndedAt"]).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: task {:?} that finished carries EndedAt, got {:?}",
                    task.task_id, read["EndedAt"]
                )
            });
            assert!(
                end >= start,
                "case {case:?}: task {:?} cannot end before it started, got {start} then {end}",
                task.task_id
            );
            assert!(
                read["DurationInSeconds"].as_i64().is_some(),
                "case {case:?}: task {:?} that completed carries DurationInSeconds, got {:?}",
                task.task_id,
                read["DurationInSeconds"]
            );
            started_at.push((task.task_id, start));
            ended_at.push((task.task_id, end));
        }

        // The ordering the graph requires: a task starts no earlier than the instant each
        // task it declared as a dependency ended.
        for task in row.expected_tasks {
            let start = started_at
                .iter()
                .find(|(id, _)| *id == task.task_id)
                .map(|(_, at)| *at)
                .expect("every expected task was read");
            for upstream in task.upstream {
                let end = ended_at
                    .iter()
                    .find(|(id, _)| id == upstream)
                    .map(|(_, at)| *at)
                    .expect("an upstream task of the same definition was read");
                assert!(
                    start + TIMESTAMP_TOLERANCE >= end,
                    "case {case:?}: task {:?} waits for {upstream:?}, so it may not start \
                     ({start}) before that task ended ({end})",
                    task.task_id
                );
            }
        }

        // A read changes nothing a later caller sees: the finished run answers the same.
        let (status, again) = call(
            &router,
            "GetWorkflowRun",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to succeed, got {status} {again:?}"
        );
        assert_eq!(
            again["RunDetail"], report["RunDetail"],
            "case {case:?}: a finished run must not move again, got {:?} after {:?}",
            again["RunDetail"], report["RunDetail"]
        );
    }
}

#[tokio::test]
async fn a_definition_declaring_more_tasks_than_the_emulator_runs_is_not_executed() {
    let router = definition_router();
    create_bucket(&router).await;

    // One task past the ceiling: the reader does not take the rest of them, so this is a
    // definition no run of it executes.
    let mut definition = String::from("tasks:\n");
    for index in 0..=MAX_DECLARED_TASKS {
        definition.push_str(&format!("  - id: task{index}\n"));
    }
    let key = "dags/too-many.yaml";
    stage(&router, key, definition.as_bytes()).await;
    let arn = create_workflow(&router, "runOutcomeCeiling", key).await;
    let (status, run) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {run:?}"
    );
    let run_id = run["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    let report = run_to_completion(&router, &arn, &run_id, "past the task ceiling").await;
    let detail = &report["RunDetail"];
    assert_eq!(
        detail["RunState"].as_str(),
        Some(RUN_STATUS_FAILED),
        "case past the task ceiling: a definition this emulator does not run cannot \
         complete successfully, got {:?}",
        detail["RunState"]
    );
    let reported = detail["ErrorMessage"].as_str().unwrap_or_default();
    assert!(
        reported.contains(&MAX_DECLARED_TASKS.to_string()),
        "case past the task ceiling: the reason must name the ceiling \
         ({MAX_DECLARED_TASKS}), got {reported:?}"
    );
    assert!(
        detail["TaskInstances"].is_null(),
        "case past the task ceiling: nothing of a definition this emulator does not run \
         is executed, so the run holds no task instance, got {:?}",
        detail["TaskInstances"]
    );
}

#[tokio::test]
async fn the_instances_a_run_executed_are_the_ones_its_listing_pages_over() {
    let router = definition_router();
    create_bucket(&router).await;

    // One definition whose tasks span two dependency levels, so the listing has more than
    // one instance to report and they are not all at one level.
    let key = "dags/listing.yaml";
    stage(
        &router,
        key,
        b"tasks:\n  - id: alpha\n    operator: EmptyOperator\n  - id: beta\n    \
          dependencies: [alpha]\n  - id: gamma\n    dependencies: [alpha]\n",
    )
    .await;
    let arn = create_workflow(&router, "runOutcomeListing", key).await;
    let (status, run) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {run:?}"
    );
    let run_id = run["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    let report = run_to_completion(&router, &arn, &run_id, "listing").await;
    assert_eq!(
        report["RunDetail"]["RunState"].as_str(),
        Some(RUN_STATUS_SUCCESS),
        "case listing: every declared task finished, so the run completed successfully, \
         got {:?}",
        report["RunDetail"]["RunState"]
    );

    // The identifiers the run minted for the three tasks it executed, in the order a
    // listing walks them.
    let instances = instances_by_task(&router, &arn, &run_id, "listing").await;
    let declared: Vec<&str> = instances.keys().map(String::as_str).collect();
    assert_eq!(
        declared,
        vec!["alpha", "beta", "gamma"],
        "case listing: the run must execute the tasks the definition declared, got \
         {declared:?}"
    );
    let mut expected_ids: Vec<&str> = instances.values().map(String::as_str).collect();
    expected_ids.sort_unstable();

    let (status, listed) = call(
        &router,
        "ListTaskInstances",
        json!({"WorkflowArn": arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case listing: expected ListTaskInstances to succeed, got {status} {listed:?}"
    );
    let summaries = listed["TaskInstances"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let listed_ids: Vec<&str> = summaries
        .iter()
        .filter_map(|summary| summary["TaskInstanceId"].as_str())
        .collect();
    assert_eq!(
        listed_ids, expected_ids,
        "case listing: the listing must report the instances the run executed, got {listed:?}"
    );
    for summary in &summaries {
        assert_eq!(
            summary["Status"].as_str(),
            Some(TASK_SUCCESS),
            "case listing: every task of this definition finished successfully, got {summary:?}"
        );
    }
    assert_eq!(
        listed_ids,
        reported_instance_ids(&report)
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
        "case listing: the listing and the run's own report must name the same instances"
    );
}

#[tokio::test]
async fn stopping_a_run_ends_the_tasks_it_was_executing() {
    let router = definition_router();
    create_bucket(&router).await;

    let key = "dags/stopped.yaml";
    stage(
        &router,
        key,
        b"tasks:\n  - id: first\n    operator: EmptyOperator\n  - id: second\n    \
          dependencies: [first]\n  - id: third\n    dependencies: [second]\n",
    )
    .await;
    let arn = create_workflow(&router, "runOutcomeStopped", key).await;
    let (status, run) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {run:?}"
    );
    let run_id = run["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    // Stop the run while its first tasks are still being executed: the shutdown
    // "terminates all running tasks", so a task it terminated reports neither a success
    // nor a status that is still on its way somewhere, and the run itself may not carry
    // on to a successful completion.
    tokio::time::sleep(RUN_ADMITTED_AFTER + RUN_DEQUEUED_AFTER + Duration::from_millis(50)).await;
    let (status, stopped) = call(
        &router,
        "StopWorkflowRun",
        json!({"WorkflowArn": arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case stop: expected StopWorkflowRun to succeed, got {status} {stopped:?}"
    );
    tokio::time::sleep(RUN_STOPPED_AFTER + Duration::from_millis(200)).await;

    let (status, listed) = call(
        &router,
        "ListTaskInstances",
        json!({"WorkflowArn": arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case stop: expected ListTaskInstances to succeed, got {status} {listed:?}"
    );
    let reported: Vec<&str> = listed["TaskInstances"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|summary| summary["Status"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !reported.is_empty(),
        "case stop: the run had started executing, so it holds the instances it started, \
         got {listed:?}"
    );
    assert!(
        reported.contains(&TASK_INSTANCE_STATUS_CANCELLED),
        "case stop: the stop terminates the tasks the run had not finished, got {reported:?}"
    );
    assert!(
        reported.iter().all(|status| *status != TASK_INSTANCE_STATUS_RUNNING
            && *status != TASK_INSTANCE_STATUS_SCHEDULED),
        "case stop: once the shutdown is complete no task of the run is still running or \
         waiting, got {reported:?}"
    );

    let (status, read) = call(
        &router,
        "GetWorkflowRun",
        json!({"WorkflowArn": arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case stop: expected GetWorkflowRun to succeed, got {status} {read:?}"
    );
    assert_eq!(
        read["RunDetail"]["RunState"].as_str(),
        Some(RUN_STATUS_STOPPED),
        "case stop: a run the caller stopped finishes in the modelled stopped state, got {:?}",
        read["RunDetail"]["RunState"]
    );
}

// === The OUTCOME each task declares, and the outcome the run therefore reports ===
//
// A task's declaration states the work it performs, and a declared command line is a
// shell program: a program the workspace's one bounded POSIX/bash session performs to a
// non-zero status is work that did not succeed. So such a task's instance ends `FAILED`
// with a reason, the tasks waiting on it are never executed (`UPSTREAM_FAILED`), and the
// run reports the modelled `FAILED` with a non-null `ErrorMessage` naming the task it
// failed on — "The error message if the workflow run failed. This value is null if the run
// completed successfully" (`smithy_query(query="shape", shape="WorkflowRunDetail")`).
//
// A declaration whose program that session does NOT perform states no outcome, so the task
// it declares is not failed on that ground: what an operator does to the resources it
// names is the worker's work, not this emulator's.

/// What a row's run must report for one task instance.
struct ExpectedOutcome {
    /// `TaskId`: the declared task the instance executes, which is not the identifier the
    /// instance itself is reported under ([`instances_by_task`]).
    task_id: &'static str,
    /// The `TaskInstanceStatus` the instance must report.
    status: &'static str,
}

/// One row of the outcome table: a definition, the state its run must settle in, the task
/// its `ErrorMessage` must name (for a run that fails), and the instances it must hold.
struct OutcomeRow {
    case: &'static str,
    definition: &'static str,
    expected_state: &'static str,
    names_in_error: Option<&'static str>,
    expected: &'static [ExpectedOutcome],
}

/// The table. Rows differ along the FORM the declaration states its outcome in (the
/// utility that answers failure, an explicit non-zero exit, a non-zero exit after a wait,
/// a command list whose last command decides, a question about a file that is not there),
/// the GRAPH position of the failing task (the only task, one of two at the same level,
/// upstream of a waiting task), and the two controls: a program that answers success, and
/// a command this session has no program for, which states no outcome at all.
const OUTCOME_ROWS: &[OutcomeRow] = &[
    OutcomeRow {
        case: "the only task, answering failure through the utility for it",
        definition: "tasks:\n  only_task:\n    operator: BashOperator\n    bash_command: false\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("only_task"),
        expected: &[ExpectedOutcome {
            task_id: "only_task",
            status: TASK_FAILED,
        }],
    },
    OutcomeRow {
        // The Apache Airflow YAML shape: the DAG's own id, its tasks one mapping in.
        case: "the only task, exiting non-zero explicitly",
        definition: "failing_dag:\n  schedule: null\n  tasks:\n    only_task:\n      operator: \
                     airflow.providers.standard.operators.bash.BashOperator\n      \
                     bash_command: \"exit 1\"\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("only_task"),
        expected: &[ExpectedOutcome {
            task_id: "only_task",
            status: TASK_FAILED,
        }],
    },
    OutcomeRow {
        case: "a non-zero status other than one, reached after a declared wait",
        definition: "tasks:\n  - id: late_failure\n    bash_command: \"sleep 0.2 && exit 3\"\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("late_failure"),
        expected: &[ExpectedOutcome {
            task_id: "late_failure",
            status: TASK_FAILED,
        }],
    },
    OutcomeRow {
        // The status of a program is the status of its LAST command, so a list whose
        // last command answers failure is a program that answered failure.
        case: "a command list whose last command answers failure",
        definition: "tasks:\n  - id: listed\n    bash_command: \"echo started; false\"\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("listed"),
        expected: &[ExpectedOutcome {
            task_id: "listed",
            status: TASK_FAILED,
        }],
    },
    OutcomeRow {
        // A question the session can answer about its own tree: the file is not there, so
        // the utility answers failure.
        case: "a question about a file the task's own tree does not hold",
        definition: "tasks:\n  - id: asks\n    bash_command: \"test -f absent.txt\"\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("asks"),
        expected: &[ExpectedOutcome {
            task_id: "asks",
            status: TASK_FAILED,
        }],
    },
    OutcomeRow {
        // One of two tasks at the SAME level: the level still ends, and the task beside
        // the failing one keeps the outcome its own program answered.
        case: "one of two tasks at the same level",
        definition: "tasks:\n  - id: fine\n    bash_command: \"echo ok\"\n  - id: broken\n    \
                     bash_command: \"exit 1\"\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("broken"),
        expected: &[
            ExpectedOutcome {
                task_id: "fine",
                status: TASK_SUCCESS,
            },
            ExpectedOutcome {
                task_id: "broken",
                status: TASK_FAILED,
            },
        ],
    },
    OutcomeRow {
        // A failing task upstream of two waiting ones: the work they waited for never
        // finished, so neither of them is executed at all.
        case: "a failing task upstream of the tasks that wait for it",
        definition: "tasks:\n  - id: extract\n    bash_command: \"exit 1\"\n  - id: transform\n    \
                     dependencies: [extract]\n    bash_command: \"echo ok\"\n  - id: load\n    \
                     dependencies: [transform]\n",
        expected_state: RUN_STATUS_FAILED,
        names_in_error: Some("extract"),
        expected: &[
            ExpectedOutcome {
                task_id: "extract",
                status: TASK_FAILED,
            },
            ExpectedOutcome {
                task_id: "transform",
                status: TASK_UPSTREAM_FAILED,
            },
            ExpectedOutcome {
                task_id: "load",
                status: TASK_UPSTREAM_FAILED,
            },
        ],
    },
    OutcomeRow {
        case: "a program that answers success",
        definition: "tasks:\n  - id: succeeds\n    bash_command: \"echo done\"\n",
        expected_state: RUN_STATUS_SUCCESS,
        names_in_error: None,
        expected: &[ExpectedOutcome {
            task_id: "succeeds",
            status: TASK_SUCCESS,
        }],
    },
    OutcomeRow {
        // What the operator DOES to the resources it names is the worker's work, which
        // this emulator does not perform: such a declaration states no outcome, so the
        // task is not failed on that ground.
        case: "a command this session has no program for, which states no outcome",
        definition: "tasks:\n  - id: operates\n    operator: BashOperator\n    \
                     bash_command: \"aws s3 cp s3://bucket/key ./local\"\n",
        expected_state: RUN_STATUS_SUCCESS,
        names_in_error: None,
        expected: &[ExpectedOutcome {
            task_id: "operates",
            status: TASK_SUCCESS,
        }],
    },
];

#[tokio::test]
async fn a_task_whose_declared_program_cannot_succeed_fails_the_run_it_belongs_to() {
    let router = definition_router();
    create_bucket(&router).await;

    // Every run is started FIRST and polled afterwards, so the rows' executions overlap
    // on the one clock they all run on rather than being waited for one after another.
    let mut started = Vec::with_capacity(OUTCOME_ROWS.len());
    for (index, row) in OUTCOME_ROWS.iter().enumerate() {
        let key = format!("dags/outcome-declared-{index}.yaml");
        stage(&router, &key, row.definition.as_bytes()).await;
        let arn = create_workflow(&router, &format!("declaredOutcome{index}"), &key).await;
        let (status, run) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected StartWorkflowRun to succeed, got {status} {run:?}",
            row.case
        );
        let run_id = run["RunId"]
            .as_str()
            .expect("a started run has a RunId")
            .to_string();
        started.push((row, arn, run_id));
    }

    for (row, arn, run_id) in started {
        let case = row.case;
        let report = run_to_completion(&router, &arn, &run_id, case).await;
        let detail = &report["RunDetail"];

        assert_eq!(
            detail["RunState"].as_str(),
            Some(row.expected_state),
            "case {case:?}: expected RunState {:?}, got {:?}",
            row.expected_state,
            detail["RunState"]
        );
        // A finished run carries its window whichever outcome it finished with.
        assert!(
            crate::workflow_lifecycle::is_date_time(&detail["CompletedOn"])
                && detail["Duration"].as_i64().is_some(),
            "case {case:?}: a finished run carries CompletedOn and Duration, got {:?} and {:?}",
            detail["CompletedOn"],
            detail["Duration"]
        );
        match row.names_in_error {
            None => assert!(
                detail["ErrorMessage"].is_null(),
                "case {case:?}: a run that completed successfully reports no ErrorMessage, \
                 got {:?}",
                detail["ErrorMessage"]
            ),
            Some(task_id) => {
                let reported = detail["ErrorMessage"].as_str().unwrap_or_default();
                assert!(
                    reported.contains(task_id),
                    "case {case:?}: the reason the run failed must name {task_id:?}, got \
                     {reported:?}"
                );
                assert!(
                    (1..=MAX_ERROR_MESSAGE_CHARS).contains(&reported.chars().count()),
                    "case {case:?}: the reported reason must fit the modelled \
                     1..{MAX_ERROR_MESSAGE_CHARS} characters, got {} in {reported:?}",
                    reported.chars().count()
                );
            }
        }

        // The instances the run reports are one per task the definition declared,
        // whatever each of them ended as, each under the identifier the run minted for it.
        let instances = instances_by_task(&router, &arn, &run_id, case).await;
        let declared: BTreeSet<&str> = row.expected.iter().map(|task| task.task_id).collect();
        let executed: BTreeSet<&str> = instances.keys().map(String::as_str).collect();
        assert_eq!(
            executed, declared,
            "case {case:?}: expected the run to execute the declared tasks {declared:?}, got \
             {executed:?}"
        );
        let mut minted: Vec<&str> = instances.values().map(String::as_str).collect();
        minted.sort_unstable();
        let reported_ids: Vec<String> = reported_instance_ids(&report);
        assert_eq!(
            reported_ids, minted,
            "case {case:?}: expected the run to hold task instances {minted:?}, got {:?}",
            detail["TaskInstances"]
        );

        for task in row.expected {
            let instance_id = instances
                .get(task.task_id)
                .unwrap_or_else(|| panic!("case {case:?}: task {:?} ran in this run", task.task_id));
            let read = read_task_instance(&router, &arn, &run_id, instance_id, case).await;
            assert_eq!(
                read["Status"].as_str(),
                Some(task.status),
                "case {case:?}: expected task {:?} to report {:?}, got {:?}",
                task.task_id,
                task.status,
                read["Status"]
            );
            // A task whose own work answered failure reports a reason; one that never ran
            // reports neither a reason nor a window, because no execution of it happened.
            match task.status {
                TASK_FAILED => {
                    assert!(
                        read["ErrorMessage"].as_str().is_some_and(|reason| {
                            !reason.is_empty() && reason.chars().count() <= MAX_ERROR_MESSAGE_CHARS
                        }),
                        "case {case:?}: a failed instance reports a reason, got {:?}",
                        read["ErrorMessage"]
                    );
                    assert!(
                        epoch_seconds(&read["StartedAt"]).is_some()
                            && epoch_seconds(&read["EndedAt"]).is_some(),
                        "case {case:?}: a task that ran and failed carries StartedAt and \
                         EndedAt, got {:?} and {:?}",
                        read["StartedAt"],
                        read["EndedAt"]
                    );
                }
                TASK_UPSTREAM_FAILED => {
                    assert!(
                        read["StartedAt"].is_null() && read["EndedAt"].is_null(),
                        "case {case:?}: a task that never ran carries neither StartedAt nor \
                         EndedAt, got {:?} and {:?}",
                        read["StartedAt"],
                        read["EndedAt"]
                    );
                }
                _ => assert!(
                    read["ErrorMessage"].is_null(),
                    "case {case:?}: an instance that succeeded reports no ErrorMessage, got {:?}",
                    read["ErrorMessage"]
                ),
            }
        }

        // No instance of a task whose program answered failure reports success, through
        // the listing as well as through the read above.
        let (status, listed) = call(
            &router,
            "ListTaskInstances",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTaskInstances to succeed, got {status} {listed:?}"
        );
        for task in row.expected.iter().filter(|task| task.status == TASK_FAILED) {
            let instance_id = instances
                .get(task.task_id)
                .unwrap_or_else(|| panic!("case {case:?}: task {:?} ran in this run", task.task_id));
            let listed_status = listed["TaskInstances"]
                .as_array()
                .and_then(|entries| {
                    entries.iter().find(|summary| {
                        summary["TaskInstanceId"].as_str() == Some(instance_id.as_str())
                    })
                })
                .and_then(|summary| summary["Status"].as_str())
                .unwrap_or_default();
            assert_eq!(
                listed_status, TASK_FAILED,
                "case {case:?}: the listing must report task {:?} as {TASK_FAILED}, got \
                 {listed_status:?}",
                task.task_id
            );
        }

        // A read changes nothing a later caller sees: the finished run answers the same.
        let (status, again) = call(
            &router,
            "GetWorkflowRun",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to succeed, got {status} {again:?}"
        );
        assert_eq!(
            again["RunDetail"], report["RunDetail"],
            "case {case:?}: a finished run must not move again, got {:?} after {:?}",
            again["RunDetail"], report["RunDetail"]
        );
    }
}
