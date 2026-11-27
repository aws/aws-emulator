// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH task instances a run holds when the workflow has more than
//! one version: the ones declared by the version THAT run executes.
//!
//! The rule under test: a run executes the task graph of the workflow version it was
//! started under — the one `StartWorkflowRun`'s `WorkflowVersion` named, else the latest
//! at that moment — and a version is immutable ("takes a snapshot of the definition at
//! creation time, so subsequent changes to the Amazon S3 object will not affect the
//! workflow unless you create a new version",
//! `smithy_query(query="shape", shape="GetWorkflowResponse")`, `WorkflowDefinition`), so
//! the instances a run holds are exactly the instances of the tasks THAT version's
//! definition declared: each one is discoverable through `RunDetail.TaskInstances`,
//! readable with 200 through `GetTaskInstance` reporting the version the run executes, and
//! a task only ANOTHER version declares resolves to nothing in this run.
//!
//! This is the version dimension of the records `run_execution_outcome` covers for a
//! single-version workflow, and of the reader agreement `run_task_instances` covers: a
//! client that updates a workflow while a run is in flight, or that starts a run on an
//! earlier version, still inspects the tasks that run actually executes.
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because a run's
//! tasks come from the definition its version snapshotted out of Amazon S3. The table
//! spans what the rule generalises over: how the update changes the declared tasks (adds
//! one, removes some, replaces every identifier, declares the first ones where there were
//! none), the DEPENDENCY SHAPE on either side (independent tasks, a chain), and the FORM
//! the definition is written in (block YAML, JSON — YAML 1.2 is a superset of JSON).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    LOOKUP_MISS_STATUS, RUN_STATUS_SUCCESS, TASK_INSTANCE_STATUS_SUCCESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::run_execution_outcome::{
    instances_by_task, read_task_instance, reported_instance_ids, run_to_completion,
};
use crate::run_task_instances::current_version;
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router, stage, BUCKET,
};
use crate::workflow_lifecycle::role_arn;

/// One row: the definition the workflow's FIRST version freezes, the one an update
/// freezes as a SECOND version, and the `TaskId`s each of them declares.
struct Row {
    case: &'static str,
    first: &'static str,
    first_tasks: &'static [&'static str],
    second: &'static str,
    second_tasks: &'static [&'static str],
}

/// The table. Rows differ along how the update changes the declared set, the shape of the
/// dependencies on either side and the form the definition is written in — never merely
/// along the workflow's name.
const ROWS: &[Row] = &[
    Row {
        case: "the update adds a task that waits for the one already declared",
        first: "tasks:\n  - id: extract\n    operator: BashOperator\n",
        first_tasks: &["extract"],
        second: "tasks:\n  - id: extract\n    operator: BashOperator\n  - id: transform\n    \
                 operator: PythonOperator\n    dependencies: [extract]\n",
        second_tasks: &["extract", "transform"],
    },
    Row {
        case: "the update removes all but the first task of a chain",
        first: "tasks:\n  - id: fetch\n  - id: clean\n    dependencies: [fetch]\n  - id: \
                publish\n    dependencies: [clean]\n",
        first_tasks: &["fetch", "clean", "publish"],
        second: "tasks:\n  - id: fetch\n",
        second_tasks: &["fetch"],
    },
    Row {
        case: "the update replaces every task identifier",
        first: "tasks:\n  - id: alpha\n  - id: beta\n",
        first_tasks: &["alpha", "beta"],
        second: "tasks:\n  - id: gamma\n    operator: EmptyOperator\n",
        second_tasks: &["gamma"],
    },
    Row {
        case: "the update declares the first tasks where the version before had none",
        first: "name: idleAtFirst\ntasks: []\n",
        first_tasks: &[],
        second: "tasks:\n  - id: load\n  - id: verify\n    dependencies: [load]\n",
        second_tasks: &["load", "verify"],
    },
    Row {
        case: "the update rewrites the same declarations in JSON form",
        first: "tasks:\n  - id: scan\n",
        first_tasks: &["scan"],
        second: "{\"tasks\": [{\"id\": \"scan\"}, {\"id\": \"report\", \"dependencies\": \
                 [\"scan\"]}]}",
        second_tasks: &["scan", "report"],
    },
];

/// The definition a run in flight executes, and the one an update replaces it with: the
/// identifiers are disjoint, so the instances the run holds say which version it executes.
const IN_FLIGHT_BEFORE: &str = "tasks:\n  - id: ingest\n    operator: BashOperator\n  - id: \
                                reduce\n    dependencies: [ingest]\n";
const IN_FLIGHT_BEFORE_TASKS: &[&str] = &["ingest", "reduce"];
const IN_FLIGHT_AFTER: &str = "tasks:\n  - id: rewritten\n";
const IN_FLIGHT_AFTER_TASKS: &[&str] = &["rewritten"];

/// Start a run of `workflow_arn`, naming `version` when the caller asks for one and
/// leaving the optional member out otherwise, and answer its `RunId`.
async fn start_run(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    version: Option<&str>,
    case: &str,
) -> String {
    let mut body = json!({"WorkflowArn": workflow_arn});
    if let Some(version) = version {
        body["WorkflowVersion"] = Value::String(version.to_string());
    }
    let (status, started) = call(router, "StartWorkflowRun", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    started["RunId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a started run has a RunId, got {started:?}"))
        .to_string()
}

/// Append a version to `workflow_arn` freezing whatever the definition object holds now,
/// and answer the `WorkflowVersion` the update reports.
async fn update_workflow(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    key: &str,
    case: &str,
) -> String {
    let (status, updated) = call(
        router,
        "UpdateWorkflow",
        json!({
            "WorkflowArn": workflow_arn,
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
            "RoleArn": role_arn(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
    );
    updated["WorkflowVersion"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: an update reports the version it created, got {updated:?}")
        })
        .to_string()
}

/// Assert `GetTaskInstance` resolves to NOTHING for `task_instance_id` in this run: a task
/// another version of the workflow declares is not one this run executed, and a task
/// IDENTIFIER the definition declares is not an instance identifier at all — a run mints
/// those itself.
async fn assert_no_such_instance(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    task_instance_id: &str,
    case: &str,
) {
    let (status, answered) = call(
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
        status, LOOKUP_MISS_STATUS,
        "case {case:?}/{task_instance_id}: a task this run's version never declared must be \
         HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
    );
    assert!(
        answered["__type"]
            .as_str()
            .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
        "case {case:?}/{task_instance_id}: expected a ResourceNotFoundException, got {:?}",
        answered["__type"]
    );
}

/// Let the run finish and assert it holds exactly the instances of `declared`, each one
/// reporting `version` as the workflow version it belongs to, and that a task only
/// `foreign` declares resolves to nothing in it.
async fn assert_run_executed(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    version: &str,
    declared: &[&str],
    foreign: &[&str],
    case: &str,
) {
    let report = run_to_completion(router, workflow_arn, run_id, case).await;
    assert_eq!(
        report["RunDetail"]["RunState"].as_str(),
        Some(RUN_STATUS_SUCCESS),
        "case {case:?}: a run of a buildable graph completes successfully, got {:?}",
        report["RunDetail"]["RunState"]
    );
    assert_eq!(
        report["WorkflowVersion"].as_str(),
        Some(version),
        "case {case:?}: expected the run to report the version it executes {version:?}, got {:?}",
        report["WorkflowVersion"]
    );

    // The instances a client DISCOVERS for the run are one per task its version declared,
    // each under an identifier the run minted and each naming the task it executes.
    let instances = instances_by_task(router, workflow_arn, run_id, case).await;
    let executed: BTreeSet<&str> = instances.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = declared.iter().copied().collect();
    assert_eq!(
        executed, expected,
        "case {case:?}: expected the run to hold the instances of {expected:?}, got \
         {executed:?}"
    );
    let discovered: BTreeSet<String> = reported_instance_ids(&report).into_iter().collect();
    let minted: BTreeSet<String> = instances.values().cloned().collect();
    assert_eq!(
        discovered, minted,
        "case {case:?}: expected the run to report the instance identifiers it minted \
         {minted:?}, got {discovered:?}"
    );

    // …and each one reads back as an instance of that task, in that version of the
    // workflow.
    for id in declared {
        let instance_id = instances
            .get(*id)
            .unwrap_or_else(|| panic!("case {case:?}/{id}: this run executed task {id:?}"));
        let read = read_task_instance(router, workflow_arn, run_id, instance_id, case).await;
        assert_eq!(
            read["TaskId"].as_str(),
            Some(*id),
            "case {case:?}/{id}: expected the instance to name its task, got {:?}",
            read["TaskId"]
        );
        assert_eq!(
            read["TaskInstanceId"].as_str(),
            Some(instance_id.as_str()),
            "case {case:?}/{id}: expected the instance to report the identifier it is \
             addressed by, got {:?}",
            read["TaskInstanceId"]
        );
        assert_eq!(
            read["WorkflowVersion"].as_str(),
            Some(version),
            "case {case:?}/{id}: expected the version the run executes {version:?}, got {:?}",
            read["WorkflowVersion"]
        );
        assert_eq!(
            read["Status"].as_str(),
            Some(TASK_INSTANCE_STATUS_SUCCESS),
            "case {case:?}/{id}: a task of a completed run has finished, got {:?}",
            read["Status"]
        );
        assert!(
            !read["DurationInSeconds"].is_null(),
            "case {case:?}/{id}: a completed task reports how long it took, got {:?}",
            read["DurationInSeconds"]
        );
    }

    for id in foreign {
        assert_no_such_instance(router, workflow_arn, run_id, id, case).await;
    }
}

/// The identifiers in `other` that `declared` does not hold: the tasks the OTHER version
/// of the workflow declares and this run therefore never executed.
fn foreign_tasks(declared: &[&str], other: &'static [&'static str]) -> Vec<&'static str> {
    other
        .iter()
        .copied()
        .filter(|id| !declared.contains(id))
        .collect()
}

#[tokio::test]
async fn a_run_holds_the_instances_of_the_tasks_its_own_version_declared() {
    let router = definition_router();
    create_bucket(&router).await;

    // Every run is started FIRST and polled afterwards, so the rows' executions overlap on
    // the one clock they all run on rather than being waited for one after another.
    let mut started = Vec::with_capacity(ROWS.len() * 2);
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let key = format!("dags/versioned-{index}.yaml");
        stage(&router, &key, row.first.as_bytes()).await;
        let arn = create_workflow(&router, &format!("runVersionGraph{index}"), &key).await;
        let first_version = current_version(&router, &arn, case).await;

        // The object is overwritten and an update freezes it as a SECOND version, so the
        // workflow now declares a different set of tasks from the one its first version
        // froze.
        stage(&router, &key, row.second.as_bytes()).await;
        let second_version = update_workflow(&router, &arn, &key, case).await;
        assert_ne!(
            first_version, second_version,
            "case {case:?}: the update must append a version of its own, got \
             {second_version:?} again"
        );

        // One run on the EARLIER version, named explicitly, and one on the latest, named
        // by omission.
        let earlier = start_run(&router, &arn, Some(&first_version), case).await;
        let latest = start_run(&router, &arn, None, case).await;
        started.push((
            format!("{case} / the earlier version"),
            arn.clone(),
            earlier,
            first_version,
            row.first_tasks,
            foreign_tasks(row.first_tasks, row.second_tasks),
        ));
        started.push((
            format!("{case} / the version the update appended"),
            arn,
            latest,
            second_version,
            row.second_tasks,
            foreign_tasks(row.second_tasks, row.first_tasks),
        ));
    }

    for (case, arn, run_id, version, declared, foreign) in started {
        assert_run_executed(
            &router, &arn, &run_id, &version, declared, &foreign, &case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_run_in_flight_keeps_the_task_graph_of_the_version_it_started_under() {
    let case = "a workflow updated while a run of it is in flight";
    let router = definition_router();
    create_bucket(&router).await;
    let key = "dags/in-flight.yaml";
    stage(&router, key, IN_FLIGHT_BEFORE.as_bytes()).await;
    let arn = create_workflow(&router, "runInFlightUpdate", key).await;
    let version = current_version(&router, &arn, case).await;

    let run_id = start_run(&router, &arn, None, case).await;

    // The definition is replaced, and an update freezes the replacement as a new version,
    // while the run started above has not finished: the run executes the graph ITS version
    // froze, so none of that reaches it.
    stage(&router, key, IN_FLIGHT_AFTER.as_bytes()).await;
    let appended = update_workflow(&router, &arn, key, case).await;
    assert_ne!(
        version, appended,
        "case {case:?}: the update must append a version of its own, got {appended:?} again"
    );

    assert_run_executed(
        &router,
        &arn,
        &run_id,
        &version,
        IN_FLIGHT_BEFORE_TASKS,
        IN_FLIGHT_AFTER_TASKS,
        case,
    )
    .await;

    // A run started AFTER the update executes the version the update appended, so the
    // identifier the earlier run never held is the one this one does.
    let next = start_run(&router, &arn, None, case).await;
    assert_run_executed(
        &router,
        &arn,
        &next,
        &appended,
        IN_FLIGHT_AFTER_TASKS,
        IN_FLIGHT_BEFORE_TASKS,
        case,
    )
    .await;
}
