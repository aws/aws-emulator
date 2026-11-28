// SPDX-License-Identifier: CC0-1.0

//! What a 200 READ puts in a record's modeled members: exactly what the write that created it
//! persisted — no member the write set left out, and no member it never set filled in with a
//! value the service invented.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response that
//! is not computed from persisted state cannot pass: every read follows a write.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! 1. **An optional member a write never set is reported by no read of the record.** A member
//!    whose documented default states how the resource BEHAVES — "CONTINUE_TASK: When the
//!    cutoff time is reached, any tasks that are running continue. The default value"
//!    (`smithy_query(query="doc",shape="RegisterTaskWithMaintenanceWindowRequest",member="CutoffBehavior")`)
//!    — is not a value the write stores: `MaintenanceWindowTask$CutoffBehavior` carries no
//!    modelled default, so both reads of a window task (the single read and the window's
//!    listing) report it only for a write that chose it. The members the service DOES
//!    materialise for every registered task — the role it runs as and its parameter map — are
//!    reported by the same reads whatever the write stated.
//! 2. **A member a write DID set is reported as it was written.** A parameter is listed under
//!    the name it was created with, and the `ARN` of that same record is built from that name,
//!    so the two members of one record cannot disagree about what the parameter is called.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::maintenance_window_task_service_role::service_linked_role_arn;
use crate::parameter_store::{call, call_in_region, router, ACCOUNT, REGION};

/// Create a maintenance window at [`REGION`]'s endpoint and answer the id it minted.
async fn window(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, response) = call(
        router,
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
        "case {case:?}: the window must be created, got {status} {response}"
    );
    response["WindowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create reports a WindowId, got {response}"))
        .to_string()
}

/// Register `body`'s task with `window_id` and answer the `WindowTaskId` minted for it.
async fn register(
    router: &MultiServiceRouter,
    case: &str,
    window_id: &str,
    body: &Value,
) -> String {
    let mut request = body.clone();
    request["WindowId"] = Value::String(window_id.to_string());
    let (status, response) = call(router, "RegisterTaskWithMaintenanceWindow", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the register {request} must succeed, got {status} {response}"
    );
    response["WindowTaskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a register reports a WindowTaskId, got {response}"))
        .to_string()
}

/// Register a target with `window_id` and answer the `WindowTargetId` it minted — the
/// criterion form a task names a registered target of its own window by
/// (`Key=WindowTargetIds,Values=`).
async fn target(router: &MultiServiceRouter, case: &str, window_id: &str) -> String {
    let (status, response) = call(
        router,
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

/// The single read of one task, asserting it was served.
async fn read_task(
    router: &MultiServiceRouter,
    case: &str,
    window_id: &str,
    window_task_id: &str,
) -> Value {
    let (status, response) = call(
        router,
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

/// The entry the window's LISTING reports for one task, asserting the listing was served and
/// holds it.
async fn listed_task(
    router: &MultiServiceRouter,
    case: &str,
    window_id: &str,
    window_task_id: &str,
) -> Value {
    let (status, response) = call(
        router,
        "DescribeMaintenanceWindowTasks",
        &json!({ "WindowId": window_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the listing of {window_id:?}'s tasks must succeed, got {status} \
         {response}"
    );
    response["Tasks"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|task| task["WindowTaskId"].as_str() == Some(window_task_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing holds the task {window_task_id:?}, got {response}")
        })
}

/// RULE 1: neither read of a window task reports a `CutoffBehavior` the write never chose, and
/// both report the role the task runs as and its parameter map whatever the write stated.
///
/// The rows differ along what the rule generalises over: whether the write states the member at
/// all, which of the two values the modelled enum admits it states, which task type and which
/// target form the task carries, and whether the write that settles the member is the register
/// or a later update (a patch that sets it, a replacing update that restates nothing).
#[tokio::test]
async fn no_read_of_a_window_task_reports_a_cutoff_behaviour_nothing_chose() {
    let router = router();
    let case = "the cutoff projection table";
    let window_id = window(&router, case, "cutoff-projection").await;
    let window_target_id = target(&router, case, &window_id).await;

    // (case, the register's own members, the update to apply afterwards, the CutoffBehavior
    // both reads must then report)
    let rows: Vec<(&str, Value, Option<Value>, Option<&str>)> = vec![
        (
            "a RUN_COMMAND register stating no cutoff behaviour",
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunShellScript",
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "Targets": [{ "Key": "WindowTargetIds", "Values": [window_target_id] }],
            }),
            None,
            None,
        ),
        (
            "a targetless LAMBDA register stating no cutoff behaviour",
            json!({
                "TaskType": "LAMBDA",
                "TaskArn": "arn:aws:lambda:us-west-2:123456789012:function:projection",
                "Targets": [],
            }),
            None,
            None,
        ),
        (
            "an AUTOMATION register stating the documented cutoff behaviour",
            json!({
                "TaskType": "AUTOMATION",
                "TaskArn": "AWS-RestartEC2Instance",
                "MaxConcurrency": "50%",
                "MaxErrors": "0",
                "Targets": [{ "Key": "tag:Environment", "Values": ["production"] }],
                "CutoffBehavior": "CONTINUE_TASK",
            }),
            None,
            Some("CONTINUE_TASK"),
        ),
        (
            "a STEP_FUNCTIONS register stating the other value the enum admits",
            json!({
                "TaskType": "STEP_FUNCTIONS",
                "TaskArn": "arn:aws:states:us-west-2:123456789012:stateMachine:projection",
                "MaxConcurrency": "1",
                "MaxErrors": "1",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
                "CutoffBehavior": "CANCEL_TASK",
            }),
            None,
            Some("CANCEL_TASK"),
        ),
        (
            "a register stating none, then a patch that sets it",
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "Targets": [{ "Key": "tag-key", "Values": ["Patch"] }],
            }),
            Some(json!({ "CutoffBehavior": "CANCEL_TASK" })),
            Some("CANCEL_TASK"),
        ),
        (
            "a register stating it, then a replacing update restating nothing of it",
            json!({
                "TaskType": "RUN_COMMAND",
                "TaskArn": "AWS-RunPatchBaseline",
                "MaxConcurrency": "2",
                "MaxErrors": "1",
                "Targets": [{ "Key": "tag-key", "Values": ["Patch"] }],
                "CutoffBehavior": "CANCEL_TASK",
            }),
            Some(json!({ "Replace": true, "TaskArn": "AWS-RunShellScript" })),
            None,
        ),
    ];

    for (why, body, update, expected) in rows {
        let window_task_id = register(&router, why, &window_id, &body).await;
        if let Some(update) = update {
            let mut request = update.clone();
            request["WindowId"] = Value::String(window_id.clone());
            request["WindowTaskId"] = Value::String(window_task_id.clone());
            let (status, answered) = call(&router, "UpdateMaintenanceWindowTask", &request).await;
            assert_eq!(
                status, 200,
                "case {why:?}: the update {request} must be served, got {status} {answered}"
            );
            // The update's own answer is a projection of the same record, so it reports the
            // member the same way.
            assert_eq!(
                answered["CutoffBehavior"].as_str(),
                expected,
                "case {why:?}: expected the update to answer CutoffBehavior {expected:?}, got \
                 {answered}"
            );
        }

        // Both reads of the task — the single read and the window's listing — project the one
        // record, so each reports the member exactly as the writes left it.
        for (operation, reported) in [
            (
                "GetMaintenanceWindowTask",
                read_task(&router, why, &window_id, &window_task_id).await,
            ),
            (
                "DescribeMaintenanceWindowTasks",
                listed_task(&router, why, &window_id, &window_task_id).await,
            ),
        ] {
            assert_eq!(
                reported["CutoffBehavior"].as_str(),
                expected,
                "case {why:?}: expected {operation} to report CutoffBehavior {expected:?}, got \
                 {reported}"
            );
            if expected.is_none() {
                assert!(
                    reported.get("CutoffBehavior").is_none_or(Value::is_null),
                    "case {why:?}: expected {operation} to carry no CutoffBehavior member at \
                     all, got {reported}"
                );
            }
            // The two members the service materialises for every registered task are reported
            // by the same reads: the role the task runs as and its parameter map.
            assert_eq!(
                reported["ServiceRoleArn"].as_str(),
                Some(service_linked_role_arn().as_str()),
                "case {why:?}: expected {operation} to report the account's service-linked \
                 role, got {reported}"
            );
            assert_eq!(
                reported["TaskParameters"],
                json!({}),
                "case {why:?}: expected {operation} to report the empty parameter map, got \
                 {reported}"
            );
        }
    }
}

/// RULE 2: the metadata listing reports a parameter under the name it was created with, and the
/// `ARN` of that same record is built from that name.
///
/// The rows differ along what the rule generalises over: how many levels the created name has
/// and therefore whether it names a hierarchy at all, which of the symbols the name rule admits
/// it carries ("Parameter names can include only the following symbols and letters:
/// a-zA-Z0-9_.-" plus the `/` separator,
/// `smithy_query(query="doc",shape="PutParameterRequest",member="Name")`), the parameter's type,
/// and how many versions and labels it holds when the listing reads it.
#[tokio::test]
async fn the_metadata_listing_reports_the_name_the_parameter_was_created_with() {
    let router = router();

    // (case, the name a put created, the parameter type, how many versions to write, whether a
    // version is labelled and then unlabelled before the listing)
    let rows: Vec<(&str, &str, &str, usize, bool)> = vec![
        ("an unqualified name", "ProjectedName", "String", 1, false),
        (
            "an unqualified name, overwritten and label-churned",
            "ProjectedChurn",
            "String",
            2,
            true,
        ),
        (
            "an unqualified name of the symbols the name rule admits",
            "Projected_name-1.0",
            "StringList",
            1,
            false,
        ),
        (
            "a two-level name, which names a hierarchy",
            "/Projected/two-levels",
            "String",
            2,
            false,
        ),
        (
            "a five-level name",
            "/Projected/a/b/c/five",
            "StringList",
            1,
            true,
        ),
    ];

    for (why, name, parameter_type, versions, churn_a_label) in rows {
        for version in 0..versions {
            let (status, response) = call(
                &router,
                "PutParameter",
                &json!({
                    "Name": name,
                    "Type": parameter_type,
                    "Value": format!("value-{version}"),
                    "Overwrite": version > 0,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {why:?}: writing {name:?} must succeed, got {status} {response}"
            );
        }
        if churn_a_label {
            for operation in ["LabelParameterVersion", "UnlabelParameterVersion"] {
                let body = json!({
                    "Name": name,
                    "ParameterVersion": 1,
                    "Labels": ["projected-label"],
                });
                let (status, response) = call(&router, operation, &body).await;
                assert_eq!(
                    status, 200,
                    "case {why:?}: {operation} on {name:?} must be served, got {status} \
                     {response}"
                );
            }
        }

        let (status, listed) = call_in_region(
            &router,
            REGION,
            "DescribeParameters",
            &json!({ "ParameterFilters": [{ "Key": "Name", "Values": [name] }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why:?}: the listing of {name:?} must be served, got {status} {listed}"
        );
        let records = listed["Parameters"]
            .as_array()
            .unwrap_or_else(|| panic!("case {why:?}: the listing carries Parameters, got {listed}"));
        assert_eq!(
            records.len(),
            1,
            "case {why:?}: the filter names exactly this parameter, got {listed}"
        );
        let record = &records[0];
        // The ARN carries the name after its `parameter/` prefix, with the leading separator a
        // hierarchy's name starts with dropped, so the two members of one record agree about
        // what the parameter is called.
        let expected_arn = format!(
            "arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/{}",
            name.strip_prefix('/').unwrap_or(name)
        );
        assert_eq!(
            (record["Name"].as_str(), record["ARN"].as_str()),
            (Some(name), Some(expected_arn.as_str())),
            "case {why:?}: expected the listing to report the created name beside the ARN built \
             from it, got {listed}"
        );
    }
}
