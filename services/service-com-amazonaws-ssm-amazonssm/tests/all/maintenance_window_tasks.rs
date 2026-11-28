// SPDX-License-Identifier: CC0-1.0

//! The TASKS of a maintenance window end to end: `RegisterTaskWithMaintenanceWindow` adds
//! one, `GetMaintenanceWindowTask` reports it, `DeleteMaintenanceWindow` takes the window's
//! tasks with it, and a task naming a registered target of the same window is what stops
//! `DeregisterTargetFromMaintenanceWindow` from orphaning that target.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router. Nothing reaches
//! into the state struct, so a response that is not computed from persisted state cannot
//! pass: every read follows a write.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! 1. **A read reports what the register persisted.** For the task the request names, the get
//!    reports exactly the members the register supplied, under the id the service minted, and
//!    nothing for a member the register left out.
//! 2. **Two members are the SERVICE's, not the request's.** A task registered without
//!    targets holds the documented placeholder `1` for `MaxConcurrency` and `MaxErrors`
//!    whatever the register stated, and one carrying no `Priority` holds the modelled
//!    default `0` — while a register carrying no `CutoffBehavior` stores none, so a read
//!    reports nothing for it.
//! 3. **A task belongs to ONE window of ONE Region.** A register naming a window this
//!    endpoint holds none of is refused `DoesNotExistException` and writes nothing, a task of
//!    one window is no task of another, and deleting the window takes its tasks.
//! 4. **A retry is not a second task.** One `ClientToken` reports the task the first register
//!    minted; the same token carrying different members is refused
//!    `IdempotentParameterMismatch`.
//! 5. **A target a task still names is not removed.** `DeregisterTargetFromMaintenanceWindow`
//!    refuses `TargetInUseException` naming the task that references the target, unless the
//!    request explicitly states `Safe=false`.
//! 6. **The store is bounded.** Past the ceiling one window's tasks are held under, a register
//!    is refused `ResourceLimitExceededException` and writes nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::maintenance_window_task_service_role::service_linked_role_arn;
use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// A second Region, so a window and its tasks are shown to live at the endpoint of ONE
/// Region.
const OTHER_REGION: &str = "eu-central-1";

/// A `WindowId` of the form the modelled `MaintenanceWindowId` pattern requires
/// (`^mw-[0-9a-f]{17}$`) that no create has minted.
const UNKNOWN_WINDOW_ID: &str = "mw-00000000000000000";

/// A `WindowTaskId` of the form the modelled `MaintenanceWindowTaskId` pattern requires that
/// no register has minted.
const UNKNOWN_TASK_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The optional members a get reports only when the register supplied them, so a row that
/// leaves one out is checked to report nothing for it.
///
/// `ServiceRoleArn` and `TaskParameters` are NOT among them: the role a task runs as and the
/// parameter map it carries are materialised for every registered task, so a read reports both
/// whatever the register stated (see `all/maintenance_window_task_service_role.rs`).
/// `CutoffBehavior` IS among them: its documented `CONTINUE_TASK` is how a task that chose
/// nothing behaves at the cutoff rather than a value the register stores, and
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

/// `RegisterTaskWithMaintenanceWindow` at `region`'s endpoint with `body`.
async fn register(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "RegisterTaskWithMaintenanceWindow", body).await
}

/// The same write, asserting it succeeded, and answering the `WindowTaskId` it minted.
async fn register_ok(router: &MultiServiceRouter, case: &str, region: &str, body: &Value) -> String {
    let (status, response) = register(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the register {body} must succeed, got {status} {response}"
    );
    let id = response["WindowTaskId"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: a register reports a WindowTaskId, got {response}")
    });
    assert!(
        is_task_id(id),
        "case {case:?}: expected the 8-4-4-4-12 hexadecimal the model fixes, got {id:?}"
    );
    id.to_string()
}

/// `GetMaintenanceWindowTask` at `region`'s endpoint.
async fn get(
    router: &MultiServiceRouter,
    region: &str,
    window_id: &str,
    window_task_id: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GetMaintenanceWindowTask",
        &json!({ "WindowId": window_id, "WindowTaskId": window_task_id }),
    )
    .await
}

/// The same read, asserting it succeeded.
async fn get_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    window_id: &str,
    window_task_id: &str,
) -> Value {
    let (status, response) = get(router, region, window_id, window_task_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read of {window_task_id:?} must succeed, got {status} {response}"
    );
    response
}

/// `DeregisterTargetFromMaintenanceWindow` with `body`.
async fn deregister_target(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DeregisterTargetFromMaintenanceWindow",
        body,
    )
    .await
}

/// Register a target with `window_id` and answer the `WindowTargetId` it minted.
async fn target(router: &MultiServiceRouter, case: &str, window_id: &str) -> String {
    let (status, response) = call_in_region(
        router,
        REGION,
        "RegisterTargetWithMaintenanceWindow",
        &json!({
            "WindowId": window_id,
            "ResourceType": "INSTANCE",
            "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the target this case's task references must be registered, got {status} \
         {response}"
    );
    response["WindowTargetId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a register reports a WindowTargetId, got {response}")
        })
        .to_string()
}

/// The ids of the targets `window_id` holds, as `DescribeMaintenanceWindowTargets` reports
/// them.
async fn target_ids_of(router: &MultiServiceRouter, case: &str, window_id: &str) -> Vec<String> {
    let (status, response) = call_in_region(
        router,
        REGION,
        "DescribeMaintenanceWindowTargets",
        &json!({ "WindowId": window_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe of {window_id:?}'s targets must succeed, got {status} \
         {response}"
    );
    response["Targets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|target| {
            target["WindowTargetId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Whether `id` is shaped like the `MaintenanceWindowTaskId` the model fixes: the 8-4-4-4-12
/// hexadecimal of its own pattern.
fn is_task_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width && group.chars().all(|digit| digit.is_ascii_hexdigit())
            })
}

/// The `Priority` a read reports, which is `0` whether the member is serialized or left to
/// its modelled default.
fn reported_priority(reported: &Value) -> i64 {
    reported["Priority"].as_i64().unwrap_or(0)
}

/// Assert that `reported` carries every member of `expected`, and nothing for an optional
/// member `expected` leaves out.
fn assert_reports(case: &str, reported: &Value, expected: &Value) {
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
                "case {case:?}: expected no {member} for a register that carried none, got \
                 {reported}"
            );
        }
    }
}

/// RULE 1: the get reports, for the task the request names, exactly the members its register
/// supplied — under the id the service minted — and nothing for a member the register left
/// out.
///
/// The rows differ along what the rule generalises over: which `MaintenanceWindowTaskType` the
/// register states (every one of the four the modelled enum admits), which form its targets
/// take (node ids, a registered target of the window, none at all), which invocation
/// parameters match that type, and which of the optional labels, parameters, logging and alarm
/// members it carries.
#[tokio::test]
async fn a_read_reports_what_the_register_persisted() {
    let router = router();
    let case = "the register table";
    let window_id = window(&router, case, REGION, "task-table").await;
    let window_target_id = target(&router, case, &window_id).await;

    // (case, the register body, the members the read must report)
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
                "TaskInvocationParameters": {
                    "RunCommand": {
                        "Comment": "the weekly run",
                        "TimeoutSeconds": 600,
                        "Parameters": { "Operation": ["Install"] },
                    },
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
            }),
        ),
        (
            "an AUTOMATION task targeting a registered target of the window",
            json!({
                "TaskType": "AUTOMATION",
                "TaskArn": "AWS-RestartEC2Instance",
                "MaxConcurrency": "50%",
                "MaxErrors": "0",
                "Targets": [{ "Key": "WindowTargetIds", "Values": [window_target_id] }],
                "TaskInvocationParameters": {
                    "Automation": { "DocumentVersion": "$DEFAULT" },
                },
            }),
            json!({
                "TaskType": "AUTOMATION",
                "TaskArn": "AWS-RestartEC2Instance",
                "MaxConcurrency": "50%",
                "MaxErrors": "0",
                // The register carried no Priority, so the modelled default stands, and no
                // CutoffBehavior, so nothing is reported for it; it named no ServiceRoleArn
                // and no TaskParameters, so the task runs as the account's service-linked
                // role and holds the empty map.
                "Priority": 0,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [{ "Key": "WindowTargetIds", "Values": [window_target_id] }],
                "TaskInvocationParameters": {
                    "Automation": { "DocumentVersion": "$DEFAULT" },
                },
            }),
        ),
        (
            "a LAMBDA task with no targets at all",
            json!({
                "TaskType": "LAMBDA",
                "TaskArn": "arn:aws:lambda:us-west-2:123456789012:function:maintenance",
                "Priority": 10,
                "TaskInvocationParameters": {
                    "Lambda": { "Qualifier": "$LATEST" },
                },
            }),
            json!({
                "TaskType": "LAMBDA",
                "TaskArn": "arn:aws:lambda:us-west-2:123456789012:function:maintenance",
                "Priority": 10,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [],
                // A targetless task holds the documented placeholder for both bounds.
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "TaskInvocationParameters": {
                    "Lambda": { "Qualifier": "$LATEST" },
                },
            }),
        ),
        (
            "a STEP_FUNCTIONS task with logging and an alarm",
            json!({
                "TaskType": "STEP_FUNCTIONS",
                "TaskArn": "arn:aws:states:us-west-2:123456789012:stateMachine:maintenance",
                "MaxConcurrency": "100%",
                "MaxErrors": "100%",
                "Targets": [{ "Key": "tag:Environment", "Values": ["production"] }],
                "LoggingInfo": {
                    "S3BucketName": "maintenance-logs",
                    "S3KeyPrefix": "window/tasks",
                    "S3Region": "us-west-2",
                },
                "AlarmConfiguration": {
                    "IgnorePollAlarmFailure": true,
                    "Alarms": [{ "Name": "maintenance-window-alarm" }],
                },
                "TaskInvocationParameters": {
                    "StepFunctions": { "Name": "maintenance-run" },
                },
            }),
            json!({
                "TaskType": "STEP_FUNCTIONS",
                "TaskArn": "arn:aws:states:us-west-2:123456789012:stateMachine:maintenance",
                "MaxConcurrency": "100%",
                "MaxErrors": "100%",
                "Priority": 0,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [{ "Key": "tag:Environment", "Values": ["production"] }],
                "LoggingInfo": {
                    "S3BucketName": "maintenance-logs",
                    "S3KeyPrefix": "window/tasks",
                    "S3Region": "us-west-2",
                },
                "AlarmConfiguration": {
                    "IgnorePollAlarmFailure": true,
                    "Alarms": [{ "Name": "maintenance-window-alarm" }],
                },
                "TaskInvocationParameters": {
                    "StepFunctions": { "Name": "maintenance-run" },
                },
            }),
        ),
        (
            "a RUN_COMMAND task carrying nothing optional",
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "Targets": [{ "Key": "tag-key", "Values": ["Patch"] }],
            }),
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "Priority": 0,
                "ServiceRoleArn": service_linked_role_arn(),
                "TaskParameters": {},
                "Targets": [{ "Key": "tag-key", "Values": ["Patch"] }],
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (why, mut body, expected) in rows {
        body["WindowId"] = Value::String(window_id.clone());
        let window_task_id = register_ok(&router, why, REGION, &body).await;
        assert!(
            !minted.contains(&window_task_id),
            "case {why:?}: expected an id no other register minted, got {window_task_id:?}"
        );
        let reported = get_ok(&router, why, REGION, &window_id, &window_task_id).await;
        assert_eq!(
            (
                reported["WindowId"].as_str(),
                reported["WindowTaskId"].as_str()
            ),
            (Some(window_id.as_str()), Some(window_task_id.as_str())),
            "case {why:?}: expected the read to report the ids it was addressed by, got \
             {reported}"
        );
        assert_reports(why, &reported, &expected);
        minted.push(window_task_id);
    }
}

/// RULE 2: a task registered without targets holds the documented placeholder `1` for both
/// bounds whatever the register stated, while a targeted task holds the bounds it stated.
///
/// The rows differ along what the rule generalises over: whether the register names no targets
/// member at all or an empty list, whether it states bounds alongside, and which form a stated
/// bound takes (a count, a percentage, the zero the `MaxErrors` pattern admits).
#[tokio::test]
async fn the_bounds_of_a_targetless_task_are_the_documented_placeholder() {
    let router = router();
    let case = "the bounds table";
    let window_id = window(&router, case, REGION, "bounds-window").await;

    // (case, the targets the register states, the bounds it states, the bounds the read
    // reports)
    let rows: Vec<(&str, Value, Value, (&str, &str))> = vec![
        (
            "no targets member and no bounds",
            Value::Null,
            json!({}),
            ("1", "1"),
        ),
        (
            "no targets member, bounds stated anyway",
            Value::Null,
            json!({ "MaxConcurrency": "25", "MaxErrors": "5" }),
            ("1", "1"),
        ),
        (
            "an empty targets list, bounds stated anyway",
            json!([]),
            json!({ "MaxConcurrency": "10%", "MaxErrors": "0" }),
            ("1", "1"),
        ),
        (
            "targets and a count",
            json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
            json!({ "MaxConcurrency": "7", "MaxErrors": "3" }),
            ("7", "3"),
        ),
        (
            "targets and a percentage",
            json!([{ "Key": "tag:Team", "Values": ["payments"] }]),
            json!({ "MaxConcurrency": "100%", "MaxErrors": "0%" }),
            ("100%", "0%"),
        ),
        (
            "targets, the smallest percentage and the zero MaxErrors admits",
            json!([{ "Key": "tag:Team", "Values": ["platform"] }]),
            json!({ "MaxConcurrency": "1%", "MaxErrors": "0" }),
            ("1%", "0"),
        ),
    ];

    for (why, targets, bounds, (concurrency, errors)) in rows {
        let mut body = json!({
            "WindowId": window_id,
            "TaskType": "RUN_COMMAND",
            "TaskArn": "AWS-RunShellScript",
        });
        if !targets.is_null() {
            body["Targets"] = targets;
        }
        for (member, value) in bounds.as_object().expect("the stated bounds") {
            body[member] = value.clone();
        }
        let window_task_id = register_ok(&router, why, REGION, &body).await;
        let reported = get_ok(&router, why, REGION, &window_id, &window_task_id).await;
        assert_eq!(
            (
                reported["MaxConcurrency"].as_str(),
                reported["MaxErrors"].as_str()
            ),
            (Some(concurrency), Some(errors)),
            "case {why:?}: expected MaxConcurrency {concurrency:?} and MaxErrors {errors:?}, \
             got {reported}"
        );
    }
}

/// RULE 2 (the other members the service chooses): `Priority` holds the modelled default when
/// the register carried none — every priority the modelled range admits is held as stated —
/// while `CutoffBehavior` is held exactly as the register stated it and reported not at all
/// when it stated none.
#[tokio::test]
async fn the_priority_default_is_the_modelled_one_and_the_cutoff_behaviour_is_held_as_stated() {
    let router = router();
    let case = "the defaults table";
    let window_id = window(&router, case, REGION, "defaults-window").await;

    // (case, the CutoffBehavior stated, the Priority stated, what the read reports)
    struct Row {
        why: &'static str,
        cutoff: Option<&'static str>,
        priority: Option<i64>,
        expected_cutoff: Option<&'static str>,
        expected_priority: i64,
    }
    let rows: Vec<Row> = vec![
        Row {
            why: "neither member stated",
            cutoff: None,
            priority: None,
            expected_cutoff: None,
            expected_priority: 0,
        },
        Row {
            why: "the documented cutoff behaviour stated explicitly",
            cutoff: Some("CONTINUE_TASK"),
            priority: Some(0),
            expected_cutoff: Some("CONTINUE_TASK"),
            expected_priority: 0,
        },
        Row {
            why: "the other cutoff behaviour the enum admits",
            cutoff: Some("CANCEL_TASK"),
            priority: Some(1),
            expected_cutoff: Some("CANCEL_TASK"),
            expected_priority: 1,
        },
        Row {
            why: "a priority past the single digits",
            cutoff: None,
            priority: Some(42),
            expected_cutoff: None,
            expected_priority: 42,
        },
        Row {
            why: "the lowest priority this emulator is asked for",
            cutoff: Some("CANCEL_TASK"),
            priority: Some(1_000),
            expected_cutoff: Some("CANCEL_TASK"),
            expected_priority: 1_000,
        },
    ];

    for row in rows {
        let why = row.why;
        let mut body = json!({
            "WindowId": window_id,
            "TaskType": "RUN_COMMAND",
            "TaskArn": "AWS-RunShellScript",
            "MaxConcurrency": "1",
            "MaxErrors": "1",
            "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
        });
        if let Some(cutoff) = row.cutoff {
            body["CutoffBehavior"] = Value::String(cutoff.to_string());
        }
        if let Some(priority) = row.priority {
            body["Priority"] = Value::Number(priority.into());
        }
        let window_task_id = register_ok(&router, why, REGION, &body).await;
        let reported = get_ok(&router, why, REGION, &window_id, &window_task_id).await;
        assert_eq!(
            (
                reported["CutoffBehavior"].as_str(),
                reported_priority(&reported)
            ),
            (row.expected_cutoff, row.expected_priority),
            "case {why:?}: expected CutoffBehavior {:?} and Priority {}, got {reported}",
            row.expected_cutoff,
            row.expected_priority
        );
    }
}

/// RULE 3: a task belongs to one window of one Region — a register naming a window this
/// endpoint holds none of is refused and writes nothing, a task of one window is no task of
/// another, and deleting the window takes its tasks with it.
#[tokio::test]
async fn a_task_belongs_to_one_window_of_one_region() {
    let router = router();
    let case = "the window scope";
    let here = window(&router, case, REGION, "scoped-task-window").await;
    let also_here = window(&router, case, REGION, "second-task-window").await;
    let elsewhere = window(&router, case, OTHER_REGION, "scoped-task-window").await;
    let body = |window_id: &str| {
        json!({
            "WindowId": window_id,
            "TaskType": "RUN_COMMAND",
            "TaskArn": "AWS-RunShellScript",
            "MaxConcurrency": "1",
            "MaxErrors": "1",
            "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
        })
    };
    let window_task_id = register_ok(&router, case, REGION, &body(&here)).await;

    // (case, the Region the register is signed for, the window it names)
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
        let (status, response) = register(&router, region, &body(window_id)).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DoesNotExistException"),
            "case {why:?}: expected the register to be refused, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Maintenance window {window_id} does not exist"),
            "case {why:?}: expected the refusal to name the window asked for, got {response}"
        );
    }

    // (case, the Region the read is signed for, the window it names, the task it names)
    let unreadable: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a task id nothing minted",
            REGION,
            here.as_str(),
            UNKNOWN_TASK_ID,
        ),
        (
            "this task addressed through another window of the Region",
            REGION,
            also_here.as_str(),
            window_task_id.as_str(),
        ),
        (
            "this task at the other Region's endpoint",
            OTHER_REGION,
            elsewhere.as_str(),
            window_task_id.as_str(),
        ),
        (
            "a window this endpoint holds none of",
            REGION,
            UNKNOWN_WINDOW_ID,
            window_task_id.as_str(),
        ),
    ];
    for (why, region, window_id, task_id) in unreadable {
        let (status, response) = get(&router, region, window_id, task_id).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DoesNotExistException"),
            "case {why:?}: expected the read to be refused, got {status} {response}"
        );
    }

    // The task the one successful register minted is still there: nothing the refusals touched
    // was written.
    let reported = get_ok(&router, case, REGION, &here, &window_task_id).await;
    assert_eq!(
        reported["TaskArn"].as_str(),
        Some("AWS-RunShellScript"),
        "case {case:?}: expected the one task this window holds, got {reported}"
    );

    // Deleting the window takes its tasks: a task is only ever addressed through its window.
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
    let (status, response) = get(&router, REGION, &here, &window_task_id).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DoesNotExistException"),
        "case {case:?}: expected the deleted window to hold no task to read, got {status} \
         {response}"
    );
    // A window created again under the same name holds none of the deleted window's tasks.
    let again = window(&router, case, REGION, "scoped-task-window").await;
    let (status, response) = get(&router, REGION, &again, &window_task_id).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DoesNotExistException"),
        "case {case:?}: expected a new window to hold no task of the deleted one, got {status} \
         {response}"
    );
    assert_eq!(
        error_message(&response),
        format!("Maintenance window {again} has no task {window_task_id}"),
        "case {case:?}: expected the refusal to name the window and the task asked for, got \
         {response}"
    );
}

/// RULE 4: "user-provided idempotency token" — a retry of one register reports the task that
/// register already minted, and the same token carrying different members is another call
/// rather than a retry.
#[tokio::test]
async fn an_idempotency_token_makes_a_retried_register_the_same_task() {
    let router = router();
    let case = "the idempotency case";
    let window_id = window(&router, case, REGION, "token-task-window").await;
    let body = json!({
        "WindowId": window_id,
        "TaskType": "RUN_COMMAND",
        "TaskArn": "AWS-RunShellScript",
        "ClientToken": "one-task-token",
        "MaxConcurrency": "1",
        "MaxErrors": "1",
        "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
    });
    let first = register_ok(&router, case, REGION, &body).await;
    let retried = register_ok(&router, case, REGION, &body).await;
    assert_eq!(
        retried, first,
        "case {case:?}: expected the retry to report the task the first register minted"
    );

    // (case, the member the second call changes, its value)
    let mismatches: Vec<(&str, &str, Value)> = vec![
        ("another task ARN", "TaskArn", json!("AWS-RunPatchBaseline")),
        ("another priority", "Priority", json!(5)),
        (
            "another cutoff behaviour",
            "CutoffBehavior",
            json!("CANCEL_TASK"),
        ),
        (
            "other targets",
            "Targets",
            json!([{ "Key": "InstanceIds", "Values": ["i-00000000000000001"] }]),
        ),
        (
            "task parameters the first call did not carry",
            "TaskParameters",
            json!({ "Operation": { "Values": ["Scan"] } }),
        ),
    ];
    for (why, member, value) in mismatches {
        let mut different = body.clone();
        different[member] = value;
        let (status, response) = register(&router, REGION, &different).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "IdempotentParameterMismatch"),
            "case {why:?}: expected one token carrying different members to be refused, got \
             {status} {response}"
        );
    }

    // None of the refused retries minted anything: the one task is still readable and still
    // carries what the first register stated.
    let reported = get_ok(&router, case, REGION, &window_id, &first).await;
    assert_eq!(
        reported["TaskArn"].as_str(),
        Some("AWS-RunShellScript"),
        "case {case:?}: expected the task the first register minted, got {reported}"
    );
}

/// RULE 5: a target a task of the same window still names by id is not removed — the
/// deregister is refused `TargetInUseException` naming the task that references it, unless the
/// request explicitly states `Safe=false`.
///
/// The rows differ along what the rule generalises over: whether the request asks for the
/// check at all, and whether the task's criteria name the target by id or name nodes.
#[tokio::test]
async fn a_target_a_task_references_is_not_deregistered() {
    let router = router();
    let case = "the interlock case";
    let window_id = window(&router, case, REGION, "interlock-window").await;
    let referenced = target(&router, case, &window_id).await;
    let untouched = target(&router, case, &window_id).await;
    let window_task_id = register_ok(
        &router,
        case,
        REGION,
        &json!({
            "WindowId": window_id,
            "TaskType": "RUN_COMMAND",
            "TaskArn": "AWS-RunShellScript",
            "MaxConcurrency": "1",
            "MaxErrors": "1",
            "Targets": [{ "Key": "WindowTargetIds", "Values": [referenced] }],
        }),
    )
    .await;

    // (case, the deregister body, whether the removal goes through)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the safety check not asked for at all",
            json!({ "WindowId": window_id, "WindowTargetId": referenced }),
            false,
        ),
        (
            "the safety check asked for",
            json!({ "WindowId": window_id, "WindowTargetId": referenced, "Safe": true }),
            false,
        ),
        (
            "a target no task of the window names",
            json!({ "WindowId": window_id, "WindowTargetId": untouched }),
            true,
        ),
        (
            "the safety check waived",
            json!({ "WindowId": window_id, "WindowTargetId": referenced, "Safe": false }),
            true,
        ),
    ];
    for (why, body, removed) in rows {
        let (status, response) = deregister_target(&router, REGION, &body).await;
        if removed {
            assert_eq!(
                status, 200,
                "case {why:?}: expected the deregister to be served, got {status} {response}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "TargetInUseException"),
            "case {why:?}: expected the deregister to be refused, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!(
                "This Target cannot be deregistered because it is still referenced in Task: \
                 {window_task_id}"
            ),
            "case {why:?}: expected the refusal to name the referencing task, got {response}"
        );
        // The refused deregister wrote nothing: the window still holds the target.
        let held = target_ids_of(&router, why, &window_id).await;
        assert!(
            held.contains(&referenced),
            "case {why:?}: expected the refused target to still be registered, got {held:?}"
        );
    }

    // The two rows that removed a target removed exactly those two: the window now holds
    // neither.
    let held = target_ids_of(&router, case, &window_id).await;
    assert!(
        !held.contains(&referenced) && !held.contains(&untouched),
        "case {case:?}: expected both deregistered targets to be gone, got {held:?}"
    );

    // A task whose criteria name NODES references no registered target, so a safe deregister
    // of one goes through.
    let other_window = window(&router, case, REGION, "node-task-window").await;
    let node_target = target(&router, case, &other_window).await;
    register_ok(
        &router,
        case,
        REGION,
        &json!({
            "WindowId": other_window,
            "TaskType": "RUN_COMMAND",
            "TaskArn": "AWS-RunShellScript",
            "MaxConcurrency": "1",
            "MaxErrors": "1",
            "Targets": [{ "Key": "InstanceIds", "Values": [node_target] }],
        }),
    )
    .await;
    let (status, response) = deregister_target(
        &router,
        REGION,
        &json!({ "WindowId": other_window, "WindowTargetId": node_target }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a task naming nodes rather than a registered target\": expected the deregister \
         to be served, got {status} {response}"
    );
}

/// RULE 6: the store is bounded — past the ceiling one window's tasks are held under, a
/// register is refused `ResourceLimitExceededException`, names the emulator's own ceiling and
/// writes nothing.
///
/// The ceiling is read from the crate rather than written here, so the case tests the bound
/// the handler enforces and not a copy of it.
#[tokio::test]
async fn a_register_past_the_ceiling_is_refused() {
    const MAX_TASKS: usize =
        service_com_amazonaws_ssm_amazonssm::state::maintenance_window_tasks::MAX_TASKS_PER_WINDOW;

    let router = router();
    let case = "the ceiling case";
    let window_id = window(&router, case, REGION, "ceiling-window").await;
    let body = json!({
        "WindowId": window_id,
        "TaskType": "RUN_COMMAND",
        "TaskArn": "AWS-RunShellScript",
        "MaxConcurrency": "1",
        "MaxErrors": "1",
        "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
    });
    let mut minted: Vec<String> = Vec::new();
    for index in 0..MAX_TASKS {
        minted.push(register_ok(&router, case, REGION, &body).await);
        assert_eq!(
            minted.len(),
            index + 1,
            "case {case:?}: expected each register under the ceiling to mint a task"
        );
    }

    let (status, response) = register(&router, REGION, &body).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceLimitExceededException"),
        "case {case:?}: expected the register past the ceiling to be refused, got {status} \
         {response}"
    );
    assert_eq!(
        error_message(&response),
        format!(
            "This emulator holds at most {MAX_TASKS} tasks for the maintenance window {window_id}"
        ),
        "case {case:?}: expected the refusal to name the ceiling and the window, got {response}"
    );

    // The refused register wrote nothing: every task the ceiling admitted is still readable,
    // and the refusal left no further record behind.
    for (position, window_task_id) in minted.iter().enumerate() {
        let reported = get_ok(&router, case, REGION, &window_id, window_task_id).await;
        assert_eq!(
            reported["WindowTaskId"].as_str(),
            Some(window_task_id.as_str()),
            "case {case:?}: expected the task registered at position {position} to still be \
             held, got {reported}"
        );
    }

    // A second window of the same Region is bounded on its own: the ceiling is per window.
    let other = window(&router, case, REGION, "other-ceiling-window").await;
    let mut next = body.clone();
    next["WindowId"] = Value::String(other.clone());
    let elsewhere = register_ok(&router, case, REGION, &next).await;
    let reported = get_ok(&router, case, REGION, &other, &elsewhere).await;
    assert_eq!(
        reported["WindowId"].as_str(),
        Some(other.as_str()),
        "case {case:?}: expected the other window to hold its own task, got {reported}"
    );
}
