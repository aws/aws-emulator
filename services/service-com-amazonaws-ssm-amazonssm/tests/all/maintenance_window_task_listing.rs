// SPDX-License-Identifier: CC0-1.0

//! The LISTING of a maintenance window's tasks end to end:
//! `RegisterTaskWithMaintenanceWindow` adds one and `DescribeMaintenanceWindowTasks` reports
//! the ones a window holds, narrowed by the documented filter keys and served one page at a
//! time, while `DeleteMaintenanceWindow` takes them with it.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response that
//! is not computed from persisted state cannot pass: every read follows a write.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! 1. **The listing reports what the registers persisted.** For the window the request names,
//!    it reports one entry per register, carrying exactly the members that register supplied
//!    (under the model's own `Type` member for the register's `TaskType`), in the order they
//!    were registered, and nothing for a member the register left out — including the
//!    placeholder `1` the register persisted for a targetless task's `MaxConcurrency` and
//!    `MaxErrors`.
//! 2. **The listing narrows by the documented filter keys** — `WindowTaskId`, `TaskArn`,
//!    `Priority` and `TaskType` — which intersect, each filter's values union, and a key
//!    outside that set is refused before anything is read.
//! 3. **A page holds `MaxResults` tasks** and the `NextToken` resumes the listing at the task
//!    the page stopped at; a token this read did not hand out is refused.
//! 4. **The listing is scoped to ONE window of ONE Region.** A window this endpoint holds none
//!    of — never created, created elsewhere, or deleted — is refused `DoesNotExistException`,
//!    and a window holding no task is reported as holding none.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::maintenance_window_task_service_role::service_linked_role_arn;
use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// A second Region, so a window and its tasks are shown to live at the endpoint of ONE Region.
const OTHER_REGION: &str = "eu-central-1";

/// A `WindowId` of the form the modelled `MaintenanceWindowId` pattern requires
/// (`^mw-[0-9a-f]{17}$`) that no create has minted.
const UNKNOWN_WINDOW_ID: &str = "mw-00000000000000000";

/// A `WindowTaskId` of the form the modelled `MaintenanceWindowTaskId` pattern requires that no
/// register has minted.
const UNKNOWN_TASK_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The members the listing reports only when the register supplied them, so a row that leaves
/// one out is checked to report nothing for it. `MaintenanceWindowTask`'s own member list
/// (`smithy_query(query="shape",shape="MaintenanceWindowTask")`) holds no
/// `TaskInvocationParameters`, so it is not among them.
///
/// `ServiceRoleArn` and `TaskParameters` are NOT among them either: the role a task runs as and
/// the parameter map it carries are materialised for every registered task, so the listing
/// reports both whatever the register stated (see
/// `all/maintenance_window_task_service_role.rs`). `CutoffBehavior` IS among them: its
/// documented `CONTINUE_TASK` is how a task that chose nothing behaves at the cutoff rather
/// than a value the register stores, and `MaintenanceWindowTask$CutoffBehavior` carries no
/// modelled default.
const OPTIONAL_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "LoggingInfo",
    "AlarmConfiguration",
    "MaxConcurrency",
    "MaxErrors",
    "CutoffBehavior",
];

/// Create a window at `region`'s endpoint and answer the id it minted.
async fn window(router: &MultiServiceRouter, case: &str, region: &str, name: &str) -> String {
    let (status, response) = call_in_region(
        router,
        region,
        "CreateMaintenanceWindow",
        &json!({
            "Name": name,
            "Schedule": "cron(0 3 ? * SUN *)",
            "Duration": 2,
            "Cutoff": 1,
            "AllowUnassociatedTargets": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the window every task of this case belongs to must be created, got \
         {status} {response}"
    );
    response["WindowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create reports a WindowId, got {response}"))
        .to_string()
}

/// `RegisterTaskWithMaintenanceWindow` at `region`'s endpoint, asserting it succeeded, and
/// answering the `WindowTaskId` it minted.
async fn register_ok(router: &MultiServiceRouter, case: &str, region: &str, body: &Value) -> String {
    let (status, response) =
        call_in_region(router, region, "RegisterTaskWithMaintenanceWindow", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the register {body} must succeed, got {status} {response}"
    );
    response["WindowTaskId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a register reports a WindowTaskId, got {response}")
        })
        .to_string()
}

/// `DescribeMaintenanceWindowTasks` at `region`'s endpoint with `body`.
async fn describe(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribeMaintenanceWindowTasks", body).await
}

/// The tasks one window holds, in the order the listing reports them.
async fn tasks_of(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    window_id: &str,
) -> Vec<Value> {
    let (status, response) = describe(router, region, &json!({ "WindowId": window_id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the listing of {window_id:?} must succeed, got {status} {response}"
    );
    response["Tasks"].as_array().cloned().unwrap_or_default()
}

/// The `WindowTaskId`s of the entries `reported` holds, in the order they are reported.
fn ids_of(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|task| {
            task["WindowTaskId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `Priority` the listing reports, which is `0` whether the member is serialized or left to
/// its modelled default (`MaintenanceWindowTask$Priority`, `default 0`).
fn reported_priority(reported: &Value) -> i64 {
    reported["Priority"].as_i64().unwrap_or(0)
}

/// RULE 1: the listing reports one entry per register of the window the request names, carrying
/// exactly the members that register supplied and nothing for a member it left out.
///
/// The rows differ along what the rule generalises over: which `MaintenanceWindowTaskType` the
/// register states (each of the four the modelled enum admits), which form its targets take
/// (node ids, a tag, none at all), which priority and bounds it carries, and which of the
/// optional labels, parameters and logging members it supplies.
#[tokio::test]
async fn the_listing_reports_what_each_register_persisted() {
    let router = router();
    let case = "the register table";
    let window_id = window(&router, case, REGION, "task-listing-table").await;

    // (case, the register body, the members the listing must report)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a RUN_COMMAND task with every member",
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "Name": "patch-the-fleet",
                "Description": "the weekly patch run",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/mw-task-role",
                "Priority": 3,
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "TaskParameters": { "Operation": { "Values": ["Install"] } },
            }),
            json!({
                "Type": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "Name": "patch-the-fleet",
                "Description": "the weekly patch run",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/mw-task-role",
                "Priority": 3,
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "TaskParameters": { "Operation": { "Values": ["Install"] } },
            }),
        ),
        (
            "an AUTOMATION task on a tag, with no labels at all",
            json!({
                "TaskType": "AUTOMATION",
                "TaskArn": "AWS-RestartEC2Instance",
                "MaxConcurrency": "10%",
                "MaxErrors": "0",
                "Targets": [{ "Key": "tag:Environment", "Values": ["production", "staging"] }],
            }),
            json!({
                "Type": "AUTOMATION",
                "TaskArn": "AWS-RestartEC2Instance",
                "MaxConcurrency": "10%",
                "MaxErrors": "0",
                "Priority": 0,
                // The register named no role, no parameters and no cutoff behaviour, so the
                // task runs as the account's service-linked role, holds the empty map, and is
                // reported with no CutoffBehavior at all.
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [{ "Key": "tag:Environment", "Values": ["production", "staging"] }],
            }),
        ),
        (
            "a targetless LAMBDA task, which holds the placeholder bounds",
            json!({
                "TaskType": "LAMBDA",
                "TaskArn": "arn:aws:lambda:us-west-2:123456789012:function:window-task",
                "Priority": 10,
                "Targets": [],
            }),
            json!({
                "Type": "LAMBDA",
                "TaskArn": "arn:aws:lambda:us-west-2:123456789012:function:window-task",
                "Priority": 10,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [],
                // "the system inserts a placeholder value of 1"
                // (`RegisterTaskWithMaintenanceWindowRequest$MaxConcurrency`).
                "MaxConcurrency": "1",
                "MaxErrors": "1",
            }),
        ),
        (
            "a STEP_FUNCTIONS task with logging and an alarm",
            json!({
                "TaskType": "STEP_FUNCTIONS",
                "TaskArn": "arn:aws:states:us-west-2:123456789012:stateMachine:window",
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-00000000000000001"] }],
                "LoggingInfo": {
                    "S3BucketName": "window-logs",
                    "S3Region": "us-west-2",
                    "S3KeyPrefix": "tasks/",
                },
                "AlarmConfiguration": {
                    "IgnorePollAlarmFailure": false,
                    "Alarms": [{ "Name": "window-task-alarm" }],
                },
            }),
            json!({
                "Type": "STEP_FUNCTIONS",
                "TaskArn": "arn:aws:states:us-west-2:123456789012:stateMachine:window",
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "Priority": 0,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-00000000000000001"] }],
                "LoggingInfo": {
                    "S3BucketName": "window-logs",
                    "S3Region": "us-west-2",
                    "S3KeyPrefix": "tasks/",
                },
                "AlarmConfiguration": {
                    "IgnorePollAlarmFailure": false,
                    "Alarms": [{ "Name": "window-task-alarm" }],
                },
            }),
        ),
    ];

    let mut registered: Vec<(&str, String, Value)> = Vec::new();
    for (why, mut body, expected) in rows {
        body["WindowId"] = Value::String(window_id.clone());
        let id = register_ok(&router, why, REGION, &body).await;
        registered.push((why, id, expected));
    }

    let held = tasks_of(&router, case, REGION, &window_id).await;
    assert_eq!(
        held.len(),
        registered.len(),
        "case {case:?}: expected one entry per register, got {held:?}"
    );
    for (position, (why, id, expected)) in registered.iter().enumerate() {
        // The order the listing reports them in is the order they were registered.
        let reported = &held[position];
        assert_eq!(
            reported["WindowTaskId"].as_str(),
            Some(id.as_str()),
            "case {why:?}: expected the task registered at position {position}, got {reported}"
        );
        assert_eq!(
            reported["WindowId"].as_str(),
            Some(window_id.as_str()),
            "case {why:?}: expected the window it was registered with, got {reported}"
        );
        for (member, value) in expected.as_object().expect("the expected members") {
            if member == "Priority" {
                assert_eq!(
                    reported_priority(reported),
                    value.as_i64().expect("an expected Priority"),
                    "case {why:?}: expected Priority {value}, got {reported}"
                );
                continue;
            }
            assert_eq!(
                &reported[member], value,
                "case {why:?}: expected {member} {value}, got {reported}"
            );
        }
        for member in OPTIONAL_MEMBERS {
            if expected.get(member).is_none() {
                assert!(
                    reported.get(member).is_none_or(Value::is_null),
                    "case {why:?}: expected no {member} for a register that carried none, got \
                     {reported}"
                );
            }
        }
        // `TaskType` is the REGISTER's spelling; the listed task states it as `Type`.
        assert!(
            reported.get("TaskType").is_none_or(Value::is_null),
            "case {why:?}: expected the listed task to state its type as Type, got {reported}"
        );
    }
}

/// RULE 2: the listing narrows by the documented filter keys, which intersect while each
/// filter's values union — and a key outside that set is refused before anything is read.
///
/// The rows differ along which documented key the filter names, how many values it unions, how
/// many filters intersect, and whether anything carries the value asked for.
#[tokio::test]
async fn the_listing_narrows_by_the_documented_filter_keys() {
    let router = router();
    let case = "the filter table";
    let window_id = window(&router, case, REGION, "filtered-tasks").await;
    let body = |task_type: &str, task_arn: &str, priority: i64| {
        json!({
            "WindowId": window_id,
            "TaskType": task_type,
            "TaskArn": task_arn,
            "Priority": priority,
            "MaxConcurrency": "1",
            "MaxErrors": "1",
            "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
        })
    };
    let run_command = register_ok(
        &router,
        case,
        REGION,
        &body("RUN_COMMAND", "AWS-RunPatchBaseline", 3),
    )
    .await;
    let automation = register_ok(
        &router,
        case,
        REGION,
        &body("AUTOMATION", "AWS-RestartEC2Instance", 5),
    )
    .await;
    let second_run_command = register_ok(
        &router,
        case,
        REGION,
        &body("RUN_COMMAND", "AWS-RunShellScript", 5),
    )
    .await;

    // (case, the filters, the task ids the listing must report)
    // The keys are the documented set of `DescribeMaintenanceWindowTasksRequest$Filters`:
    // `WindowTaskId`, `TaskArn`, `Priority` and `TaskType`.
    let rows: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "the task type",
            json!([{ "Key": "TaskType", "Values": ["RUN_COMMAND"] }]),
            vec![&run_command, &second_run_command],
        ),
        (
            "either task type, whose values union",
            json!([{ "Key": "TaskType", "Values": ["RUN_COMMAND", "AUTOMATION"] }]),
            vec![&run_command, &automation, &second_run_command],
        ),
        (
            "the task's own id",
            json!([{ "Key": "WindowTaskId", "Values": [automation] }]),
            vec![&automation],
        ),
        (
            "the task arn",
            json!([{ "Key": "TaskArn", "Values": ["AWS-RunShellScript"] }]),
            vec![&second_run_command],
        ),
        (
            "the priority, written as the decimal the caller sends",
            json!([{ "Key": "Priority", "Values": ["5"] }]),
            vec![&automation, &second_run_command],
        ),
        (
            "a priority and a type, which intersect",
            json!([
                { "Key": "Priority", "Values": ["5"] },
                { "Key": "TaskType", "Values": ["RUN_COMMAND"] },
            ]),
            vec![&second_run_command],
        ),
        (
            "two filters no one task satisfies",
            json!([
                { "Key": "Priority", "Values": ["3"] },
                { "Key": "TaskType", "Values": ["AUTOMATION"] },
            ]),
            vec![],
        ),
        (
            "a value no task carries",
            json!([{ "Key": "TaskArn", "Values": ["AWS-RunAnsiblePlaybook"] }]),
            vec![],
        ),
        (
            "a priority no task carries",
            json!([{ "Key": "Priority", "Values": ["7"] }]),
            vec![],
        ),
    ];
    for (why, filters, expected) in rows {
        let (status, response) = describe(
            &router,
            REGION,
            &json!({ "WindowId": window_id, "Filters": filters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why:?}: the filtered listing must be served, got {status} {response}"
        );
        let reported = ids_of(&response["Tasks"].as_array().cloned().unwrap_or_default());
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            reported, expected,
            "case {why:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // A key the operation does not narrow by names no property of a task.
    for (why, key) in [
        ("a key of the sibling target listing", "OwnerInformation"),
        ("the listed task's own spelling of its type", "Type"),
        ("a key nothing publishes", "NotAKey"),
    ] {
        let (status, response) = describe(
            &router,
            REGION,
            &json!({
                "WindowId": window_id,
                "Filters": [{ "Key": key, "Values": ["anything"] }],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {why:?}: expected a refusal of the key {key:?}, got {status} {response}"
        );
        assert!(
            error_message(&response).contains(key),
            "case {why:?}: expected the refusal to name the key submitted, got {response}"
        );
    }
}

/// RULE 3: a page holds `MaxResults` tasks and the `NextToken` resumes the listing at the task
/// the page stopped at, so a walk over the pages reports each task once — and a token this read
/// did not hand out is refused.
#[tokio::test]
async fn a_page_resumes_at_the_task_it_stopped_at() {
    let router = router();
    let case = "the paging case";
    let window_id = window(&router, case, REGION, "paged-tasks").await;
    // The smallest page the modelled `MaintenanceWindowMaxResults` admits (range 10..100), with
    // one task more than that, so the walk takes two pages.
    let page_size = 10;
    let mut minted: Vec<String> = Vec::new();
    for index in 0..=page_size {
        minted.push(
            register_ok(
                &router,
                case,
                REGION,
                &json!({
                    "WindowId": window_id,
                    "TaskType": "RUN_COMMAND",
                    "TaskArn": format!("AWS-RunTask-{index}"),
                    "MaxConcurrency": "1",
                    "MaxErrors": "1",
                    "Targets": [{ "Key": "tag:Index", "Values": [index.to_string()] }],
                }),
            )
            .await,
        );
    }

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "WindowId": window_id, "MaxResults": page_size });
        if let Some(next) = &token {
            body["NextToken"] = Value::String(next.clone());
        }
        let (status, response) = describe(&router, REGION, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: each page must be served, got {status} {response}"
        );
        let page = response["Tasks"].as_array().cloned().unwrap_or_default();
        assert!(
            page.len() <= page_size as usize,
            "case {case:?}: a page holds at most MaxResults tasks, got {}",
            page.len()
        );
        walked.extend(ids_of(&page));
        match response["NextToken"].as_str() {
            Some(next) if !next.is_empty() => token = Some(next.to_string()),
            _ => break,
        }
    }
    assert_eq!(
        walked, minted,
        "case {case:?}: expected the walk to report every task once, in the order they were \
         registered"
    );

    // A token this read did not hand out is not a page of it: one that is not shaped like a task
    // id, and one that is but names no task of this result.
    for (why, bad) in [
        ("a token this service did not mint", "not-a-token"),
        ("a task id that is no task of this window", UNKNOWN_TASK_ID),
    ] {
        let (status, response) = describe(
            &router,
            REGION,
            &json!({ "WindowId": window_id, "NextToken": bad }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {why:?}: expected a refusal, got {status} {response}"
        );
    }
}

/// RULE 4: the listing is scoped to one window of one Region — a window this endpoint holds
/// none of is refused, a window holding no task reports none, and deleting the window leaves
/// nothing to list.
#[tokio::test]
async fn the_listing_is_scoped_to_one_window_of_one_region() {
    let router = router();
    let case = "the window scope";
    let here = window(&router, case, REGION, "scoped-tasks").await;
    let elsewhere = window(&router, case, OTHER_REGION, "scoped-tasks").await;
    let registered = register_ok(
        &router,
        case,
        REGION,
        &json!({
            "WindowId": here,
            "TaskType": "RUN_COMMAND",
            "TaskArn": "AWS-RunPatchBaseline",
            "MaxConcurrency": "1",
            "MaxErrors": "1",
            "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
        }),
    )
    .await;

    let held = tasks_of(&router, case, REGION, &here).await;
    assert_eq!(
        ids_of(&held),
        vec![registered.clone()],
        "case {case:?}: expected the one task this window holds, got {held:?}"
    );
    // A window that holds no task is reported as holding none, rather than refused.
    let none = tasks_of(&router, case, OTHER_REGION, &elsewhere).await;
    assert!(
        none.is_empty(),
        "case {case:?}: expected the other Region's window to hold no task, got {none:?}"
    );

    // (case, the Region the listing is signed for, the window it names)
    let refused: Vec<(&str, &str, &str)> = vec![
        ("an id no create minted", REGION, UNKNOWN_WINDOW_ID),
        ("a window of another Region", REGION, elsewhere.as_str()),
        (
            "this Region's window at the other endpoint",
            OTHER_REGION,
            here.as_str(),
        ),
    ];
    for (why, region, window_id) in refused {
        let (status, response) = describe(&router, region, &json!({ "WindowId": window_id })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DoesNotExistException"),
            "case {why:?}: expected the listing to be refused, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Maintenance window {window_id} does not exist"),
            "case {why:?}: expected the refusal to name the window asked for, got {response}"
        );
    }

    // Deleting the window takes its tasks with it: a task is only ever addressed through the
    // window it belongs to.
    let (status, response) = call_in_region(
        &router,
        REGION,
        "DeleteMaintenanceWindow",
        &json!({ "WindowId": here }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete must be served, got {status} {response}"
    );
    let (status, response) = describe(&router, REGION, &json!({ "WindowId": here })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DoesNotExistException"),
        "case {case:?}: expected the deleted window to hold no tasks to list, got {status} \
         {response}"
    );
}
