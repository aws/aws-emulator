// SPDX-License-Identifier: CC0-1.0

//! `StopDeployment`: stopping a deployment that is still in flight, refusing one
//! that already reached an outcome, and the rollback a caller can ask for.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness in `codedeploy_lifecycle` — so the awsJson1_1 envelope,
//! the boundary validator and this service's refusal wording are all exercised.

use std::time::Duration;

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{
    deployment_id_on_free_group, error_code, s3_revision, seeded_group, terminal_deployment,
    Emulator,
};

/// Create a deployment of `revision` on `application`/`group`, answering its id.
///
/// A deployment group runs one deployment at a time, so a suite that deploys to
/// one group twice waits for the first to be terminal, exactly as a client would.
async fn create_deployment(
    emulator: &Emulator,
    application: &str,
    group: &str,
    revision: &Value,
) -> String {
    deployment_id_on_free_group(
        emulator,
        json!({
            "applicationName": application,
            "deploymentGroupName": group,
            "revision": revision,
        }),
    )
    .await
}

/// The stored `deploymentInfo` of `deployment_id`.
async fn deployment_info(emulator: &Emulator, deployment_id: &str) -> Value {
    emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await["deploymentInfo"]
        .clone()
}

/// Poll `deployment_id` until the clock has given it its terminal status,
/// requiring that status to be `expected`, and answer its stored record.
///
/// The poll itself is the shared [`terminal_deployment`] of `codedeploy_lifecycle`;
/// `expected` is the terminal status the deployment's group implies — a group
/// that resolves at least one deployment target ends `Succeeded`, one that
/// resolves none ends `Failed` (see `codedeploy_deployment_outcome`).
async fn awaited_terminal(
    emulator: &Emulator,
    case: &str,
    deployment_id: &str,
    expected: &str,
) -> Value {
    let info = terminal_deployment(emulator, deployment_id, case).await;
    assert_eq!(
        info["status"].as_str(),
        Some(expected),
        "case {case:?}: expected the terminal status {expected}, got {info:?}"
    );
    info
}

/// RULE: stopping a deployment that has not reached an outcome yet takes effect
/// at the instant of the call — the stored deployment holds the terminal
/// `Stopped` status with a completion time from then on, and the clock never
/// moves it again — and the call answers the `StopStatus` for a stop that has
/// already taken effect.
#[tokio::test]
async fn stopping_an_in_flight_deployment_makes_it_terminal() {
    // Rows differ along the compute platform the deployment runs on, the form of
    // the revision it deploys (each member the model's `RevisionLocation`
    // carries) and how the request spells "no rollback": absent, or false.
    let cases: &[(&str, &str, Value, Option<bool>)] = &[
        (
            "server-s3",
            "Server",
            s3_revision("stop-bucket", "app.zip"),
            None,
        ),
        (
            "lambda-string",
            "Lambda",
            json!({"revisionType": "String", "string": {"content": "version: 0.0", "sha256": "aa"}}),
            Some(false),
        ),
        (
            "ecs-appspec",
            "ECS",
            json!({"revisionType": "AppSpecContent", "appSpecContent": {"content": "version: 0.0", "sha256": "bb"}}),
            Some(false),
        ),
        (
            "server-github",
            "Server",
            json!({"revisionType": "GitHub", "gitHubLocation": {"repository": "octo/app", "commitId": "0f1e2d3"}}),
            None,
        ),
    ];

    for (case, platform, revision, auto_rollback) in cases {
        let emulator = Emulator::new();
        let group = seeded_group(&emulator, case, platform).await;
        let deployment_id = create_deployment(&emulator, case, &group, revision).await;

        let mut request = json!({"deploymentId": deployment_id});
        if let Some(enabled) = auto_rollback {
            request["autoRollbackEnabled"] = json!(enabled);
        }
        let stopped = emulator.ok("StopDeployment", request).await;
        assert_eq!(
            stopped["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: expected the stop to have taken effect, got {stopped:?}"
        );
        assert_eq!(
            stopped["statusMessage"].as_str(),
            Some("No more commands will be scheduled for execution in the deployment instances"),
            "case {case:?}: expected the fixed statusMessage a stop answers with, got {stopped:?}"
        );

        let info = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            info["status"].as_str(),
            Some("Stopped"),
            "case {case:?}: expected the stored deployment to be Stopped, got {info:?}"
        );
        let complete_time = info["completeTime"].as_f64();
        assert!(
            complete_time.is_some(),
            "case {case:?}: expected a completeTime on the stopped deployment, got {info:?}"
        );
        assert!(
            info["rollbackInfo"].is_null(),
            "case {case:?}: expected no rollbackInfo without autoRollbackEnabled, got {info:?}"
        );
        // How the deployment ENDED: a caller stopped it, which is what the record
        // attributes itself to.
        assert_eq!(
            (
                info["errorInformation"]["code"].as_str(),
                info["errorInformation"]["message"].as_str(),
            ),
            (
                Some("MANUAL_STOP"),
                Some("the deployment is cancelled due to an abort request"),
            ),
            "case {case:?}: expected the stop to be attributed to MANUAL_STOP, got {info:?}"
        );
        assert_eq!(
            info["revision"], *revision,
            "case {case:?}: expected the stored revision to survive the stop, got {info:?}"
        );

        // Past the window in which the clock would otherwise have completed the
        // deployment: a stopped deployment stays stopped, and keeps the record it
        // was stopped with.
        tokio::time::sleep(Duration::from_millis(750)).await;
        let later = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            later["status"].as_str(),
            Some("Stopped"),
            "case {case:?}: expected Stopped to be terminal, got {later:?}"
        );
        assert_eq!(
            later["completeTime"].as_f64(),
            complete_time,
            "case {case:?}: expected the completion time to be stable, got {later:?}"
        );
    }
}

/// RULE: a deployment that reached its OWN outcome — the clock completed it, or
/// it failed — has nothing left to stop, so the call is refused with
/// `DeploymentAlreadyCompletedException` naming the state it settled in, and the
/// stored record is untouched; a deployment a stop already ended is in the state
/// the caller is asking for, so a second stop is ANSWERED (idempotent) and leaves
/// that record untouched too.
#[tokio::test]
async fn stopping_a_completed_deployment_is_refused() {
    // Rows differ along HOW the deployment reached its outcome — the clock
    // completing it (on a Lambda group, which resolves a target, so the clock's
    // outcome is `Succeeded`), the clock failing it (an EC2/on-premises group that
    // resolves none) and a caller stopping it — and so along whether a second stop
    // is refused or answered.
    let cases: &[(&str, &str, &str, bool)] = &[
        ("clock-completed", "Lambda", "Succeeded", true),
        ("clock-failed", "Server", "Failed", true),
        ("already-stopped", "Server", "Stopped", false),
    ];

    for (case, platform, terminal, refused) in cases {
        let emulator = Emulator::new();
        let group = seeded_group(&emulator, case, platform).await;
        let revision = s3_revision("stop-bucket", "app.zip");
        let deployment_id = create_deployment(&emulator, case, &group, &revision).await;

        if *terminal == "Stopped" {
            emulator
                .ok("StopDeployment", json!({"deploymentId": deployment_id}))
                .await;
        } else {
            awaited_terminal(&emulator, case, &deployment_id, terminal).await;
        }
        let before = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            before["status"].as_str(),
            Some(*terminal),
            "case {case:?}: expected the deployment to be {terminal} before the refusal, got {before:?}"
        );

        let (status, body) = emulator
            .call("StopDeployment", json!({"deploymentId": deployment_id}))
            .await;
        if !*refused {
            assert_eq!(
                (status, body["status"].as_str()),
                (200, Some("Succeeded")),
                "case {case:?}: expected a second stop of a stopped deployment to be answered, \
                 got {status} {body:?}"
            );
            let after = deployment_info(&emulator, &deployment_id).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the second stop to leave the record untouched, got \
                 {after:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a completed deployment, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "DeploymentAlreadyCompletedException",
            "case {case:?}: expected DeploymentAlreadyCompletedException, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!("The deployment {deployment_id} is already in state {}", terminal.to_uppercase())
                    .as_str()
            ),
            "case {case:?}: expected the refusal to name the state it settled in, got {body:?}"
        );

        let after = deployment_info(&emulator, &deployment_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave the record untouched, got {after:?}"
        );
    }
}

/// RULE: `autoRollbackEnabled` asks for the deployment group to go back to the
/// revision deployed before the stopped deployment, so a rollback DEPLOYMENT of
/// that revision is created — created by the service, not by a client — and the
/// stopped deployment reports it; with nothing deployed before it there is
/// nothing to roll back to, and with no rollback asked for none is created.
#[tokio::test]
async fn a_rollback_is_created_only_when_a_previous_revision_exists() {
    struct Case {
        case: &'static str,
        /// Whether the group already had a deployment before the one stopped.
        previous_deployment: bool,
        /// `autoRollbackEnabled`, absent when `None`.
        auto_rollback: Option<bool>,
        /// Whether the stop reports a `rollbackInfo` at all.
        expect_rollback_info: bool,
        /// Whether a rollback deployment is created for it.
        expect_rollback_deployment: bool,
    }
    let cases = &[
        Case {
            case: "rollback-with-previous",
            previous_deployment: true,
            auto_rollback: Some(true),
            expect_rollback_info: true,
            expect_rollback_deployment: true,
        },
        Case {
            case: "rollback-without-previous",
            previous_deployment: false,
            auto_rollback: Some(true),
            expect_rollback_info: true,
            expect_rollback_deployment: false,
        },
        Case {
            case: "rollback-declined",
            previous_deployment: true,
            auto_rollback: Some(false),
            expect_rollback_info: false,
            expect_rollback_deployment: false,
        },
        Case {
            case: "rollback-unasked",
            previous_deployment: true,
            auto_rollback: None,
            expect_rollback_info: false,
            expect_rollback_deployment: false,
        },
    ];

    for case in cases {
        let name = case.case;
        let emulator = Emulator::new();
        let group = seeded_group(&emulator, name, "Server").await;
        let first_revision = s3_revision("stop-bucket", "first.zip");
        let stopped_revision = s3_revision("stop-bucket", "second.zip");
        if case.previous_deployment {
            create_deployment(&emulator, name, &group, &first_revision).await;
        }
        let deployment_id = create_deployment(&emulator, name, &group, &stopped_revision).await;

        let mut request = json!({"deploymentId": deployment_id});
        if let Some(enabled) = case.auto_rollback {
            request["autoRollbackEnabled"] = json!(enabled);
        }
        emulator.ok("StopDeployment", request).await;

        let info = deployment_info(&emulator, &deployment_id).await;
        let rollback = &info["rollbackInfo"];
        assert_eq!(
            !rollback.is_null(),
            case.expect_rollback_info,
            "case {name:?}: expected rollbackInfo present={}, got {info:?}",
            case.expect_rollback_info
        );
        if !case.expect_rollback_info {
            continue;
        }
        assert_eq!(
            rollback["rollbackTriggeringDeploymentId"].as_str(),
            Some(deployment_id.as_str()),
            "case {name:?}: expected the stopped deployment as the trigger, got {rollback:?}"
        );
        assert!(
            rollback["rollbackMessage"].as_str().is_some_and(|message| !message.is_empty()),
            "case {name:?}: expected a rollbackMessage, got {rollback:?}"
        );
        let rollback_id = rollback["rollbackDeploymentId"].as_str().map(str::to_string);
        assert_eq!(
            rollback_id.is_some(),
            case.expect_rollback_deployment,
            "case {name:?}: expected a rollback deployment={}, got {rollback:?}",
            case.expect_rollback_deployment
        );
        let Some(rollback_id) = rollback_id else {
            continue;
        };
        assert_ne!(
            rollback_id, deployment_id,
            "case {name:?}: expected the rollback to be its own deployment, got {rollback:?}"
        );

        // The rollback is a deployment in its own right: it redeploys what the
        // group was on before, it is the service's own, and the clock runs it
        // like any other deployment.
        let rolled_back = deployment_info(&emulator, &rollback_id).await;
        assert_eq!(
            rolled_back["revision"], first_revision,
            "case {name:?}: expected the rollback to deploy the previous revision, got {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["previousRevision"], stopped_revision,
            "case {name:?}: expected the stopped revision as the rollback's previous one, got {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["creator"].as_str(),
            Some("codeDeployRollback"),
            "case {name:?}: expected the service as the rollback's creator, got {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["applicationName"].as_str(),
            Some(name),
            "case {name:?}: expected the rollback on the same application, got {rolled_back:?}"
        );
        assert_eq!(
            rolled_back["deploymentGroupName"].as_str(),
            Some(group.as_str()),
            "case {name:?}: expected the rollback on the same deployment group, got {rolled_back:?}"
        );
        let listed = emulator
            .ok("ListDeployments", json!({"applicationName": name, "deploymentGroupName": group}))
            .await;
        assert_eq!(
            listed["deployments"][0].as_str(),
            Some(rollback_id.as_str()),
            "case {name:?}: expected the rollback as the newest deployment, got {listed:?}"
        );
        // It runs to its own terminal status without anybody driving it. The
        // group here is an EC2/on-premises one, which resolves no deployment
        // target, so that status is `Failed`.
        awaited_terminal(&emulator, name, &rollback_id, "Failed").await;
    }
}

/// RULE: a deployment id this service cannot act on is refused with CodeDeploy's
/// own code for it — the `…RequiredException` when the request gave none, and
/// the `…DoesNotExistException` when it named one this account does not hold —
/// and nothing is stopped.
#[tokio::test]
async fn an_unusable_deployment_id_is_refused() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "refused", "Server").await;
    let revision = s3_revision("stop-bucket", "app.zip");
    let live = create_deployment(&emulator, "refused", &group, &revision).await;

    // Rows span the ways an id can be unusable: absent, empty, blank, an id of
    // the minted `d-XXXXXXXXX` form that was never minted, and one of another
    // form entirely, which is refused on FORMAT before any lookup.
    let cases: &[(&str, Value, &str)] = &[
        ("absent", json!({}), "DeploymentIdRequiredException"),
        (
            "empty",
            json!({"deploymentId": ""}),
            "DeploymentIdRequiredException",
        ),
        (
            "blank",
            json!({"deploymentId": "   "}),
            "DeploymentIdRequiredException",
        ),
        (
            "never minted",
            json!({"deploymentId": "d-ZZZZZZZZZ"}),
            "DeploymentDoesNotExistException",
        ),
        (
            // An id outside CodeDeploy's `d-` + uppercase-alphanumeric format is
            // refused for its FORMAT, before the deployment is looked up: that is
            // what `InvalidDeploymentIdException` ("At least one of the deployment
            // IDs was specified in an invalid format") distinguishes from an id
            // this account simply does not have, and every operation of this
            // service taking a deployment id now shares that one check.
            "another form",
            json!({"deploymentId": "not-a-deployment-id"}),
            "InvalidDeploymentIdException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator.call("StopDeployment", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );
    }

    // None of those refusals touched the deployment that does exist.
    let info = deployment_info(&emulator, &live).await;
    assert_ne!(
        info["status"].as_str(),
        Some("Stopped"),
        "case bystander: expected the live deployment to be unaffected, got {info:?}"
    );
}
