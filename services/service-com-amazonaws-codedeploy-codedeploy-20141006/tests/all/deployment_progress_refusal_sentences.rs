// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE the two refusals about a deployment's PROGRESS and TYPE carry,
//! driven over the wire.
//!
//! RULES under test:
//!
//! * `DeploymentAlreadyCompletedException` says
//!   `The deployment <deploymentId> is already in state <STATE>` — the id the
//!   request named and the deployment's INTERNAL state name, the upper-snake form
//!   of its `DeploymentStatus`, in which a deployment a caller stopped is
//!   `ABORTED` rather than `Stopped`. Rows span the three outcomes a deployment
//!   reaches, how it reached each one (the clock, or a caller stopping it), the
//!   compute platform, the deployment type and every operation of this service
//!   that raises the error on it. A second `StopDeployment` of a deployment a
//!   stop ALREADY ended is not one of them: it asks for the state that deployment
//!   is already in, so it is answered rather than refused (`stop_deployment`
//!   holds that rule).
//! * `UnsupportedActionForDeploymentTypeException` says
//!   `Operation is not supported for the deployment <deploymentId> of type <TYPE>.`
//!   — the id and the `DeploymentType` the deployment runs as, so a caller
//!   learns which type was rejected. Rows span the ways a group ends up in-place
//!   (no style at all, an explicit one), the compute platform, and both
//!   operations that act only on a blue/green deployment.
//!
//! The harness (`Emulator`, `error_code`, `s3_revision`, `terminal_deployment`)
//! is the lifecycle suite's, so every case goes through the same front door a
//! client uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state::deployment_internal_state;

use crate::codedeploy_lifecycle::{error_code, s3_revision, terminal_deployment, Emulator};

/// The service role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// Create an application and one deployment group on it carrying `style` and
/// `blue_green` when those are not null, answering the group name.
async fn seeded_group(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    style: &Value,
    blue_green: &Value,
) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": SERVICE_ROLE,
    });
    if !style.is_null() {
        request["deploymentStyle"] = style.clone();
    }
    if !blue_green.is_null() {
        request["blueGreenDeploymentConfiguration"] = blue_green.clone();
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// Create a deployment on `application`/`group`, answering its id.
async fn deploy(emulator: &Emulator, application: &str, group: &str) -> String {
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": s3_revision("artifacts", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `$.message` of a failure body.
fn message(body: &Value, case: &str) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Call `operation`, require HTTP 400 with `code`, and answer the message.
async fn refused(
    emulator: &Emulator,
    operation: &str,
    request: Value,
    code: &str,
    case: &str,
) -> String {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code} from {operation}, got {body:?}"
    );
    message(&body, case)
}

/// The stored `deploymentInfo` of `deployment_id`.
async fn deployment_info(emulator: &Emulator, deployment_id: &str) -> Value {
    emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone()
}

/// A deployment driven to one of the three outcomes, and the operations that are
/// refused on it for having completed.
struct Completed {
    case: &'static str,
    platform: &'static str,
    /// The group's `deploymentStyle`, absent when null.
    style: Value,
    /// The group's `blueGreenDeploymentConfiguration`, absent when null.
    blue_green: Value,
    /// Whether a caller STOPS the deployment rather than letting the clock
    /// settle it.
    stopped_by_caller: bool,
    /// The `DeploymentStatus` the deployment ends in.
    status: &'static str,
    /// The INTERNAL state name the refusal reports for that status.
    state: &'static str,
    /// The operations refused with `DeploymentAlreadyCompletedException` on it.
    /// `ContinueDeployment` refuses an in-place deployment for its TYPE before
    /// it looks at progress, so it only appears on a blue/green row; a second
    /// `StopDeployment` only appears on a row whose outcome is not a stop,
    /// because a stop of an already-stopped deployment is answered.
    operations: &'static [&'static str],
}

/// The `deploymentStyle` of a blue/green deployment group.
fn blue_green_style() -> Value {
    json!({"deploymentType": "BLUE_GREEN", "deploymentOption": "WITH_TRAFFIC_CONTROL"})
}

/// RULE: `DeploymentAlreadyCompletedException` says
/// `The deployment <deploymentId> is already in state <STATE>` — the deployment
/// id the request carried and the deployment's internal state name, which for a
/// stopped deployment is `ABORTED` and not its API status — whichever operation
/// was refused and however the deployment reached its outcome.
#[tokio::test]
async fn a_completed_deployment_is_refused_with_its_internal_state() {
    // Rows span every outcome a deployment reaches (Succeeded, Failed, Stopped),
    // how it got there (the clock settling it, a caller stopping it), the
    // compute platform, the deployment type and the operations refused on it.
    let rows: &[Completed] = &[
        Completed {
            case: "stopped in-place server",
            platform: "Server",
            style: Value::Null,
            blue_green: Value::Null,
            stopped_by_caller: true,
            status: "Stopped",
            state: "ABORTED",
            operations: &["SkipWaitTimeForInstanceTermination"],
        },
        Completed {
            case: "stopped blue/green server",
            platform: "Server",
            style: blue_green_style(),
            blue_green: json!({
                "terminateBlueInstancesOnDeploymentSuccess": {
                    "action": "TERMINATE",
                    "terminationWaitTimeInMinutes": 2880,
                }
            }),
            stopped_by_caller: true,
            status: "Stopped",
            state: "ABORTED",
            operations: &["SkipWaitTimeForInstanceTermination", "ContinueDeployment"],
        },
        Completed {
            // An EC2/on-premises group resolves no target here, so the clock
            // settles the deployment as Failed.
            case: "clock-failed in-place server",
            platform: "Server",
            style: Value::Null,
            blue_green: Value::Null,
            stopped_by_caller: false,
            status: "Failed",
            state: "FAILED",
            operations: &["SkipWaitTimeForInstanceTermination", "StopDeployment"],
        },
        Completed {
            case: "clock-failed blue/green server",
            platform: "Server",
            style: blue_green_style(),
            blue_green: json!({
                "terminateBlueInstancesOnDeploymentSuccess": {
                    "action": "KEEP_ALIVE",
                    "terminationWaitTimeInMinutes": 0,
                }
            }),
            stopped_by_caller: false,
            status: "Failed",
            state: "FAILED",
            operations: &[
                "SkipWaitTimeForInstanceTermination",
                "ContinueDeployment",
                "StopDeployment",
            ],
        },
        Completed {
            // A Lambda group's target is the function its revision names, so the
            // clock settles the deployment as Succeeded.
            case: "clock-succeeded in-place lambda",
            platform: "Lambda",
            style: Value::Null,
            blue_green: Value::Null,
            stopped_by_caller: false,
            status: "Succeeded",
            state: "SUCCEEDED",
            operations: &["SkipWaitTimeForInstanceTermination", "StopDeployment"],
        },
        Completed {
            case: "clock-succeeded blue/green lambda",
            platform: "Lambda",
            style: blue_green_style(),
            blue_green: Value::Null,
            stopped_by_caller: false,
            status: "Succeeded",
            state: "SUCCEEDED",
            operations: &[
                "SkipWaitTimeForInstanceTermination",
                "ContinueDeployment",
                "StopDeployment",
            ],
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let application = row.case.replace([' ', '/'], "-");
        let group =
            seeded_group(&emulator, &application, row.platform, &row.style, &row.blue_green).await;
        let deployment_id = deploy(&emulator, &application, &group).await;

        if row.stopped_by_caller {
            emulator
                .ok("StopDeployment", json!({"deploymentId": deployment_id}))
                .await;
        } else {
            terminal_deployment(&emulator, &deployment_id, row.case).await;
        }
        let before = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            before["status"].as_str(),
            Some(row.status),
            "case {:?}: expected the deployment to hold {} before the refusal, got {before:?}",
            row.case,
            row.status
        );

        let expected = format!(
            "The deployment {deployment_id} is already in state {}",
            row.state
        );
        for operation in row.operations {
            let case = format!("{}/{operation}", row.case);
            let actual = refused(
                &emulator,
                operation,
                json!({"deploymentId": deployment_id}),
                "DeploymentAlreadyCompletedException",
                &case,
            )
            .await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            // The INTERNAL state is what the sentence reports: a stopped
            // deployment is ABORTED there, so its API status must not appear.
            if row.state != row.status {
                assert!(
                    !actual.contains(row.status),
                    "case {case:?}: the sentence must report the internal state {:?} rather than \
                     the API status {:?}, got {actual:?}",
                    row.state,
                    row.status
                );
            }
        }

        let after = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            after, before,
            "case {:?}: expected the refusals to leave the record untouched, got {after:?}",
            row.case
        );
    }
}

/// RULE: `UnsupportedActionForDeploymentTypeException` says
/// `Operation is not supported for the deployment <deploymentId> of type <TYPE>.`
/// — the deployment id the request carried and the `DeploymentType` the
/// deployment runs as — for every operation that acts only on a blue/green
/// deployment.
#[tokio::test]
async fn an_action_outside_a_deployments_type_is_refused_with_that_type() {
    // Rows span the ways a deployment ends up in-place — CodeDeploy's default
    // when a group carries no `deploymentStyle`, and an explicit one, under both
    // `deploymentOption` values — and the compute platform it runs on. IN_PLACE
    // and BLUE_GREEN are the whole `DeploymentType` catalogue
    // (`smithy_query(query="shape",shape="DeploymentType")`), and a blue/green
    // deployment is the one these operations serve, so IN_PLACE is the type this
    // refusal reports.
    let rows: &[(&str, &str, Value)] = &[
        ("no deployment style at all", "Server", Value::Null),
        (
            "an explicit in-place style without traffic control",
            "Server",
            json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITHOUT_TRAFFIC_CONTROL"}),
        ),
        (
            "an explicit in-place style with traffic control",
            "Server",
            json!({"deploymentType": "IN_PLACE", "deploymentOption": "WITH_TRAFFIC_CONTROL"}),
        ),
        ("a lambda deployment with no style", "Lambda", Value::Null),
    ];

    for (case, platform, style) in rows {
        let emulator = Emulator::new();
        let application = case.replace([' ', '/'], "-");
        let group = seeded_group(&emulator, &application, platform, style, &Value::Null).await;
        let deployment_id = deploy(&emulator, &application, &group).await;
        let expected = format!(
            "Operation is not supported for the deployment {deployment_id} of type IN_PLACE."
        );

        // Both operations that act only on a blue/green deployment. The wait-skip
        // judges the deployment's PROGRESS first, so it is called here while the
        // deployment the clock has only just created is still in flight.
        for operation in ["SkipWaitTimeForInstanceTermination", "ContinueDeployment"] {
            let case = format!("{case}/{operation}");
            let actual = refused(
                &emulator,
                operation,
                json!({"deploymentId": deployment_id}),
                "UnsupportedActionForDeploymentTypeException",
                &case,
            )
            .await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // The refusals changed nothing: the deployment still reports its type.
        let info = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            info["deploymentStyle"]["deploymentType"].as_str(),
            Some("IN_PLACE"),
            "case {case:?}: expected the deployment to be untouched, got {info:?}"
        );
    }
}

/// RULE: the internal state name of a deployment status is the status in upper
/// snake case, with the one state whose internal name differs from its API
/// status — a deployment a caller stopped is `ABORTED` — and that is the token
/// every refusal about a deployment's progress reports.
#[tokio::test]
async fn every_deployment_status_has_its_internal_state_name() {
    // The catalogue is the model's `DeploymentStatus` enum
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
    // InProgress | Baking | Succeeded | Failed | Stopped | Ready), including the
    // statuses no operation of this emulator drives a deployment into.
    let cases: &[(&str, &str)] = &[
        ("Created", "CREATED"),
        ("Queued", "QUEUED"),
        ("InProgress", "IN_PROGRESS"),
        ("Baking", "BAKING"),
        ("Succeeded", "SUCCEEDED"),
        ("Failed", "FAILED"),
        ("Stopped", "ABORTED"),
        ("Ready", "READY"),
    ];

    for (status, expected) in cases {
        let actual = deployment_internal_state(status);
        assert_eq!(
            actual, *expected,
            "case {status:?}: expected {expected:?}, got {actual:?}"
        );
    }
}
