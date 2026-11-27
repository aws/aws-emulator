// SPDX-License-Identifier: CC0-1.0

//! What `DeregisterTaskFromMaintenanceWindow` releases, beyond the task record itself.
//!
//! `all/maintenance_window_task_removal.rs` pins the removal and the teardown it unblocks.
//! The two rules here are the other side of the same removal — that it gives the window's
//! task store back the room the register took, and that it touches nothing else the window
//! holds:
//!
//! 1. **A deregistration releases the capacity its register consumed.** A window filled to
//!    the ceiling its task store is bounded by refuses another register; after `n` of its
//!    tasks are deregistered, exactly `n` further registers are served — each minting a
//!    FRESH id, never a removed one — and the one after them is refused again with the same
//!    wording. The rows differ along how many tasks the row removes before registering.
//! 2. **A removal is scoped to the one task it names.** The window itself still reads back
//!    with the schedule it was created with, the targets registered with it are still
//!    reported, and every task beside the removed one still reads back with its own members.
//!    The rows differ along which of the window's tasks is removed, including the one that
//!    references the registered target.
//!
//! The ceiling is read from the crate (`MAX_TASKS_PER_WINDOW`) rather than written here, so
//! these cases test the bound the handlers enforce and not a copy of it. Every call goes
//! through the front door a client uses — the shared harness in `all/parameter_store.rs`
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service
//! router — and every task a row removes was registered by a
//! `RegisterTaskWithMaintenanceWindow` of this suite and read back through this service's own
//! reads, so nothing here can pass on a response that is not computed from persisted state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// How many tasks one window's task store holds, read from the crate that enforces it.
const MAX_TASKS: usize =
    service_com_amazonaws_ssm_amazonssm::state::maintenance_window_tasks::MAX_TASKS_PER_WINDOW;

/// Create a window and answer the id it minted.
async fn window(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, response) = call_in_region(
        router,
        REGION,
        "CreateMaintenanceWindow",
        &json!({
            "Name": name,
            "Schedule": "cron(0 4 ? * MON *)",
            "Duration": 3,
            "Cutoff": 1,
            "AllowUnassociatedTargets": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the window must be created first, got {status} {response}"
    );
    response["WindowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create reports a WindowId, got {response}"))
        .to_string()
}

/// The body of one register of `window_id`, naming `targets` and carrying `name`.
fn registration(window_id: &str, name: &str, targets: Value) -> Value {
    json!({
        "WindowId": window_id,
        "TaskType": "RUN_COMMAND",
        "TaskArn": "AWS-RunShellScript",
        "MaxConcurrency": "1",
        "MaxErrors": "1",
        "Name": name,
        "Targets": targets,
    })
}

/// One `RegisterTaskWithMaintenanceWindow`, as `(status, body)`.
async fn register(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, "RegisterTaskWithMaintenanceWindow", body).await
}

/// One register that must be served, answering the `WindowTaskId` it minted.
async fn register_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = register(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the register to be served, got {status} {response}"
    );
    response["WindowTaskId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a register reports a WindowTaskId, got {response}")
        })
        .to_string()
}

/// `DeregisterTaskFromMaintenanceWindow`, asserting it is served and echoes the ids the
/// request carried.
async fn deregister_task_ok(
    router: &MultiServiceRouter,
    case: &str,
    window_id: &str,
    window_task_id: &str,
) {
    let (status, response) = call_in_region(
        router,
        REGION,
        "DeregisterTaskFromMaintenanceWindow",
        &json!({ "WindowId": window_id, "WindowTaskId": window_task_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the deregistration to be served, got {status} {response}"
    );
    assert_eq!(
        (
            response["WindowId"].as_str(),
            response["WindowTaskId"].as_str()
        ),
        (Some(window_id), Some(window_task_id)),
        "case {case:?}: expected the ids the request carried, got {response}"
    );
}

/// The `WindowTaskId`s `window_id` holds, as its own describe reports them.
async fn held_tasks(router: &MultiServiceRouter, case: &str, window_id: &str) -> Vec<String> {
    let (status, response) = call_in_region(
        router,
        REGION,
        "DescribeMaintenanceWindowTasks",
        &json!({ "WindowId": window_id, "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must be served, got {status} {response}"
    );
    response["Tasks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|held| held["WindowTaskId"].as_str().map(str::to_string))
        .collect()
}

/// Assert a register is refused by the ceiling, with the wording that refusal carries.
async fn refused_by_the_ceiling(
    router: &MultiServiceRouter,
    case: &str,
    window_id: &str,
    body: &Value,
) {
    let (status, response) = register(router, body).await;
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
}

/// RULE 1: the room a register took is given back by the deregistration of what it
/// registered — so a window at the ceiling admits exactly as many further registers as it
/// had tasks removed, and no more.
#[tokio::test]
async fn a_deregistration_releases_the_capacity_the_register_consumed() {
    let router = router();
    let case = "the capacity case";
    let window_id = window(&router, case, "capacity-window").await;

    // Fill the window to its ceiling. Each register names its own node, so no two of them
    // are the same registration.
    let mut held: Vec<String> = Vec::new();
    for nth in 0..MAX_TASKS {
        let body = registration(
            &window_id,
            &format!("filler-{nth}"),
            json!([{ "Key": "InstanceIds", "Values": [format!("i-{nth:017x}")] }]),
        );
        held.push(register_ok(&router, case, &body).await);
    }
    assert_eq!(
        held_tasks(&router, case, &window_id).await.len(),
        MAX_TASKS,
        "case {case:?}: expected the window to hold every task the ceiling admits"
    );

    // Every id this suite has ever seen for this window, so a later register minting one of
    // them again would be caught.
    let mut ever_minted = held.clone();

    // (case, how many of the window's tasks the row deregisters before registering again)
    let rows: [(&str, usize); 3] = [
        ("one task removed", 1),
        ("two tasks removed", 2),
        ("five tasks removed", 5),
    ];
    for (why, removals) in rows {
        refused_by_the_ceiling(&router, why, &window_id, &registration(
            &window_id,
            "over-the-ceiling",
            json!([{ "Key": "InstanceIds", "Values": ["i-00000000000000fff"] }]),
        ))
        .await;

        let removed: Vec<String> = held.drain(0..removals).collect();
        for window_task_id in &removed {
            deregister_task_ok(&router, why, &window_id, window_task_id).await;
        }
        assert_eq!(
            held_tasks(&router, why, &window_id).await.len(),
            MAX_TASKS - removals,
            "case {why:?}: expected the removals to leave {} tasks held",
            MAX_TASKS - removals
        );

        // Exactly as many registers as removals are served, and each mints an id no register
        // of this window has minted before: the removal released room, it did not resurrect
        // a record or hand back a retired id.
        for nth in 0..removals {
            let body = registration(
                &window_id,
                &format!("replacement-{nth}"),
                json!([{ "Key": "InstanceIds", "Values": [format!("i-1{nth:016x}")] }]),
            );
            let minted = register_ok(&router, why, &body).await;
            assert!(
                !ever_minted.contains(&minted),
                "case {why:?}: expected a fresh WindowTaskId, got the already-minted {minted:?}"
            );
            assert!(
                !removed.contains(&minted),
                "case {why:?}: expected a fresh WindowTaskId, got the removed {minted:?}"
            );
            ever_minted.push(minted.clone());
            held.push(minted);
        }

        let now_held = held_tasks(&router, why, &window_id).await;
        assert_eq!(
            now_held.len(),
            MAX_TASKS,
            "case {why:?}: expected the window to be back at the ceiling, holding {now_held:?}"
        );
        for window_task_id in &removed {
            assert!(
                !now_held.contains(window_task_id),
                "case {why:?}: expected the removed {window_task_id:?} to stay removed"
            );
        }
    }

    // And the window is bounded again at the end: the released room was finite.
    refused_by_the_ceiling(&router, case, &window_id, &registration(
        &window_id,
        "over-the-ceiling-again",
        json!([{ "Key": "InstanceIds", "Values": ["i-00000000000000eee"] }]),
    ))
    .await;
}

/// RULE 2: the removal is scoped to the one task the request names — the window, its
/// registered targets and its other tasks are all untouched, whichever task is removed.
#[tokio::test]
async fn a_removal_leaves_the_rest_of_the_window_standing() {
    let router = router();
    let case = "the scope case";
    let window_id = window(&router, case, "scope-window").await;

    // One registered target of the window, and three tasks: one naming that target, two
    // naming nodes directly.
    let (status, response) = call_in_region(
        &router,
        REGION,
        "RegisterTargetWithMaintenanceWindow",
        &json!({
            "WindowId": window_id,
            "ResourceType": "INSTANCE",
            "Targets": [{ "Key": "tag:Name", "Values": ["probe"] }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the target must be registered first, got {status} {response}"
    );
    let window_target_id = response["WindowTargetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a register reports a WindowTargetId, got {response}"))
        .to_string();

    // (the task's Name, the criteria it names)
    let registrations: [(&str, Value); 3] = [
        (
            "names-the-registered-target",
            json!([{ "Key": "WindowTargetIds", "Values": [window_target_id.clone()] }]),
        ),
        (
            "names-a-node",
            json!([{ "Key": "InstanceIds", "Values": ["i-00000000000000001"] }]),
        ),
        (
            "names-another-node",
            json!([{ "Key": "InstanceIds", "Values": ["i-00000000000000002"] }]),
        ),
    ];
    let mut tasks: Vec<(String, String)> = Vec::new();
    for (name, targets) in registrations {
        let body = registration(&window_id, name, targets);
        tasks.push((register_ok(&router, case, &body).await, name.to_string()));
    }

    // Each row removes one of the three by the name it was registered under, including the
    // one that references the target.
    let rows: [(&str, &str); 3] = [
        (
            "the task naming the registered target",
            "names-the-registered-target",
        ),
        ("the task registered last", "names-another-node"),
        ("the one task left", "names-a-node"),
    ];
    for (why, named) in rows {
        let nth = tasks
            .iter()
            .position(|(_, name)| name == named)
            .unwrap_or_else(|| panic!("case {why:?}: the window holds the task named {named:?}"));
        let (window_task_id, _) = tasks[nth].clone();
        deregister_task_ok(&router, why, &window_id, &window_task_id).await;
        tasks.remove(nth);

        // The window itself is untouched: it reads back with what the create gave it.
        let (status, read) = call_in_region(
            &router,
            REGION,
            "GetMaintenanceWindow",
            &json!({ "WindowId": window_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why:?}: expected the window to still be readable, got {status} {read}"
        );
        assert_eq!(
            (
                read["Name"].as_str(),
                read["Schedule"].as_str(),
                read["Duration"].as_i64(),
                read["Cutoff"].as_i64()
            ),
            (
                Some("scope-window"),
                Some("cron(0 4 ? * MON *)"),
                Some(3),
                Some(1)
            ),
            "case {why:?}: expected the window the create made, got {read}"
        );

        // The registered target is untouched, even by the removal of the task that
        // referenced it.
        let (status, targets) = call_in_region(
            &router,
            REGION,
            "DescribeMaintenanceWindowTargets",
            &json!({ "WindowId": window_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why:?}: expected the targets to still be readable, got {status} {targets}"
        );
        let reported: Vec<String> = targets["Targets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|held| held["WindowTargetId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            reported,
            vec![window_target_id.clone()],
            "case {why:?}: expected the registered target to stand, got {reported:?}"
        );

        // Every task beside the removed one still reads back with its own members.
        let mut expected: Vec<String> = tasks.iter().map(|(id, _)| id.clone()).collect();
        expected.sort();
        let mut now_held = held_tasks(&router, why, &window_id).await;
        now_held.sort();
        assert_eq!(
            now_held, expected,
            "case {why:?}: expected the tasks beside it to be untouched, got {now_held:?}"
        );
        for (other_id, name) in &tasks {
            let (status, read) = call_in_region(
                &router,
                REGION,
                "GetMaintenanceWindowTask",
                &json!({ "WindowId": window_id, "WindowTaskId": other_id }),
            )
            .await;
            assert_eq!(
                (status, read["Name"].as_str(), read["TaskArn"].as_str()),
                (200, Some(name.as_str()), Some("AWS-RunShellScript")),
                "case {why:?}: expected {other_id:?} to still read back as {name:?}, got \
                 {status} {read}"
            );
        }
    }

    // The three rows between them removed every task the window held.
    assert!(
        held_tasks(&router, case, &window_id).await.is_empty(),
        "case {case:?}: every registered task was removed"
    );
}
