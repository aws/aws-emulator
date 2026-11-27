// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READERS of a run's task instances agreeing with one another:
//! `GetTaskInstance`, `ListTaskInstances` and `GetWorkflowRun`'s `RunDetail.TaskInstances`.
//!
//! The rule under test: the three readers report ONE set of records — the instances of the
//! tasks the run's definition declared — so every identifier `RunDetail.TaskInstances`
//! names is one the listing pages over and one `GetTaskInstance` serves with 200, each
//! summary agrees member for member with the instance read on its own, and two reads of
//! the same settled instance answer identically, because a reporting operation only
//! materialises what the clock already owed. It holds for a run whichever way the run came
//! about: one a caller started, and one the workflow's own trigger fired.
//!
//! What a run EXECUTES, and the outcome each instance therefore reports, is the subject of
//! `run_execution_outcome`; this suite is about the three readers of those records never
//! disagreeing, which is what a client inspecting a run's tasks depends on. The second rule
//! it covers is WHAT those identifiers are: an instance identifier is the service's own,
//! minted per (run, task, attempt), and never the identifier the definition declared for
//! the task — the last test.
//!
//! Every test drives the real wire path through the same router the emulator boots, with
//! the Amazon S3 service mounted beside this one on ONE shared engine, because a run's
//! tasks come from the definition its version snapshotted out of Amazon S3.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    LOOKUP_MISS_STATUS, RUN_TYPE_SCHEDULED, SCHEDULED_RUN_EVERY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::run_execution_outcome::{
    instances_by_task, read_task_instance, reported_instance_ids, run_to_completion,
};
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router, stage,
};

/// The definition the started-run test stages: three tasks, one of which waits for
/// another, so the run holds instances at more than one dependency level and the readers
/// have a set to agree on rather than a single record. It declares no schedule, so only a
/// caller starts a run of it.
const DEFINITION: &str = "tasks:\n  - id: extract\n    operator: BashOperator\n  \
                          - id: transform\n    operator: PythonOperator\n    \
                          dependencies: [extract]\n  - id: publish\n";

/// The same three tasks, under a definition that DECLARES A SCHEDULE: "when a workflow
/// includes schedule information in its YAML definition, the service automatically
/// configures the appropriate triggers for automated execution"
/// (`smithy_query(query="shape", shape="GetWorkflowResponse")`, `ScheduleConfiguration`),
/// so the service fires runs of it with no caller asking.
const SCHEDULED_DEFINITION: &str = "schedule: \"@daily\"\ntasks:\n  - id: extract\n    \
                                    operator: BashOperator\n  - id: transform\n    \
                                    operator: PythonOperator\n    dependencies: \
                                    [extract]\n  - id: publish\n";

/// The `TaskId`s [`DEFINITION`] declares.
const DECLARED_TASKS: &[&str] = &["extract", "transform", "publish"];

/// Every `TaskInstanceSummary` `ListTaskInstances` reports for a run, keyed by the
/// identifier it names, walked through the cursors the listing issues so a run holding
/// more than one page is read whole.
async fn listed_instances(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    case: &str,
) -> BTreeMap<String, Value> {
    let mut summaries: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({"WorkflowArn": workflow_arn, "RunId": run_id, "MaxResults": 100});
        if let Some(cursor) = &token {
            body["NextToken"] = Value::String(cursor.clone());
        }
        let (status, page) = call(router, "ListTaskInstances", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTaskInstances to succeed, got {status} {page:?}"
        );
        let items = page["TaskInstances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TaskInstances list, got {page:?}"))
            .clone();
        for item in items {
            let id = item["TaskInstanceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary names its instance, got {item:?}")
                })
                .to_string();
            assert!(
                summaries.insert(id.clone(), item).is_none(),
                "case {case:?}: the listing must not report instance {id:?} twice"
            );
        }
        match page["NextToken"].as_str() {
            Some(cursor) => token = Some(cursor.to_string()),
            None => break,
        }
    }
    summaries
}

/// Assert the three readers of `run_id`'s task instances report ONE set of records, and
/// that reading one twice answers identically.
async fn assert_readers_agree(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    workflow_version: &str,
    case: &str,
) {
    let finished = run_to_completion(router, workflow_arn, run_id, case).await;
    let detailed: BTreeSet<String> = finished["RunDetail"]["TaskInstances"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        detailed.len(),
        DECLARED_TASKS.len(),
        "case {case:?}: the run must name an instance per declared task, got {detailed:?}"
    );

    let summaries = listed_instances(router, workflow_arn, run_id, case).await;
    let listed: BTreeSet<String> = summaries.keys().cloned().collect();
    assert_eq!(
        listed, detailed,
        "case {case:?}: the listing and GetWorkflowRun must name the same instances, got \
         {listed:?} and {detailed:?}"
    );

    let mut tasks: BTreeSet<String> = BTreeSet::new();
    for (id, summary) in &summaries {
        let read = read_task_instance(router, workflow_arn, run_id, id, case).await;

        // The three members the OWNING RUN decides, read from the run by both readers
        // rather than echoed from the request.
        for (member, expected) in [
            ("WorkflowArn", workflow_arn),
            ("RunId", run_id),
            ("WorkflowVersion", workflow_version),
        ] {
            assert_eq!(
                read[member].as_str(),
                Some(expected),
                "case {case:?}/{id}: expected {member} {expected:?}, got {:?}",
                read[member]
            );
            assert_eq!(
                summary[member], read[member],
                "case {case:?}/{id}: the summary and the instance must agree on {member}, \
                 got {:?} and {:?}",
                summary[member], read[member]
            );
        }

        // Every member the SUMMARY carries is the one the instance itself reports, so a
        // client that pages and a client that reads see one record. `Status` and the
        // derived `DurationInSeconds` are asserted PRESENT first, so the agreement below
        // cannot be satisfied by two absent members.
        for member in ["Status", "DurationInSeconds"] {
            assert!(
                !read[member].is_null(),
                "case {case:?}/{id}: a settled instance reports {member}, got {:?}",
                read[member]
            );
        }
        for member in [
            "TaskInstanceId",
            "Status",
            "OperatorName",
            "DurationInSeconds",
        ] {
            assert_eq!(
                summary[member], read[member],
                "case {case:?}/{id}: the summary and the instance must agree on {member}, \
                 got {:?} and {:?}",
                summary[member], read[member]
            );
        }

        let task_id = read["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}/{id}: an instance names its task, got {read:?}"))
            .to_string();
        assert!(
            DECLARED_TASKS.contains(&task_id.as_str()),
            "case {case:?}/{id}: expected an instance of a declared task {DECLARED_TASKS:?}, \
             got {task_id:?}"
        );
        assert!(
            tasks.insert(task_id.clone()),
            "case {case:?}: the run must hold ONE instance per task, got two for {task_id:?}"
        );

        // A reporting operation changes nothing a later caller sees: the second read of a
        // settled instance answers exactly the same.
        let again = read_task_instance(router, workflow_arn, run_id, id, case).await;
        assert_eq!(
            again, read,
            "case {case:?}/{id}: a settled instance must not move between reads, got \
             {again:?} after {read:?}"
        );
    }
    let declared: BTreeSet<String> = DECLARED_TASKS.iter().map(|id| (*id).to_string()).collect();
    assert_eq!(
        tasks, declared,
        "case {case:?}: the instances must be of exactly the declared tasks, got {tasks:?}"
    );
}

/// The `WorkflowVersion` a workflow currently reports.
///
/// `pub(crate)` so another suite whose rule is about WHICH version a run executes reads
/// it through the same call rather than keeping a second copy of it.
pub(crate) async fn current_version(router: &MultiServiceRouter, arn: &str, case: &str) -> String {
    let (status, fetched) = call(router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
    );
    fetched["WorkflowVersion"]
        .as_str()
        .expect("a stored workflow reports its version")
        .to_string()
}

#[tokio::test]
async fn every_reader_of_a_started_run_s_task_instances_reports_the_same_records() {
    let case = "a run a caller started";
    let router = definition_router();
    create_bucket(&router).await;
    let key = "dags/readers.yaml";
    stage(&router, key, DEFINITION.as_bytes()).await;
    let arn = create_workflow(&router, "taskInstanceReaders", key).await;
    let version = current_version(&router, &arn, case).await;

    let (status, started) = call(&router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    assert_readers_agree(&router, &arn, &run_id, &version, case).await;
}

#[tokio::test]
async fn every_reader_of_a_triggered_run_s_task_instances_reports_the_same_records() {
    let case = "a run the workflow's own trigger fired";
    let router = definition_router();
    create_bucket(&router).await;
    let key = "dags/triggered.yaml";
    stage(&router, key, SCHEDULED_DEFINITION.as_bytes()).await;

    // A workflow whose DEFINITION declares a schedule, which is what the service
    // configures a trigger from, so the run under test is one no caller started.
    let arn = create_workflow(&router, "triggeredTaskInstances", key).await;
    let version = current_version(&router, &arn, case).await;

    // Wait for the firing the trigger owes, which is what records a run of this kind.
    let deadline = SystemTime::now() + SCHEDULED_RUN_EVERY + Duration::from_secs(10);
    let mut run_id = None;
    while SystemTime::now() < deadline {
        let (status, listed) = call(&router, "ListWorkflowRuns", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
        );
        run_id = listed["WorkflowRuns"]
            .as_array()
            .and_then(|runs| {
                runs.iter()
                    .find(|run| run["RunType"].as_str() == Some(RUN_TYPE_SCHEDULED))
                    .and_then(|run| run["RunId"].as_str().map(str::to_string))
            });
        if run_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let run_id = run_id.unwrap_or_else(|| {
        panic!("case {case:?}: the trigger must have fired a run within {SCHEDULED_RUN_EVERY:?}")
    });

    assert_readers_agree(&router, &arn, &run_id, &version, case).await;
}

// === WHAT a task-instance identifier IS ===
//
// `TaskInstanceId` is "The unique identifier of this task instance" while the task the
// instance executes is reported separately as `TaskId`, "The unique identifier of the task
// definition within the workflow" (`smithy_query(query="shape",
// shape="TaskInstanceSummary,GetTaskInstanceResponse")`), so the identifier of an instance
// is the service's own, minted when the run creates the instance, and not a name the
// caller's definition authored: two runs of one workflow execute the same declared tasks
// and must still report instance identifiers that tell their instances apart.

/// The published bounds of the modelled `IdString` every reader of a task-instance
/// identifier targets. Source: `smithy_query(query="shape", shape="IdString")` —
/// `@length(1..255)` and `@pattern ^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`.
const MIN_ID_STRING_CHARS: usize = 1;
const MAX_ID_STRING_CHARS: usize = 255;

/// Whether `value` is one that modelled `IdString` can carry.
fn is_id_string(value: &str) -> bool {
    let length = value.chars().count();
    (MIN_ID_STRING_CHARS..=MAX_ID_STRING_CHARS).contains(&length)
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
}

/// One row: a definition, and the `TaskId`s it declares.
struct MintedIdRow {
    case: String,
    definition: String,
    declared: Vec<String>,
}

/// The table. Rows differ along how many tasks the definition declares, the shape of the
/// dependencies between them and the FORM of the identifiers it declares for them — never
/// merely along the workflow's name.
fn minted_id_rows() -> Vec<MintedIdRow> {
    let row = |case: &str, definition: String, declared: &[&str]| MintedIdRow {
        case: case.to_string(),
        definition,
        declared: declared.iter().map(|id| (*id).to_string()).collect(),
    };
    // The longest identifier the modelled member can carry, so a definition at that bound
    // is covered too.
    let longest = format!("t{}", "x".repeat(MAX_ID_STRING_CHARS - 1));
    vec![
        row(
            "one task",
            "tasks:\n  - id: solo\n    operator: EmptyOperator\n".to_string(),
            &["solo"],
        ),
        row(
            "two tasks, the second waiting for the first",
            "tasks:\n  - id: first_task\n  - id: second_task\n    dependencies: \
             [first_task]\n"
                .to_string(),
            &["first_task", "second_task"],
        ),
        row(
            "three tasks fanning out from one",
            "tasks:\n  - id: root\n  - id: left\n    dependencies: [root]\n  - id: right\n    \
             dependencies: [root]\n"
                .to_string(),
            &["root", "left", "right"],
        ),
        row(
            "task identifiers carrying each punctuation character the member admits",
            "tasks:\n  - id: a.b\n  - id: c-d\n    dependencies: [a.b]\n  - id: e_f\n    \
             dependencies: [c-d]\n"
                .to_string(),
            &["a.b", "c-d", "e_f"],
        ),
        row(
            "a task identifier at the longest the member can carry",
            format!("tasks:\n  - id: {longest}\n"),
            &[longest.as_str()],
        ),
    ]
}

/// Assert `GetTaskInstance` resolves to NOTHING for `task_instance_id` in this run.
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
        "case {case:?}: {task_instance_id:?} names no instance of this run, so the read must \
         be HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
    );
    assert!(
        answered["__type"]
            .as_str()
            .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
        "case {case:?}: expected a ResourceNotFoundException for {task_instance_id:?}, got {:?}",
        answered["__type"]
    );
}

/// Start a run of `workflow_arn` and answer its `RunId`.
async fn start_run(router: &MultiServiceRouter, workflow_arn: &str, case: &str) -> String {
    let (status, started) = call(
        router,
        "StartWorkflowRun",
        json!({"WorkflowArn": workflow_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    started["RunId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a started run has a RunId, got {started:?}"))
        .to_string()
}

#[tokio::test]
async fn a_run_mints_an_instance_identifier_per_task_instead_of_reporting_the_declared_task_id() {
    let router = definition_router();
    create_bucket(&router).await;
    let rows = minted_id_rows();

    // Every run is started FIRST and polled afterwards, so the rows' executions overlap on
    // the one clock they all run on rather than being waited for one after another. TWO
    // runs per row, of ONE workflow: the same definition declares the same tasks for both,
    // which is the case an instance identifier has to tell apart.
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let case = row.case.as_str();
        let key = format!("dags/minted-instance-ids-{index}.yaml");
        stage(&router, &key, row.definition.as_bytes()).await;
        let arn = create_workflow(&router, &format!("mintedInstanceIds{index}"), &key).await;
        let first = start_run(&router, &arn, case).await;
        let second = start_run(&router, &arn, case).await;
        started.push((row, arn, first, second));
    }

    for (row, arn, first, second) in started {
        let declared: BTreeSet<&str> = row.declared.iter().map(String::as_str).collect();
        let mut minted_per_run: Vec<BTreeSet<String>> = Vec::with_capacity(2);
        for (leg, run_id) in [("first run", &first), ("second run", &second)] {
            let case = format!("{} / the {leg}", row.case);
            let case = case.as_str();
            let report = run_to_completion(&router, &arn, run_id, case).await;

            // One instance per declared task, each naming the task it executes.
            let instances = instances_by_task(&router, &arn, run_id, case).await;
            let executed: BTreeSet<&str> = instances.keys().map(String::as_str).collect();
            assert_eq!(
                executed, declared,
                "case {case:?}: expected an instance of each declared task {declared:?}, got \
                 {executed:?}"
            );

            // Both readers of those instances report the identifiers the run minted, and
            // nothing else.
            let minted: BTreeSet<String> = instances.values().cloned().collect();
            let reported: BTreeSet<String> = reported_instance_ids(&report).into_iter().collect();
            assert_eq!(
                reported, minted,
                "case {case:?}: expected RunDetail.TaskInstances to name the minted \
                 identifiers {minted:?}, got {reported:?}"
            );
            let listed: BTreeSet<String> = listed_instances(&router, &arn, run_id, case)
                .await
                .into_keys()
                .collect();
            assert_eq!(
                listed, minted,
                "case {case:?}: expected the listing to report the minted identifiers \
                 {minted:?}, got {listed:?}"
            );

            for (task_id, instance_id) in &instances {
                assert!(
                    is_id_string(instance_id),
                    "case {case:?}: a minted identifier must be one the modelled IdString \
                     carries, got {instance_id:?}"
                );
                assert_ne!(
                    instance_id, task_id,
                    "case {case:?}: the identifier of an instance is not the identifier of \
                     the task it executes, got {instance_id:?} for both"
                );
                assert!(
                    !declared.contains(instance_id.as_str()),
                    "case {case:?}: a minted identifier may not be any name the definition \
                     declared ({declared:?}), got {instance_id:?}"
                );
                // The declared task name is not a handle to the instance: only the minted
                // identifier addresses it.
                assert_no_such_instance(&router, &arn, run_id, task_id, case).await;
                let read = read_task_instance(&router, &arn, run_id, instance_id, case).await;
                assert_eq!(
                    read["TaskInstanceId"].as_str(),
                    Some(instance_id.as_str()),
                    "case {case:?}: expected the instance to report the identifier it is \
                     addressed by, got {:?}",
                    read["TaskInstanceId"]
                );
                assert_eq!(
                    read["TaskId"].as_str(),
                    Some(task_id.as_str()),
                    "case {case:?}: expected the instance to name the declared task \
                     {task_id:?}, got {:?}",
                    read["TaskId"]
                );
            }

            // The identifiers are the run's own and do not move between reads.
            let again: BTreeSet<String> =
                reported_instance_ids(&run_to_completion(&router, &arn, run_id, case).await)
                    .into_iter()
                    .collect();
            assert_eq!(
                again, minted,
                "case {case:?}: a settled run must report the same instance identifiers on \
                 every read, got {again:?} after {minted:?}"
            );
            minted_per_run.push(minted);
        }

        // Two runs of ONE definition: the same tasks, and no instance identifier in common,
        // so a client can tell this run's instance of a task from the other run's.
        let shared: Vec<&String> = minted_per_run[0].intersection(&minted_per_run[1]).collect();
        assert!(
            shared.is_empty(),
            "case {:?}: two runs of one workflow mint their own instance identifiers, got \
             {shared:?} in both",
            row.case
        );

        // …and an identifier is a handle on an instance of ITS OWN run: the other run of
        // the same workflow resolves none of them.
        for (reader, minted_elsewhere) in [
            (&second, &minted_per_run[0]),
            (&first, &minted_per_run[1]),
        ] {
            for id in minted_elsewhere {
                assert_no_such_instance(&router, &arn, reader, id, &row.case).await;
            }
        }
    }
}
