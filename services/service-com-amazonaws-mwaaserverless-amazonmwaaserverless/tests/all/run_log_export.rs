// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for where a workflow run's own execution output GOES: the CloudWatch
//! Logs log group the workflow's `LoggingConfiguration` names.
//!
//! The RULE every row asserts: a workflow that configures a log group has what its runs
//! produce exported to that group — "Logs are automatically exported to your specified
//! CloudWatch log group using remote logging functionality" over output that "capture[s]
//! workflow execution details, task-level information, and system events"
//! (`smithy_query(query="doc", shape="LoggingConfiguration")`), stored under the group
//! `LogGroupName` names — so CloudWatch Logs' OWN readers find the group, a stream per unit
//! of the execution (the run, and each task-instance attempt the run ran) and the events
//! those units produced; and each task instance that executed reports the stream it wrote
//! to as `LogStream` ("The CloudWatch log stream name for this task instance execution"),
//! which is a stream that group really holds. A workflow whose create named NO group logs
//! to the group the SERVICE names for it (`state::service_log_group_name`), which is the
//! same rule reading the destination out of the same snapshot member.
//!
//! CloudWatch Logs is mounted beside this service on ONE engine and asked through its own
//! `DescribeLogGroups`, `DescribeLogStreams` and `GetLogEvents` — the shape
//! `core/router/src/lib.rs` mounts every service in — because the groups, streams and
//! events are that service's, and a reported stream name that names nothing would be a
//! resource no reader can reach.
//!
//! The rows differ along what the rule generalises over: how many tasks the definition
//! declares and in which dependency SHAPE (none, one, a chain, a fan-out and join), whether
//! the run SUCCEEDS or FAILS, the FORM of the configured log group name (a path, a plain
//! name, the punctuation CloudWatch Logs admits), and whether a group is configured at all.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    service_log_group_name, RUN_STATUS_FAILED, RUN_STATUS_SUCCESS,
};

use crate::run_execution_outcome::{instances_by_task, read_task_instance, run_to_completion};
use crate::workflow_definition_snapshot::{call, create_bucket, stage, BUCKET, REGION};
use crate::workflow_lifecycle::role_arn;

/// The `X-Amz-Target` prefix of the CloudWatch Logs service (awsJson1.1).
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The attempt every instance this service's execution records is: a run executes a
/// declared task once, so the stream an instance writes to names its first attempt.
const FIRST_ATTEMPT: i32 = 1;

/// One row: the definition a workflow is created with, the log group it configures, and
/// the execution whose output must reach that group.
struct Row {
    case: &'static str,
    /// The definition content staged in Amazon S3.
    definition: &'static str,
    /// The `LoggingConfiguration.LogGroupName` the workflow is created with, and `None`
    /// for a workflow that configures none — whose runs go to the group the service names
    /// for it instead.
    log_group: Option<&'static str>,
    /// The `RunState` the run must finish in.
    expected_state: &'static str,
    /// The task instances the run executes — each of which writes to its own stream.
    expected_tasks: &'static [&'static str],
}

/// The table. The log group names are written out here, and every expected stream name is
/// composed here from the run and the task rather than read from this crate's own helper,
/// so a stream a client could not find, or a name the modelled `LogStream` could not
/// carry, fails these rows.
const ROWS: &[Row] = &[
    Row {
        case: "a definition declaring no task, exported to a path-style group",
        definition: "name: nothingToLog\ntasks: []\n",
        log_group: Some("/aws/mwaa-serverless/no-tasks"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_tasks: &[],
    },
    Row {
        case: "one task, exported to a plain group name",
        definition: "tasks:\n  - id: extract\n    operator: EmptyOperator\n",
        log_group: Some("mwaa-serverless-one-task"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_tasks: &["extract"],
    },
    Row {
        case: "a chain of three tasks, exported to a vended-logs path",
        definition: "tasks:\n  - id: extract\n  - id: transform\n    dependencies: [extract]\n  \
                     - id: load\n    depends_on: transform\n",
        log_group: Some("/aws/vendedlogs/mwaa-serverless/chain"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_tasks: &["extract", "transform", "load"],
    },
    Row {
        case: "a graph no run can execute, exported to a punctuated group name",
        definition: "dag:\n  dag_id: diamond\ntasks:\n  - id: start\n    operator: \
                     EmptyOperator\n  - id: left\n    operator: PythonOperator\n    \
                     dependencies: [start]\n  - id: right\n    operator: BashOperator\n    \
                     dependencies: [right_upstream]\n",
        log_group: Some("mwaa.serverless_logs-4/run#4"),
        expected_state: RUN_STATUS_FAILED,
        expected_tasks: &[],
    },
    Row {
        case: "a fan-out and join over four tasks, sharing that group with the row above",
        definition: "dag:\n  dag_id: diamond\ntasks:\n  - id: start\n    operator: \
                     EmptyOperator\n  - id: left\n    operator: PythonOperator\n    \
                     dependencies: [start]\n  - id: right\n    operator: BashOperator\n    \
                     dependencies: [start]\n  - id: join\n    dependencies: [left, right]\n",
        log_group: Some("mwaa.serverless_logs-4/run#4"),
        expected_state: RUN_STATUS_SUCCESS,
        expected_tasks: &["start", "left", "right", "join"],
    },
    Row {
        case: "a workflow that names no group, so the service names it",
        definition: "tasks:\n  - id: extract\n    operator: EmptyOperator\n",
        log_group: None,
        expected_state: RUN_STATUS_SUCCESS,
        expected_tasks: &["extract"],
    },
];

/// This service, Amazon S3 (a definition is an object a version snapshots) and CloudWatch
/// Logs (the destination of a run's output) on ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mwaaserverless_amazonmwaaserverless::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// One CloudWatch Logs call over its awsJson1.1 surface, in the Region every request of
/// these rows is signed for.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{REGION}/logs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Every log group CloudWatch Logs holds, by name.
async fn log_group_names(router: &MultiServiceRouter) -> BTreeSet<String> {
    let (status, described) = logs(router, "DescribeLogGroups", json!({})).await;
    assert_eq!(
        status, 200,
        "DescribeLogGroups must succeed, got {status} {described:?}"
    );
    described["logGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The streams of `log_group`, by name.
async fn log_stream_names(router: &MultiServiceRouter, log_group: &str) -> BTreeSet<String> {
    let (status, described) = logs(
        router,
        "DescribeLogStreams",
        json!({"logGroupName": log_group}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeLogStreams({log_group:?}) must succeed, got {status} {described:?}"
    );
    described["logStreams"]
        .as_array()
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The messages of `log_stream`, oldest first, as a client reading the stream sees them.
async fn log_messages(
    router: &MultiServiceRouter,
    log_group: &str,
    log_stream: &str,
) -> Vec<String> {
    let (status, read) = logs(
        router,
        "GetLogEvents",
        json!({
            "logGroupName": log_group,
            "logStreamName": log_stream,
            "startFromHead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetLogEvents({log_group:?}, {log_stream:?}) must succeed, got {status} {read:?}"
    );
    read["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Create a workflow whose definition is the object at `key`, carrying `log_group` as its
/// `LoggingConfiguration` when one is named, and answer its ARN.
async fn create_workflow(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    log_group: Option<&str>,
) -> String {
    let mut body = json!({
        "Name": name,
        "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
        "RoleArn": role_arn(),
    });
    if let Some(log_group) = log_group {
        body["LoggingConfiguration"] = json!({"LogGroupName": log_group});
    }
    let (status, created) = call(router, "CreateWorkflow", body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the workflow must be created, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string()
}

/// Start a run of `arn` and answer its `RunId`.
async fn start_run(router: &MultiServiceRouter, arn: &str, case: &str) -> String {
    let (status, run) = call(router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {run:?}"
    );
    run["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string()
}

/// The stream one task instance's attempt writes to, composed the way a client reads it
/// back: the run, the instance and the attempt.
fn task_stream(run_id: &str, task_instance_id: &str) -> String {
    format!("{run_id}.{task_instance_id}.attempt-{FIRST_ATTEMPT}")
}

#[tokio::test]
async fn a_runs_output_reaches_the_configured_log_group_and_its_task_instances_name_it() {
    let router = emulator();
    create_bucket(&router).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let key = format!("dags/run-log-export-{index}.yaml");
        stage(&router, &key, row.definition.as_bytes()).await;
        let arn = create_workflow(&router, &format!("runLogExport{index}"), &key, row.log_group)
            .await;
        let run_id = start_run(&router, &arn, case).await;
        // Where this run's output goes: the group the create named, or — for a create that
        // named none — the group the SERVICE names for the workflow, derived here from the
        // workflow's own ARN rather than written out.
        let log_group = row
            .log_group
            .map(str::to_string)
            .unwrap_or_else(|| service_log_group_name(&arn));
        let log_group = log_group.as_str();

        // That group is the destination of the run from the moment it is admitted: the
        // group exists and the run's own stream already holds the line saying so, before
        // anything was read back.
        let groups = log_group_names(&router).await;
        assert!(
            groups.contains(log_group),
            "case {case:?}: the log group {log_group:?} must exist once the run is started, \
             got {groups:?}"
        );
        let admitted = log_messages(&router, log_group, &run_id).await;
        assert!(
            admitted.iter().any(|message| message.contains(&run_id)),
            "case {case:?}: the run's own stream must carry its admission, got {admitted:?}"
        );

        let report = run_to_completion(&router, &arn, &run_id, case).await;
        assert_eq!(
            report["RunDetail"]["RunState"].as_str(),
            Some(row.expected_state),
            "case {case:?}: expected RunState {:?}, got {:?}",
            row.expected_state,
            report["RunDetail"]["RunState"]
        );

        // Every instance the run executed names the stream it wrote to, under the
        // identifier the run minted for it.
        let instances = instances_by_task(&router, &arn, &run_id, case).await;
        for task in row.expected_tasks {
            let instance_id = instances
                .get(*task)
                .unwrap_or_else(|| panic!("case {case:?}: the run executed task {task:?}"));
            let read = read_task_instance(&router, &arn, &run_id, instance_id, case).await;
            assert_eq!(
                read["LogStream"].as_str(),
                Some(task_stream(&run_id, instance_id).as_str()),
                "case {case:?}: expected task {task:?} to name the stream its attempt \
                 wrote to, got {:?}",
                read["LogStream"]
            );
        }

        // The streams the group holds for this run are exactly the run's own and one per
        // task instance it executed.
        let mut expected_streams: BTreeSet<String> = instances
            .values()
            .map(|instance_id| task_stream(&run_id, instance_id))
            .collect();
        expected_streams.insert(run_id.clone());
        let held: BTreeSet<String> = log_stream_names(&router, log_group)
            .await
            .into_iter()
            .filter(|stream| stream.starts_with(&run_id))
            .collect();
        assert_eq!(
            held, expected_streams,
            "case {case:?}: expected the group to hold the streams {expected_streams:?}, got \
             {held:?}"
        );

        // The run's own stream carries the outcome it finished in.
        let run_lines = log_messages(&router, log_group, &run_id).await;
        assert!(
            run_lines
                .iter()
                .any(|message| message.contains(row.expected_state)),
            "case {case:?}: the run's stream must report the status it finished in \
             ({:?}), got {run_lines:?}",
            row.expected_state
        );

        // Each task's stream carries that task's own execution: the attempt starting and
        // the outcome it ended with.
        for task in row.expected_tasks {
            let instance_id = instances
                .get(*task)
                .unwrap_or_else(|| panic!("case {case:?}: the run executed task {task:?}"));
            let stream = task_stream(&run_id, instance_id);
            let lines = log_messages(&router, log_group, &stream).await;
            assert!(
                lines.len() >= 2,
                "case {case:?}: the stream {stream:?} must carry the attempt starting and \
                 ending, got {lines:?}"
            );
            assert!(
                lines.iter().all(|message| message.contains(task)),
                "case {case:?}: every line of {stream:?} names the task {task:?}, got {lines:?}"
            );
            assert!(
                lines.last().is_some_and(|message| message.contains("SUCCESS")),
                "case {case:?}: the last line of {stream:?} reports the outcome the attempt \
                 ended with, got {lines:?}"
            );
        }

        // A FURTHER read of the settled run exports nothing again: the lines a run owes
        // are the ones its execution produced, so reading it twice does not duplicate
        // them.
        let before = log_messages(&router, log_group, &run_id).await;
        let _ = run_to_completion(&router, &arn, &run_id, case).await;
        let after = log_messages(&router, log_group, &run_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: re-reading a settled run must not export its lines twice"
        );
    }
}

#[tokio::test]
async fn a_stopped_runs_shutdown_reaches_the_configured_log_group() {
    const LOG_GROUP: &str = "/aws/mwaa-serverless/stopped-runs";
    let router = emulator();
    create_bucket(&router).await;

    let key = "dags/run-log-export-stop.yaml";
    stage(
        &router,
        key,
        b"tasks:\n  - id: extract\n  - id: transform\n    dependencies: [extract]\n",
    )
    .await;
    let arn = create_workflow(&router, "runLogExportStop", key, Some(LOG_GROUP)).await;
    let run_id = start_run(&router, &arn, "stopped run").await;

    let (status, stopped) = call(
        &router,
        "StopWorkflowRun",
        json!({"WorkflowArn": arn, "RunId": run_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the run must be stoppable, got {status} {stopped:?}"
    );

    // The shutdown is a transition of the run like any other, so the run's own stream
    // carries the status the stop moved it to.
    let lines = log_messages(&router, LOG_GROUP, &run_id).await;
    assert!(
        lines.iter().any(|message| message.contains("STOPPING")),
        "the run's stream must report the status its stop moved it to, got {lines:?}"
    );
}

/// The published bounds on a CloudWatch Logs log group name. Source: the shared reading of
/// that rule, `service_common::cloudwatch_logs` — CloudWatch Logs holds a group under a
/// name of 1 to 512 characters of letters, digits and `. - _ / #`.
const MIN_LOG_GROUP_NAME_CHARS: usize = 1;
const MAX_LOG_GROUP_NAME_CHARS: usize = 512;

#[tokio::test]
async fn a_configured_log_group_must_be_a_name_cloudwatch_logs_can_hold() {
    let router = emulator();
    create_bucket(&router).await;
    let key = "dags/run-log-export-names.yaml";
    stage(&router, key, b"tasks:\n  - id: extract\n").await;

    // Each published bound of the length, one character past it, and a character inside
    // and outside the published set.
    let shortest = "g".repeat(MIN_LOG_GROUP_NAME_CHARS);
    let longest = "g".repeat(MAX_LOG_GROUP_NAME_CHARS);
    let past_longest = "g".repeat(MAX_LOG_GROUP_NAME_CHARS + 1);
    // (case, the configured name, whether the create is served)
    let rows: &[(&str, &str, bool)] = &[
        ("the shortest name", &shortest, true),
        ("the longest name", &longest, true),
        ("one character past the longest", &past_longest, false),
        ("the empty name", "", false),
        ("every punctuation character of the set", "a.b-c_d/e#f", true),
        ("a space", "log group", false),
        ("a colon", "/aws/mwaa:serverless", false),
        ("a non-ASCII letter", "grüße", false),
    ];

    // The workflow every row's UPDATE leg is applied to, created with a name the rule
    // admits so each row's answer is about the name that row carries.
    let updated_workflow = create_workflow(
        &router,
        "logGroupNameUpdates",
        key,
        Some("/aws/mwaa-serverless/updates"),
    )
    .await;

    for (index, (case, name, served)) in rows.iter().enumerate() {
        let expected = if *served { 200 } else { 400 };
        let body = json!({
            "Name": format!("logGroupName{index}"),
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
            "RoleArn": role_arn(),
            "LoggingConfiguration": {"LogGroupName": name},
        });
        let (status, answered) = call(&router, "CreateWorkflow", body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected CreateWorkflow to answer {expected}, got {status} \
             {answered:?}"
        );
        if !*served {
            assert_eq!(
                answered["Reason"].as_str(),
                Some("fieldValidationFailed"),
                "case {case:?}: the refusal states the member that failed, got {answered:?}"
            );
            assert_eq!(
                answered["FieldList"][0]["Name"].as_str(),
                Some("LoggingConfiguration.LogGroupName"),
                "case {case:?}: the refusal names the log group member, got {answered:?}"
            );
        }

        // The SAME table through the other operation that accepts the configuration: an
        // update appends a version whose runs export to the group it names, so it admits
        // exactly the names a create admits — neither more nor fewer.
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            json!({
                "WorkflowArn": updated_workflow,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
                "LoggingConfiguration": {"LogGroupName": name},
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected UpdateWorkflow to answer {expected}, got {status} \
             {updated:?}"
        );
        if !*served {
            assert_eq!(
                updated["FieldList"][0]["Name"].as_str(),
                Some("LoggingConfiguration.LogGroupName"),
                "case {case:?}: the update's refusal names the log group member, got {updated:?}"
            );
        }
    }
}
