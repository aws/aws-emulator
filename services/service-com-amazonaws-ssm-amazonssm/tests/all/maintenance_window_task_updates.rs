// SPDX-License-Identifier: CC0-1.0

//! Modifying a task already registered with a maintenance window:
//! `UpdateMaintenanceWindowTask` edits what `RegisterTaskWithMaintenanceWindow` persisted, and
//! `GetMaintenanceWindowTask`/`DescribeMaintenanceWindowTasks` report what the edit left
//! behind.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response that
//! is not computed from persisted state cannot pass: every read follows a write.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! 1. **An update is a PATCH of the stored task.** "If the value for a parameter in
//!    UpdateMaintenanceWindowTask is null, then the corresponding field isn't modified": every
//!    member the request supplies is applied WHOLESALE and every member it leaves out keeps the
//!    value the task holds, and what the update reports is what a later read reports.
//! 2. **A REPLACING update clears what it leaves out.** Under `Replace`, "optional fields that
//!    aren't specified are set to null" while the members
//!    `RegisterTaskWithMaintenanceWindow` requires are required here too — so a replacing
//!    update carrying no `TaskArn` is refused and writes nothing.
//! 3. **The two bounds follow the targets.** "For maintenance window tasks without a target
//!    specified, you can't supply a value for this option. Instead, the system inserts a
//!    placeholder value of 1": a task an update leaves targetless holds `1` for
//!    `MaxConcurrency` and `MaxErrors` whatever the request stated.
//! 4. **The task's identity does not move.** No input member carries the id, the window or the
//!    task type — "you can't change the task type" — so an update leaves all three as the
//!    register persisted them, and leaves the order a listing of the window's tasks walks them
//!    in.
//! 5. **Both ids are resolved against the Region's records.** A window this endpoint holds none
//!    of, and a task that is no task of the window named, are each refused
//!    `DoesNotExistException`, and a refused update writes nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::maintenance_window_task_service_role::service_linked_role_arn;
use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// A second Region, so a task is shown to be reachable only through the endpoint of the Region
/// its window lives in.
const OTHER_REGION: &str = "eu-central-1";

/// A `WindowId` of the form the modelled `MaintenanceWindowId` pattern requires
/// (`^mw-[0-9a-f]{17}$`) that no create has minted.
const UNKNOWN_WINDOW_ID: &str = "mw-00000000000000000";

/// A `WindowTaskId` of the form the modelled `MaintenanceWindowTaskId` pattern requires that no
/// register has minted.
const UNKNOWN_TASK_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The members a read reports only when a write set them, so a row that leaves one out is
/// checked to report nothing for it: the optional members of `GetMaintenanceWindowTaskResult`
/// (`smithy_query(query="operation",operation="GetMaintenanceWindowTask")`).
///
/// `ServiceRoleArn` and `TaskParameters` are NOT among them: the role a task runs as and the
/// parameter map it carries are materialised for every registered task, so an update reports
/// both whatever it stated (see `all/maintenance_window_task_service_role.rs`).
/// `CutoffBehavior` IS among them: its documented `CONTINUE_TASK` is how a task that chose
/// nothing behaves at the cutoff rather than a value a write stores, and
/// `MaintenanceWindowTask$CutoffBehavior` carries no modelled default.
const OPTIONAL_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "LoggingInfo",
    "AlarmConfiguration",
    "TaskInvocationParameters",
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

/// Register a task of `window_id` with `body`'s members and answer the id minted for it.
async fn register(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    window_id: &str,
    body: &Value,
) -> String {
    let mut request = body.clone();
    request["WindowId"] = Value::String(window_id.to_string());
    let (status, response) =
        call_in_region(router, region, "RegisterTaskWithMaintenanceWindow", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the register {request} must succeed, got {status} {response}"
    );
    response["WindowTaskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a register reports a WindowTaskId, got {response}"))
        .to_string()
}

/// `UpdateMaintenanceWindowTask` at `region`'s endpoint, for the task `window_task_id` of
/// `window_id`, carrying `body`'s members.
async fn update_for(
    router: &MultiServiceRouter,
    region: &str,
    window_id: &str,
    window_task_id: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = body.clone();
    request["WindowId"] = Value::String(window_id.to_string());
    request["WindowTaskId"] = Value::String(window_task_id.to_string());
    call_in_region(router, region, "UpdateMaintenanceWindowTask", &request).await
}

/// The task `window_task_id` names among `window_id`'s, read back through
/// `GetMaintenanceWindowTask`.
async fn task(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    window_id: &str,
    window_task_id: &str,
) -> Value {
    let (status, response) = call_in_region(
        router,
        region,
        "GetMaintenanceWindowTask",
        &json!({ "WindowId": window_id, "WindowTaskId": window_task_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read of {window_task_id:?} must succeed, got {status} {response}"
    );
    response
}

/// The tasks one window holds, in the order `DescribeMaintenanceWindowTasks` reports them.
async fn tasks_of(router: &MultiServiceRouter, case: &str, region: &str, window_id: &str) -> Vec<Value> {
    let (status, response) = call_in_region(
        router,
        region,
        "DescribeMaintenanceWindowTasks",
        &json!({ "WindowId": window_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe of {window_id:?}'s tasks must succeed, got {status} \
         {response}"
    );
    response["Tasks"].as_array().cloned().unwrap_or_default()
}

/// The `Priority` a response reports, which is `0` whether the member is serialized or left to
/// its modelled default.
fn reported_priority(reported: &Value) -> i64 {
    reported["Priority"].as_i64().unwrap_or(0)
}

/// Assert that `reported` carries every member `expected` names, and nothing for an optional
/// member `expected` leaves out.
fn assert_members(case: &str, reported: &Value, expected: &Value) {
    for (member, value) in expected.as_object().expect("the expected members") {
        if member == "Priority" {
            assert_eq!(
                reported_priority(reported),
                value.as_i64().expect("an expected Priority"),
                "case {case:?}: expected Priority {value}, got {reported}"
            );
            continue;
        }
        assert_eq!(
            &reported[member], value,
            "case {case:?}: expected {member} {value}, got {reported}"
        );
    }
    for member in OPTIONAL_MEMBERS {
        if expected.get(member).is_none() {
            assert!(
                reported.get(member).is_none_or(Value::is_null),
                "case {case:?}: expected nothing to be carried for {member}, got {reported}"
            );
        }
    }
}

/// The register every row of the patch and replacement tables starts from: a RUN_COMMAND task
/// carrying every member a register can state.
fn registered_task() -> Value {
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
        "TaskInvocationParameters": {
            "RunCommand": {
                "Comment": "the weekly run",
                "TimeoutSeconds": 600,
                "Parameters": { "Operation": ["Install"] },
            },
        },
    })
}

/// What a read of [`registered_task`] reports before any update touches it, which is every
/// member that register stated: it carries targets, so neither bound is the placeholder, and it
/// states the priority and the cutoff behaviour, so neither is the service's default.
fn registered_members() -> Value {
    registered_task()
}

/// RULE 1: an update is a patch of the stored task — a member supplied is applied, a member
/// left out keeps what the task holds — and what the update reports is what a later read
/// reports.
///
/// The rows differ along what the rule generalises over: which members the update supplies (the
/// ARN alone, the bounds and the priority, the labels, the criteria, the parameter map, the
/// invocation parameters, the logging and alarm members, nothing at all), which form the new
/// value takes, and whether the update states `Replace` as false or leaves it out.
#[tokio::test]
async fn an_update_applies_what_it_carries_and_keeps_what_it_omits() {
    let router = router();
    let case = "the patch table";
    let window_id = window(&router, case, REGION, "task-updates").await;

    // (case, the update's members, the members the task must carry afterwards)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the task ARN alone, which is the documented example of a change",
            json!({ "TaskArn": "AWS-RunShellScript" }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                "Name": "patch-the-fleet",
                "Description": "the weekly patch run",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/mw-task-role",
                "Priority": 3,
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "TaskParameters": { "Operation": { "Values": ["Install"] } },
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
                },
            }),
        ),
        (
            "the two bounds and the priority, as percentages",
            json!({ "MaxConcurrency": "50%", "MaxErrors": "10%", "Priority": 7 }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "Name": "patch-the-fleet",
                "Description": "the weekly patch run",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/mw-task-role",
                "Priority": 7,
                "MaxConcurrency": "50%",
                "MaxErrors": "10%",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "TaskParameters": { "Operation": { "Values": ["Install"] } },
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
                },
            }),
        ),
        (
            "both labels, stating Replace as false",
            json!({
                "Replace": false,
                "Name": "renamed-task",
                "Description": "the new description",
            }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "Name": "renamed-task",
                "Description": "the new description",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/mw-task-role",
                "Priority": 3,
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "TaskParameters": { "Operation": { "Values": ["Install"] } },
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
                },
            }),
        ),
        (
            "two tag criteria in place of the node ids, with the cutoff behaviour and the role",
            json!({
                "Targets": [
                    { "Key": "tag:Environment", "Values": ["production"] },
                    { "Key": "tag-key", "Values": ["Patch"] },
                ],
                "CutoffBehavior": "CONTINUE_TASK",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/another-task-role",
            }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "Name": "patch-the-fleet",
                "Description": "the weekly patch run",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/another-task-role",
                "Priority": 3,
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "CutoffBehavior": "CONTINUE_TASK",
                "Targets": [
                    { "Key": "tag:Environment", "Values": ["production"] },
                    { "Key": "tag-key", "Values": ["Patch"] },
                ],
                "TaskParameters": { "Operation": { "Values": ["Install"] } },
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
                },
            }),
        ),
        (
            "the parameter map alone",
            json!({ "TaskParameters": { "Operation": { "Values": ["Scan"] } } }),
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
                "TaskParameters": { "Operation": { "Values": ["Scan"] } },
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
                },
            }),
        ),
        (
            // "When you update a maintenance window task that has options specified in
            // TaskInvocationParameters, you must provide again all the
            // TaskInvocationParameters values that you want to retain. The values you don't
            // specify again are removed."
            "the invocation parameters, which stand for the whole of them",
            json!({
                "TaskInvocationParameters": {
                    "RunCommand": { "OutputS3BucketName": "another-bucket" },
                },
            }),
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
                "TaskInvocationParameters": {
                    "RunCommand": { "OutputS3BucketName": "another-bucket" },
                },
            }),
        ),
        (
            "the logging and alarm members the register left out",
            json!({
                "LoggingInfo": {
                    "S3BucketName": "maintenance-logs",
                    "S3KeyPrefix": "window/tasks",
                    "S3Region": "us-west-2",
                },
                "AlarmConfiguration": {
                    "IgnorePollAlarmFailure": true,
                    "Alarms": [{ "Name": "maintenance-window-alarm" }],
                },
            }),
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
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
                },
                "LoggingInfo": {
                    "S3BucketName": "maintenance-logs",
                    "S3KeyPrefix": "window/tasks",
                    "S3Region": "us-west-2",
                },
                "AlarmConfiguration": {
                    "IgnorePollAlarmFailure": true,
                    "Alarms": [{ "Name": "maintenance-window-alarm" }],
                },
            }),
        ),
        (
            "nothing at all, which modifies nothing",
            json!({}),
            registered_members(),
        ),
    ];

    for (why, body, expected) in rows {
        let id = register(&router, why, REGION, &window_id, &registered_task()).await;
        let (status, response) = update_for(&router, REGION, &window_id, &id, &body).await;
        assert_eq!(
            status, 200,
            "case {why:?}: the update {body} must be served, got {status} {response}"
        );
        assert_eq!(
            (
                response["WindowId"].as_str(),
                response["WindowTaskId"].as_str()
            ),
            (Some(window_id.as_str()), Some(id.as_str())),
            "case {why:?}: expected the update to report the ids it was asked about, got \
             {response}"
        );
        // The update's own result carries no task type, so the expectation is read against the
        // task the later get reports as well as against the result.
        let mut without_type = expected.clone();
        without_type
            .as_object_mut()
            .expect("the expected members")
            .remove("TaskType");
        assert_members(why, &response, &without_type);

        // The read back through the get is the same answer: the update persisted it.
        let read = task(&router, why, REGION, &window_id, &id).await;
        assert_members(why, &read, &expected);
    }
}

/// RULE 2: a replacing update clears every optional member it leaves out, and is refused when
/// it leaves out the `TaskArn` `RegisterTaskWithMaintenanceWindow` requires — a refusal that
/// writes nothing.
///
/// The rows differ along how much of the task the replacement restates: every member, the
/// required ARN with criteria alone, the required ARN alone.
#[tokio::test]
async fn a_replacing_update_clears_what_it_leaves_out() {
    let router = router();
    let case = "the replacement table";
    let window_id = window(&router, case, REGION, "task-replacements").await;

    // (case, the replacing update's members, the members the task must carry afterwards)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a replacement restating every member",
            json!({
                "Replace": true,
                "TaskArn": "AWS-RunShellScript",
                "Name": "replaced-task",
                "Description": "the replacing description",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/replacing-role",
                "Priority": 5,
                "MaxConcurrency": "3",
                "MaxErrors": "2",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "tag:Team", "Values": ["payments"] }],
                "TaskParameters": { "Operation": { "Values": ["Scan"] } },
                "TaskInvocationParameters": {
                    "RunCommand": { "Comment": "the replacing run" },
                },
            }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                "Name": "replaced-task",
                "Description": "the replacing description",
                "ServiceRoleArn": "arn:aws:iam::123456789012:role/replacing-role",
                "Priority": 5,
                "MaxConcurrency": "3",
                "MaxErrors": "2",
                "CutoffBehavior": "CANCEL_TASK",
                "Targets": [{ "Key": "tag:Team", "Values": ["payments"] }],
                "TaskParameters": { "Operation": { "Values": ["Scan"] } },
                "TaskInvocationParameters": {
                    "RunCommand": { "Comment": "the replacing run" },
                },
            }),
        ),
        (
            "a replacement restating the required ARN and the criteria alone",
            json!({
                "Replace": true,
                "TaskArn": "AWS-RunShellScript",
                "Targets": [{ "Key": "tag:Team", "Values": ["payments"] }],
            }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                // Everything else the register stated is cleared to what a register that named
                // it nothing would have persisted: the modelled default for the priority, the
                // account's service-linked role for the role the task runs as, the empty map
                // for the parameters, and nothing at all for the rest — the cutoff behaviour
                // included.
                "Priority": 0,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [{ "Key": "tag:Team", "Values": ["payments"] }],
            }),
        ),
        (
            "a replacement restating the required ARN alone",
            json!({ "Replace": true, "TaskArn": "AWS-RunShellScript" }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                "Priority": 0,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                // Cleared to the empty list the modelled length admits, which leaves the task
                // targetless — so both bounds hold the documented placeholder.
                "Targets": [],
                "MaxConcurrency": "1",
                "MaxErrors": "1",
            }),
        ),
    ];

    for (why, body, expected) in rows {
        let id = register(&router, why, REGION, &window_id, &registered_task()).await;
        let (status, response) = update_for(&router, REGION, &window_id, &id, &body).await;
        assert_eq!(
            status, 200,
            "case {why:?}: the replacing update {body} must be served, got {status} {response}"
        );
        let mut without_type = expected.clone();
        without_type
            .as_object_mut()
            .expect("the expected members")
            .remove("TaskType");
        assert_members(why, &response, &without_type);
        let read = task(&router, why, REGION, &window_id, &id).await;
        assert_members(why, &read, &expected);
    }

    // The ARN is required of a replacing update, because the register requires it.
    let why = "a replacement restating no ARN at all";
    let id = register(&router, why, REGION, &window_id, &registered_task()).await;
    let (status, response) =
        update_for(&router, REGION, &window_id, &id, &json!({ "Replace": true })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {why:?}: expected a refusal, got {status} {response}"
    );
    assert!(
        error_message(&response).contains("'taskArn'"),
        "case {why:?}: expected the refusal to name the member it wanted, got {response}"
    );
    let read = task(&router, why, REGION, &window_id, &id).await;
    assert_members(why, &read, &registered_members());
}

/// RULE 3: the two bounds follow the targets the task now has — a task an update leaves
/// targetless holds the documented placeholder `1` for both whatever the request stated, and a
/// task given targets again holds what the request states.
///
/// The rows differ along how the task is left targetless or targeted and along what the request
/// states for the bounds.
#[tokio::test]
async fn the_bounds_follow_the_targets_the_task_is_left_with() {
    let router = router();
    let case = "the bounds table";
    let window_id = window(&router, case, REGION, "task-bounds").await;

    // (case, the update's members, what MaxConcurrency and MaxErrors must read afterwards)
    let rows: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "a patch emptying the criteria while stating both bounds",
            json!({ "Targets": [], "MaxConcurrency": "5", "MaxErrors": "4" }),
            Some("1"),
            Some("1"),
        ),
        (
            "a patch emptying the criteria and stating no bound",
            json!({ "Targets": [] }),
            Some("1"),
            Some("1"),
        ),
        (
            "a replacement carrying no criteria and no bound",
            json!({ "Replace": true, "TaskArn": "AWS-RunShellScript" }),
            Some("1"),
            Some("1"),
        ),
        (
            "a patch restating the criteria and both bounds",
            json!({
                "Targets": [{ "Key": "tag-key", "Values": ["Patch"] }],
                "MaxConcurrency": "10%",
                "MaxErrors": "0",
            }),
            Some("10%"),
            Some("0"),
        ),
        (
            "a replacement carrying criteria but no bound, which clears them",
            json!({
                "Replace": true,
                "TaskArn": "AWS-RunShellScript",
                "Targets": [{ "Key": "tag-key", "Values": ["Patch"] }],
            }),
            None,
            None,
        ),
    ];

    for (why, body, concurrency, errors) in rows {
        let id = register(&router, why, REGION, &window_id, &registered_task()).await;
        let (status, response) = update_for(&router, REGION, &window_id, &id, &body).await;
        assert_eq!(
            status, 200,
            "case {why:?}: the update {body} must be served, got {status} {response}"
        );
        for reported in [&response, &task(&router, why, REGION, &window_id, &id).await] {
            assert_eq!(
                reported["MaxConcurrency"].as_str(),
                concurrency,
                "case {why:?}: expected MaxConcurrency {concurrency:?}, got {reported}"
            );
            assert_eq!(
                reported["MaxErrors"].as_str(),
                errors,
                "case {why:?}: expected MaxErrors {errors:?}, got {reported}"
            );
        }
    }
}

/// RULE 4: the task's identity does not move — an update leaves its minted id, its window and
/// its task type as the register persisted them, leaves the other tasks of that window alone,
/// and leaves the order a listing walks them in.
///
/// The rows differ along which task type the register stated and which members the update
/// carries.
#[tokio::test]
async fn an_update_leaves_the_tasks_identity_and_the_listing_order_alone() {
    let router = router();
    let case = "the identity case";
    let window_id = window(&router, case, REGION, "task-identity").await;

    // (case, the task type the register states, the ARN it states, the update's members)
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a RUN_COMMAND task renamed",
            "RUN_COMMAND",
            "AWS-RunPatchBaseline",
            json!({ "Name": "renamed-run-command" }),
        ),
        (
            "an AUTOMATION task replaced",
            "AUTOMATION",
            "AWS-RestartEC2Instance",
            json!({
                "Replace": true,
                "TaskArn": "AWS-StopEC2Instance",
                "Targets": [{ "Key": "tag:Team", "Values": ["payments"] }],
            }),
        ),
        (
            "a LAMBDA task given a priority",
            "LAMBDA",
            "arn:aws:lambda:us-west-2:123456789012:function:maintenance",
            json!({ "Priority": 9 }),
        ),
    ];

    let mut registered: Vec<(&str, String, &str)> = Vec::new();
    for (why, task_type, task_arn, _) in &rows {
        let id = register(
            &router,
            why,
            REGION,
            &window_id,
            &json!({
                "TaskType": task_type,
                "TaskArn": task_arn,
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "MaxConcurrency": "1",
                "MaxErrors": "1",
            }),
        )
        .await;
        registered.push((*why, id, *task_type));
    }

    for (position, (why, id, task_type)) in registered.iter().enumerate() {
        let body = &rows[position].3;
        let (status, response) = update_for(&router, REGION, &window_id, id, body).await;
        assert_eq!(
            status, 200,
            "case {why:?}: the update must be served, got {status} {response}"
        );
        let read = task(&router, why, REGION, &window_id, id).await;
        assert_eq!(
            read["TaskType"].as_str(),
            Some(*task_type),
            "case {why:?}: expected the task type the register persisted, got {read}"
        );
        assert_eq!(
            (read["WindowId"].as_str(), read["WindowTaskId"].as_str()),
            (Some(window_id.as_str()), Some(id.as_str())),
            "case {why:?}: expected the ids the register persisted, got {read}"
        );
    }

    let held = tasks_of(&router, case, REGION, &window_id).await;
    let walked: Vec<&str> = held
        .iter()
        .map(|task| task["WindowTaskId"].as_str().unwrap_or_default())
        .collect();
    let minted: Vec<&str> = registered.iter().map(|(_, id, _)| id.as_str()).collect();
    assert_eq!(
        walked, minted,
        "case {case:?}: expected the updates to have minted no task and to have left the order \
         they were registered in"
    );
}

/// RULE 5: both ids are resolved against the Region's records — a window this endpoint holds
/// none of, and a task that is no task of the window named, are each refused, and a refused
/// update writes nothing.
#[tokio::test]
async fn an_update_resolves_both_ids_against_the_regions_records() {
    let router = router();
    let case = "the refusal table";
    let here = window(&router, case, REGION, "task-refusals").await;
    let elsewhere = window(&router, case, OTHER_REGION, "task-refusals").await;
    let other_window = window(&router, case, REGION, "another-window").await;
    let id = register(&router, case, REGION, &here, &registered_task()).await;
    let elsewhere_id = register(
        &router,
        case,
        OTHER_REGION,
        &elsewhere,
        &registered_task(),
    )
    .await;

    // (case, the Region the update is signed for, the window it names, the task it names, the
    //  message the refusal must carry)
    let rows: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "a window id no create minted",
            REGION,
            UNKNOWN_WINDOW_ID,
            id.as_str(),
            format!("Maintenance window {UNKNOWN_WINDOW_ID} does not exist"),
        ),
        (
            "a window of another Region",
            REGION,
            elsewhere.as_str(),
            elsewhere_id.as_str(),
            format!("Maintenance window {elsewhere} does not exist"),
        ),
        (
            "a task id no register minted",
            REGION,
            here.as_str(),
            UNKNOWN_TASK_ID,
            format!("Maintenance window {here} has no task {UNKNOWN_TASK_ID}"),
        ),
        (
            "a task of another window of this Region",
            REGION,
            other_window.as_str(),
            id.as_str(),
            format!("Maintenance window {other_window} has no task {id}"),
        ),
        (
            "this Region's task at the other endpoint",
            OTHER_REGION,
            here.as_str(),
            id.as_str(),
            format!("Maintenance window {here} does not exist"),
        ),
    ];

    for (why, region, window_id, window_task_id, message) in rows {
        let (status, response) = update_for(
            &router,
            region,
            window_id,
            window_task_id,
            &json!({ "Name": "a-name-nothing-must-carry" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DoesNotExistException"),
            "case {why:?}: expected the update to be refused, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            message,
            "case {why:?}: expected the refusal to name what was asked for, got {response}"
        );
    }

    // None of the refused updates touched the task they named.
    let read = task(&router, case, REGION, &here, &id).await;
    assert_members(case, &read, &registered_members());
}
