// SPDX-License-Identifier: CC0-1.0

//! The wire contract an MWAA Serverless client reads a workflow, a run and its task
//! instances back on, over the assembled emulator: `awsJson1_0`, `POST /`,
//! `X-Amz-Target: AmazonMWAAServerless.<Operation>`.
//!
//! Five rules, each asserted over a TABLE rather than against one recorded request. A
//! definition is an Amazon S3 object this service snapshots by calling S3 in process, so
//! S3 and CloudWatch Logs (where a run's output is exported) are mounted beside it on one
//! engine exactly as `core/router/src/lib.rs` mounts every service.
//!
//! 1. A timestamp member whose model declares `@timestampFormat("date-time")` is an ISO
//!    8601 / RFC 3339 date-time STRING on the wire, never the epoch-seconds number the
//!    protocol defaults to. Every timestamp of this service targets `TimestampValue`, which
//!    carries that trait (`smithy_query(query="shape", shape="TimestampValue",
//!    model_path="services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json")`),
//!    so the rows span every operation that reports one.
//! 2. `RevisionId` is minted in the service's own identifier form — a canonical
//!    36-character hyphenated UUID — which is a different form from the 32-character
//!    lowercase-hex `WorkflowVersion`, and it changes whenever the version does.
//! 3. The service DEFAULTS the trigger mode and the logging configuration a create or an
//!    update leaves out: the trigger mode from the schedule the snapshotted definition
//!    declares, and the log group from the workflow's own id. A supplied value is carried
//!    unchanged.
//! 4. A run reports the parameter set it executes with, which the service seeds with the
//!    entry naming the run's workflow before merging the caller's own overrides.
//! 5. A run materialises ONE task instance per task its definition declares, wherever that
//!    definition writes its DAG-level `tasks` collection — at the document root, under a
//!    `dag` wrapper, or under the key naming the DAG itself — and both readers
//!    (`GetWorkflowRun`'s `RunDetail.TaskInstances` and `ListTaskInstances`) report the
//!    same instances, each under the identifier the RUN minted for it (not the identifier
//!    its definition declared for the task, which the instance reports separately as
//!    `TaskId`), with the operator its declaration names and the status its execution
//!    reached.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The regional endpoint every request here is signed for, and the region the definition
/// objects are staged in.
const REGION: &str = "us-west-2";

/// The bucket every definition below is staged in.
const BUCKET: &str = "mwaa-serverless-wire-contract";

/// An IAM role ARN of this account, as the modelled `RoleARN` writes one
/// (`smithy_query(query="shape", shape="RoleARN")`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/mwaa-serverless-exec";

/// How long one poll of a run's state waits before asking again, and how many times. The
/// budget is deliberately generous: it bounds a condition that has to HOLD — the run
/// reporting a status it never leaves — and not how fast the host runs, and each wait
/// returns as soon as that status is reported.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLLS: usize = 1_200;

/// The `WorkflowRunStatus` values a run has finished in and never leaves
/// (`smithy_query(query="shape", shape="WorkflowRunStatus")`).
const TERMINAL_RUN_STATES: &[&str] = &["SUCCESS", "FAILED", "TIMEOUT", "STOPPED"];

/// The `TaskInstanceStatus` a task that ran to completion reports.
const TASK_SUCCESS: &str = "SUCCESS";

/// This service, Amazon S3 (a definition is an object a version snapshots) and CloudWatch
/// Logs (where a run's output is exported) on ONE shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same emulator, built on an engine the CALLER keeps a handle to, so a suite whose
/// rule is about a value no modelled reader reports can read the stored record while
/// still driving every request through this wire path.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
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

/// The `RevisionId` and the current `WorkflowVersion` the STORE holds for `arn`.
///
/// Read from the store because `CreateWorkflow` is the only modelled operation that
/// REPORTS a revision id (`smithy_query(query="operation",
/// operation="CreateWorkflow,UpdateWorkflow,GetWorkflow")`: no other response, and no
/// summary shape, carries the member), so the revision a later version carries is not
/// reachable over the wire.
fn stored_revision(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> (String, String) {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

    let state = engine.state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let store = state.lock();
    let workflow = store
        .workflows
        .get(arn)
        .expect("setup: the created workflow is stored");
    (
        workflow.revision_id.clone(),
        workflow.current_version.clone(),
    )
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes.
async fn s3(router: &MultiServiceRouter, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    router.dispatch(request).await.status().as_u16()
}

/// Create [`BUCKET`], naming its location constraint, because Amazon S3 serves a create
/// that names none only at the endpoint the unspecified constraint stands for.
async fn create_bucket(router: &MultiServiceRouter) {
    let status = s3(
        router,
        &format!("/{BUCKET}"),
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(status, 200, "the bucket must be created first, got {status}");
}

/// Store `content` at `key` in [`BUCKET`].
async fn stage(router: &MultiServiceRouter, key: &str, content: &str) {
    let status = s3(router, &format!("/{BUCKET}/{key}"), content.as_bytes()).await;
    assert_eq!(
        status, 200,
        "case {key:?}: the definition must be stored, got {status}"
    );
}

/// Call one MWAA Serverless operation over its `awsJson1_0` wire.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let payload = serde_json::to_vec(&body).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("airflow-serverless.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonMWAAServerless.{operation}"))
        .header("authorization", authorization("airflow-serverless"))
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The `CreateWorkflow` request body for a workflow whose definition is the object `key`,
/// carrying only the members `extra` adds beyond the required ones.
fn create_body(name: &str, key: &str, extra: &Value) -> Value {
    let mut body = json!({
        "Name": name,
        "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
        "RoleArn": ROLE_ARN,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Create one workflow and answer the whole `CreateWorkflow` answer.
async fn create_workflow(router: &MultiServiceRouter, name: &str, key: &str, extra: &Value) -> Value {
    let (status, created) = call(router, "CreateWorkflow", create_body(name, key, extra)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created
}

/// The ARN out of one `CreateWorkflow` answer.
fn arn_of(created: &Value) -> String {
    created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string()
}

/// Start one run of `arn` and answer its `RunId`, carrying `overrides` when they are not
/// `Null`.
async fn start_run(router: &MultiServiceRouter, arn: &str, overrides: &Value, case: &str) -> String {
    let mut body = json!({"WorkflowArn": arn});
    if !overrides.is_null() {
        body["OverrideParameters"] = overrides.clone();
    }
    let (status, started) = call(router, "StartWorkflowRun", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    started["RunId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a started run has a RunId"))
        .to_string()
}

/// Poll `GetWorkflowRun` until the run reports a status it never leaves, and answer that
/// whole answer. The loop waits for the CONDITION, so it returns as soon as the run has
/// settled.
async fn settled_run(router: &MultiServiceRouter, arn: &str, run_id: &str, case: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..POLLS {
        let (status, report) = call(
            router,
            "GetWorkflowRun",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {report:?}"
        );
        let state = report["RunDetail"]["RunState"].as_str().unwrap_or_default();
        if TERMINAL_RUN_STATES.contains(&state) {
            return report;
        }
        last = report;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the run never settled; last answer {last:?}")
}

/// Whether `text` is a Smithy `date-time`: an RFC 3339 instant in UTC, as
/// `aws_smithy_types`' own reader for that format parses one — which is what a generated
/// SDK client parses such a member with.
fn is_date_time(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
            .is_ok()
    })
}

/// Whether `text` is a canonical UUID: five hyphen-separated groups of 8, 4, 4, 4 and 12
/// lowercase hex digits.
fn is_canonical_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    let widths = [8, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups
            .iter()
            .zip(widths)
            .all(|(group, width)| group.len() == width)
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// Whether `text` is a modelled `WorkflowVersion`: `^[a-z0-9]{32}$`, length 32..32
/// (`smithy_query(query="shape", shape="WorkflowVersion")`).
fn is_workflow_version(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

/// A definition declaring one `EmptyOperator` task, used wherever a row only needs a
/// workflow that runs.
const ONE_TASK_DEFINITION: &str = "tasks:\n  - id: extract\n    operator: EmptyOperator\n";

/// The `TaskInstanceId`s a `GetWorkflowRun` answer reports as `RunDetail.TaskInstances`,
/// in the order it reported them.
fn reported_instance_ids(report: &Value) -> Vec<String> {
    report["RunDetail"]["TaskInstances"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Which instance of this run executes which DECLARED task: the `TaskInstanceId` the run
/// reports for each task, keyed by the `TaskId` that instance names.
///
/// A task instance's identifier is the service's own, minted for the instance the run
/// created, so a client DISCOVERS it from the run (`RunDetail.TaskInstances`) and learns
/// which declared task it executes by READING the instance — `GetTaskInstance` is the only
/// reader that reports `TaskInstanceId` and `TaskId` together, because `TaskInstanceSummary`
/// carries no `TaskId`.
async fn instances_by_task(
    router: &MultiServiceRouter,
    workflow_arn: &str,
    run_id: &str,
    report: &Value,
    case: &str,
) -> BTreeMap<String, String> {
    let mut by_task: BTreeMap<String, String> = BTreeMap::new();
    for id in reported_instance_ids(report) {
        let (status, read) = call(
            router,
            "GetTaskInstance",
            json!({"WorkflowArn": workflow_arn, "RunId": run_id, "TaskInstanceId": id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an instance the run reports must be readable by that identifier, \
             got {status} {read:?}"
        );
        assert_eq!(
            read["TaskInstanceId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the instance to report the identifier it is addressed \
             by, got {:?}",
            read["TaskInstanceId"]
        );
        let task_id = read["TaskId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: instance {id:?} must name the task it executes, got {read:?}")
            })
            .to_string();
        assert_ne!(
            task_id, id,
            "case {case:?}: an identifier is minted for the INSTANCE, so it is not the \
             identifier the definition declared for the task it executes"
        );
        let already = by_task.insert(task_id.clone(), id.clone());
        assert!(
            already.is_none(),
            "case {case:?}: task {task_id:?} runs once in this run, so one instance may name \
             it, got {already:?} and {id:?}"
        );
    }
    by_task
}

#[tokio::test]
async fn every_reported_instant_is_an_iso_8601_date_time_string() {
    let router = emulator();
    create_bucket(&router).await;
    let key = "dags/timestamps.yaml";
    stage(&router, key, ONE_TASK_DEFINITION).await;

    // A create that supplies a `Code` location too, so the one timestamp only such a
    // version reports (`CodeSnapshottedAt`) is reported as well.
    let created = create_workflow(
        &router,
        "wireTimestamps",
        key,
        &json!({"Code": {"S3Location": {"Bucket": BUCKET, "ObjectKey": "code.zip"}}}),
    )
    .await;
    let arn = arn_of(&created);

    let (_, fetched) = call(&router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
    let (_, listed) = call(&router, "ListWorkflows", json!({})).await;
    let (_, versions) = call(&router, "ListWorkflowVersions", json!({"WorkflowArn": arn})).await;

    let run_id = start_run(&router, &arn, &Value::Null, "timestamps").await;
    let (_, started) = call(
        &router,
        "StartWorkflowRun",
        json!({"WorkflowArn": arn, "ClientToken": "timestamps-replay"}),
    )
    .await;
    let settled = settled_run(&router, &arn, &run_id, "timestamps").await;
    let (_, runs) = call(&router, "ListWorkflowRuns", json!({"WorkflowArn": arn})).await;
    // The instance is addressed by the identifier the RUN minted for it, discovered from
    // the run itself: a task-instance identifier is the service's own, not the name the
    // definition declared for the task.
    let instance_id = reported_instance_ids(&settled)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("case timestamps: the settled run must report its task instance, got {settled:?}")
        });
    let (_, instance) = call(
        &router,
        "GetTaskInstance",
        json!({"WorkflowArn": arn, "RunId": run_id, "TaskInstanceId": instance_id}),
    )
    .await;

    // The table: every member of this service that reports an instant, named by the
    // operation that reports it. The rows span the operations and the shapes the member
    // sits on — a response's own member, a member of a nested structure, and a member of a
    // listing's summary.
    let workflow_summary = &listed["Workflows"][0];
    let version_summary = &versions["WorkflowVersions"][0];
    let run_summary = &runs["WorkflowRuns"][0];
    let detail = &settled["RunDetail"];
    let rows: Vec<(&str, &Value)> = vec![
        ("CreateWorkflow $.CreatedAt", &created["CreatedAt"]),
        ("GetWorkflow $.CreatedAt", &fetched["CreatedAt"]),
        ("GetWorkflow $.ModifiedAt", &fetched["ModifiedAt"]),
        ("GetWorkflow $.CodeSnapshottedAt", &fetched["CodeSnapshottedAt"]),
        ("ListWorkflows $.Workflows[0].CreatedAt", &workflow_summary["CreatedAt"]),
        ("ListWorkflows $.Workflows[0].ModifiedAt", &workflow_summary["ModifiedAt"]),
        (
            "ListWorkflowVersions $.WorkflowVersions[0].CreatedAt",
            &version_summary["CreatedAt"],
        ),
        (
            "ListWorkflowVersions $.WorkflowVersions[0].ModifiedAt",
            &version_summary["ModifiedAt"],
        ),
        ("StartWorkflowRun $.StartedAt", &started["StartedAt"]),
        ("GetWorkflowRun $.RunDetail.CreatedAt", &detail["CreatedAt"]),
        ("GetWorkflowRun $.RunDetail.StartedOn", &detail["StartedOn"]),
        ("GetWorkflowRun $.RunDetail.ModifiedAt", &detail["ModifiedAt"]),
        ("GetWorkflowRun $.RunDetail.CompletedOn", &detail["CompletedOn"]),
        (
            "ListWorkflowRuns $.WorkflowRuns[0].RunDetailSummary.CreatedOn",
            &run_summary["RunDetailSummary"]["CreatedOn"],
        ),
        (
            "ListWorkflowRuns $.WorkflowRuns[0].RunDetailSummary.StartedAt",
            &run_summary["RunDetailSummary"]["StartedAt"],
        ),
        (
            "ListWorkflowRuns $.WorkflowRuns[0].RunDetailSummary.EndedAt",
            &run_summary["RunDetailSummary"]["EndedAt"],
        ),
        ("GetTaskInstance $.StartedAt", &instance["StartedAt"]),
        ("GetTaskInstance $.EndedAt", &instance["EndedAt"]),
        ("GetTaskInstance $.ModifiedAt", &instance["ModifiedAt"]),
    ];
    for (case, reported) in rows {
        assert!(
            is_date_time(reported),
            "case {case:?}: expected an ISO 8601 date-time string, got {reported:?}"
        );
    }

    // The instant a create reported and the instant a read reports are the SAME text, so
    // the form is not merely well-shaped per response.
    assert_eq!(
        fetched["CreatedAt"], created["CreatedAt"],
        "GetWorkflow must report the instant the create reported, got {:?} against {:?}",
        fetched["CreatedAt"], created["CreatedAt"]
    );
}

#[tokio::test]
async fn a_revision_id_is_the_services_own_identifier_form_and_tracks_the_version() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_bucket(&router).await;
    let key = "dags/revisions.yaml";
    stage(&router, key, ONE_TASK_DEFINITION).await;

    let created = create_workflow(&router, "wireRevisions", key, &json!({})).await;
    let arn = arn_of(&created);
    let reported = created["RevisionId"].as_str().unwrap_or_default().to_string();
    let (stored, version) = stored_revision(&engine, &arn);
    assert_eq!(
        reported, stored,
        "the create must report the revision the store holds, got {reported:?} against \
         {stored:?}"
    );

    // Rows: the create, then one revision per update, so the rule is asserted over a
    // version HISTORY rather than a single mint.
    let mut seen: Vec<(String, String, String)> =
        vec![("the create".to_string(), reported, version)];
    for round in 1..=2 {
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": ROLE_ARN,
                "Description": format!("revision {round}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "update {round}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
        let (revision, version) = stored_revision(&engine, &arn);
        assert_eq!(
            Some(version.as_str()),
            updated["WorkflowVersion"].as_str(),
            "update {round}: the update must report the version it appended, got {:?}",
            updated["WorkflowVersion"]
        );
        seen.push((format!("update {round}"), revision, version));
    }

    for (case, revision, version) in &seen {
        assert!(
            is_canonical_uuid(revision),
            "case {case:?}: expected a canonical 36-character hyphenated UUID RevisionId, \
             got {revision:?}"
        );
        // The two identifiers the service mints for one version are different FORMS, so a
        // client that tells a revision from a version by its shape can.
        assert!(
            is_workflow_version(version),
            "case {case:?}: expected a 32-character lowercase-hex WorkflowVersion, got \
             {version:?}"
        );
        assert!(
            !is_workflow_version(revision),
            "case {case:?}: a revision id must not be shaped like a WorkflowVersion, got \
             {revision:?}"
        );
    }
    // Every version of the history has its own revision: the identifier is what optimistic
    // concurrency control compares, so it cannot repeat across versions.
    for (index, (case, revision, _)) in seen.iter().enumerate() {
        for (other_case, other, _) in seen.iter().skip(index + 1) {
            assert_ne!(
                revision, other,
                "case {case:?} and {other_case:?}: each version must carry its own revision id"
            );
        }
    }
}

/// One row of the defaults table: what the create supplies beyond the required members,
/// whether its definition declares a schedule, and the `TriggerMode` the workflow must
/// then report.
struct DefaultsRow {
    case: &'static str,
    definition: &'static str,
    supplied: fn() -> Value,
    expected_trigger_mode: &'static str,
    /// The `LoggingConfiguration.LogGroupName` the workflow must report, and `None` for a
    /// row that supplies none — whose expectation is the group the SERVICE names.
    expected_log_group: Option<&'static str>,
}

/// The table. Rows differ along what the rule generalises over: whether the definition
/// declares a schedule, and which of the two defaulted members the request supplies.
fn defaults_rows() -> Vec<DefaultsRow> {
    vec![
        DefaultsRow {
            case: "neither member supplied, no schedule declared",
            definition: ONE_TASK_DEFINITION,
            supplied: || json!({}),
            expected_trigger_mode: "manual_only",
            expected_log_group: None,
        },
        DefaultsRow {
            case: "neither member supplied, a schedule declared at the document root",
            definition: "schedule: \"30 4 * * *\"\ntasks:\n  - id: extract\n",
            supplied: || json!({}),
            expected_trigger_mode: "scheduled",
            expected_log_group: None,
        },
        DefaultsRow {
            case: "neither member supplied, a schedule declared under the dag id",
            definition: "nightly:\n  schedule: \"@daily\"\n  tasks:\n    extract:\n      \
                         operator: EmptyOperator\n",
            supplied: || json!({}),
            expected_trigger_mode: "scheduled",
            expected_log_group: None,
        },
        DefaultsRow {
            case: "a trigger mode supplied, so it is carried unchanged",
            definition: ONE_TASK_DEFINITION,
            supplied: || json!({"TriggerMode": "ON_DEMAND"}),
            expected_trigger_mode: "ON_DEMAND",
            expected_log_group: None,
        },
        DefaultsRow {
            case: "a log group supplied, so it is carried unchanged",
            definition: ONE_TASK_DEFINITION,
            supplied: || json!({"LoggingConfiguration": {"LogGroupName": "/aws/vendedlogs/mine"}}),
            expected_trigger_mode: "manual_only",
            expected_log_group: Some("/aws/vendedlogs/mine"),
        },
        DefaultsRow {
            case: "both members supplied, and a schedule declared as well",
            definition: "schedule_interval: \"*/20 * * * *\"\ntasks: []\n",
            supplied: || {
                json!({
                    "TriggerMode": "EVENT",
                    "LoggingConfiguration": {"LogGroupName": "mwaa-serverless-mine"},
                })
            },
            expected_trigger_mode: "EVENT",
            expected_log_group: Some("mwaa-serverless-mine"),
        },
    ]
}

#[tokio::test]
async fn the_service_defaults_the_trigger_mode_and_the_log_group_a_request_leaves_out() {
    let router = emulator();
    create_bucket(&router).await;

    for (index, row) in defaults_rows().iter().enumerate() {
        let case = row.case;
        let key = format!("dags/defaults-{index}.yaml");
        stage(&router, &key, row.definition).await;
        let created =
            create_workflow(&router, &format!("wireDefaults{index}"), &key, &(row.supplied)())
                .await;
        let arn = arn_of(&created);

        let (status, fetched) = call(&router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["TriggerMode"].as_str(),
            Some(row.expected_trigger_mode),
            "case {case:?}: expected TriggerMode {:?}, got {:?}",
            row.expected_trigger_mode,
            fetched["TriggerMode"]
        );
        // The group the service names is the fixed prefix, the workflow's own id — the
        // resource-id segment of its ARN — and a trailing slash, so it is derived here
        // from the ARN rather than written out.
        let workflow_id = arn.rsplit_once('/').map(|(_, id)| id).unwrap_or(&arn);
        let expected_group = row
            .expected_log_group
            .map(str::to_string)
            .unwrap_or_else(|| format!("/aws/mwaa-serverless/{workflow_id}/"));
        assert_eq!(
            fetched["LoggingConfiguration"]["LogGroupName"].as_str(),
            Some(expected_group.as_str()),
            "case {case:?}: expected the log group {expected_group:?}, got {:?}",
            fetched["LoggingConfiguration"]
        );

        // The same two values reach every other reader of the configuration, because they
        // are what the version FROZE.
        let (_, listed) = call(&router, "ListWorkflows", json!({})).await;
        let summary = listed["Workflows"]
            .as_array()
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| summary["WorkflowArn"].as_str() == Some(arn.as_str()))
            })
            .unwrap_or_else(|| panic!("case {case:?}: the listing must report {arn:?}"));
        assert_eq!(
            summary["TriggerMode"].as_str(),
            Some(row.expected_trigger_mode),
            "case {case:?}: the listing must report the same TriggerMode, got {:?}",
            summary["TriggerMode"]
        );
    }
}

/// The `OverrideParameters` maps the injection table covers: a `map<string, document>`, so
/// the rows differ along presence, size, the JSON type of a value, and whether the caller
/// writes the service's own key itself.
fn override_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("no OverrideParameters at all", Value::Null),
        ("an empty map", json!({})),
        ("one string parameter", json!({"env": "test"})),
        (
            "parameters of several JSON types",
            json!({"retries": 3, "ratio": 0.5, "dry_run": true, "note": "mixed"}),
        ),
        (
            "a nested object and a list",
            json!({"config": {"region": REGION, "tags": ["a", "b"]}, "partitions": [1, 2]}),
        ),
        (
            "the service's own key, supplied by the caller",
            json!({"mwaa_serverless_workflow_id": "chosen-by-the-caller"}),
        ),
    ]
}

#[tokio::test]
async fn a_run_reports_the_parameter_set_it_executes_with() {
    let router = emulator();
    create_bucket(&router).await;
    let key = "dags/parameters.yaml";
    stage(&router, key, ONE_TASK_DEFINITION).await;
    let created = create_workflow(&router, "wireParameters", key, &json!({})).await;
    let arn = arn_of(&created);
    let workflow_id = arn.rsplit_once('/').map(|(_, id)| id).unwrap_or(&arn).to_string();

    for (case, overrides) in override_rows() {
        let run_id = start_run(&router, &arn, &overrides, case).await;
        let (status, read) = call(
            &router,
            "GetWorkflowRun",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {read:?}"
        );

        // The service seeds the set with the entry naming the run's workflow, then the
        // caller's own parameters are merged over it — so a caller that writes that key
        // itself overrides it like any other.
        let mut expected = serde_json::Map::new();
        expected.insert(
            "mwaa_serverless_workflow_id".to_string(),
            Value::String(workflow_id.clone()),
        );
        if let Some(supplied) = overrides.as_object() {
            for (member, value) in supplied {
                expected.insert(member.clone(), value.clone());
            }
        }
        assert_eq!(
            read["OverrideParameters"],
            Value::Object(expected),
            "case {case:?}: expected the injected workflow-id entry with the supplied \
             parameters over it, got {:?}",
            read["OverrideParameters"]
        );
    }
}

/// One row of the fan-out table: a definition, and the task instances a run of it must
/// materialise — each `(TaskId, OperatorName)`, in the order both readers report them. The
/// identifier each instance is reported UNDER is not in the definition: the run mints it,
/// and the test resolves it through [`instances_by_task`].
struct FanOutRow {
    case: &'static str,
    definition: &'static str,
    expected: &'static [(&'static str, Option<&'static str>)],
}

/// The table. Rows differ along WHERE the definition writes its DAG-level `tasks`
/// collection (the document root, a `dag` wrapper, the key naming the DAG itself), the FORM
/// of that collection (a sequence, a mapping), how many tasks it declares, whether they
/// depend on each other, and how the declaration names the operator (by its class name, by
/// its fully qualified Python import path, not at all).
fn fan_out_rows() -> Vec<FanOutRow> {
    vec![
        FanOutRow {
            case: "one task in a tasks sequence at the document root",
            definition: ONE_TASK_DEFINITION,
            expected: &[("extract", Some("EmptyOperator"))],
        },
        FanOutRow {
            case: "two dependent tasks under the key naming the DAG",
            definition: "two_task_dag:\n  schedule: null\n  tasks:\n    first_task:\n      \
                         operator: airflow.providers.standard.operators.bash.BashOperator\n      \
                         bash_command: \"echo one\"\n    second_task:\n      operator: \
                         airflow.providers.standard.operators.bash.BashOperator\n      \
                         bash_command: \"echo two\"\n      dependencies: [first_task]\n",
            expected: &[
                ("first_task", Some("BashOperator")),
                ("second_task", Some("BashOperator")),
            ],
        },
        FanOutRow {
            case: "three chained tasks under the key naming the DAG",
            definition: "chain_dag:\n  tasks:\n    - id: extract\n      operator: \
                         airflow.operators.empty.EmptyOperator\n    - id: transform\n      \
                         dependencies: [extract]\n    - id: load\n      operator: \
                         PythonOperator\n      depends_on: transform\n",
            expected: &[
                ("extract", Some("EmptyOperator")),
                ("load", Some("PythonOperator")),
                ("transform", None),
            ],
        },
        FanOutRow {
            case: "two tasks under a dag wrapper",
            definition: "dag:\n  dag_id: wrapped\n  tasks:\n    - id: alpha\n      operator: \
                         BashOperator\n    - id: beta\n      dependencies: [alpha]\n      \
                         operator: BashOperator\n",
            expected: &[("alpha", Some("BashOperator")), ("beta", Some("BashOperator"))],
        },
        FanOutRow {
            case: "a definition declaring no task at all",
            definition: "empty_dag:\n  tasks: []\n",
            expected: &[],
        },
    ]
}

#[tokio::test]
async fn a_run_materialises_one_task_instance_per_declared_task() {
    let router = emulator();
    create_bucket(&router).await;

    // Every row's run is started before any of them is polled, so the executions overlap
    // on the one clock they all run on.
    let rows = fan_out_rows();
    let mut started = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let key = format!("dags/fan-out-{index}.yaml");
        stage(&router, &key, row.definition).await;
        let created =
            create_workflow(&router, &format!("wireFanOut{index}"), &key, &json!({})).await;
        let arn = arn_of(&created);
        let run_id = start_run(&router, &arn, &Value::Null, row.case).await;
        started.push((row, arn, run_id));
    }

    for (row, arn, run_id) in started {
        let case = row.case;
        let settled = settled_run(&router, &arn, &run_id, case).await;
        let detail = &settled["RunDetail"];
        assert_eq!(
            detail["RunState"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: every declared task finished, so the run completed \
             successfully, got {:?}",
            detail["RunState"]
        );

        let expected_tasks: Vec<&str> = row.expected.iter().map(|(id, _)| *id).collect();
        // `RunDetail.TaskInstances` is "a list of task instance IDs that are part of this
        // workflow run", so a run with declared work reports one MINTED id per declared
        // task and one with none reports no member at all.
        let reported_ids: Vec<String> = reported_instance_ids(&settled);
        let instances = instances_by_task(&router, &arn, &run_id, &settled, case).await;
        let executed: Vec<&str> = instances.keys().map(String::as_str).collect();
        assert_eq!(
            executed, expected_tasks,
            "case {case:?}: expected the run to execute the declared tasks \
             {expected_tasks:?}, got {executed:?}"
        );
        let mut minted: Vec<&str> = instances.values().map(String::as_str).collect();
        minted.sort_unstable();
        assert_eq!(
            reported_ids.iter().map(String::as_str).collect::<Vec<&str>>(),
            minted,
            "case {case:?}: expected RunDetail.TaskInstances to name the minted identifiers \
             {minted:?}, got {:?}",
            detail["TaskInstances"]
        );

        // The listing reports the same instances, each carrying the run it belongs to and
        // the outcome its execution reached.
        let (status, page) = call(
            &router,
            "ListTaskInstances",
            json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTaskInstances to succeed, got {status} {page:?}"
        );
        let summaries = page["TaskInstances"].as_array().cloned().unwrap_or_default();
        let listed_ids: Vec<&str> = summaries
            .iter()
            .filter_map(|summary| summary["TaskInstanceId"].as_str())
            .collect();
        assert_eq!(
            listed_ids, minted,
            "case {case:?}: expected ListTaskInstances to report {minted:?}, got \
             {listed_ids:?}"
        );
        for (task_id, operator) in row.expected {
            let id = instances.get(*task_id).unwrap_or_else(|| {
                panic!("case {case:?}: expected the run to execute task {task_id:?}")
            });
            let summary = summaries
                .iter()
                .find(|summary| summary["TaskInstanceId"].as_str() == Some(id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a summary for {id:?}, got {summaries:?}")
                });
            assert_eq!(
                summary["Status"].as_str(),
                Some(TASK_SUCCESS),
                "case {case:?}: expected task {task_id:?} to report {TASK_SUCCESS}, got {:?}",
                summary["Status"]
            );
            // The operator a summary reports is the NAME the declaration names, which for
            // a fully qualified Python import path is the class at its end.
            assert_eq!(
                summary["OperatorName"].as_str(),
                *operator,
                "case {case:?}: expected task {task_id:?} to report OperatorName \
                 {operator:?}, got {:?}",
                summary["OperatorName"]
            );
            assert_eq!(
                summary["RunId"].as_str(),
                Some(run_id.as_str()),
                "case {case:?}: expected task {task_id:?} to name its run, got {:?}",
                summary["RunId"]
            );
            assert_eq!(
                summary["WorkflowArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected task {task_id:?} to name its workflow, got {:?}",
                summary["WorkflowArn"]
            );
            assert_eq!(
                summary["WorkflowVersion"].as_str(),
                detail["WorkflowVersion"].as_str(),
                "case {case:?}: expected task {task_id:?} to name the version the run \
                 executed, got {:?}",
                summary["WorkflowVersion"]
            );
            assert!(
                summary["DurationInSeconds"].as_i64().is_some_and(|seconds| seconds >= 0),
                "case {case:?}: a finished task reports how long it took, got {:?}",
                summary["DurationInSeconds"]
            );
        }
    }
}
