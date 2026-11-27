// SPDX-License-Identifier: CC0-1.0

//! What a ROLLBACK of a service deployment is RECORDED as, driven end to end over the shared
//! wire harness in [`crate::ecs_wire`].
//!
//! The rule under test:
//!
//! > A rollback happens WITHIN the service deployment it undoes, whatever started it. That
//! > deployment keeps its own identity and ends in `ROLLBACK_SUCCESSFUL` with the reason the
//! > rollback happened as its `statusReason`, carrying the `rollback` that names that reason,
//! > the instant it started and the service revision deployed as part of it, while its
//! > `targetServiceRevision` stays the revision it was rolling out and its
//! > `sourceServiceRevisions` the one the service was running. The redeployment that puts
//! > that revision back is a `Deployment` of the service and NOT a release of its own, so no
//! > second service deployment is recorded for it: a service's deployment history holds
//! > exactly the releases it had, and the rolled-back deployment is the service's current
//! > one.
//!
//! The docs the rule is read off:
//!
//! * `ServiceDeployment$rollback` -> `Rollback`: "The rollback options the service deployment
//!   uses when the deployment fails", whose `reason` is "The reason the rollback happened.
//!   For example, the circuit breaker initiated the rollback operation", `startedAt` "The
//!   time the rollback started" and `serviceRevisionArn` "The ARN of the service revision
//!   deployed as part of the rollback".
//! * `ServiceDeployment$statusReason`: "Information about why the service deployment is in
//!   the current status. For example, the circuit breaker detected a failure."
//! * `ServiceDeployment$sourceServiceRevisions` ("The currently deployed workload
//!   configuration") and `$targetServiceRevision` ("The workload configuration being
//!   deployed").
//! * `ServiceDeploymentStatus` (`smithy_query(query="shape",
//!   shape="ServiceDeploymentStatus")`: PENDING, SUCCESSFUL, STOPPED, STOP_REQUESTED,
//!   IN_PROGRESS, ROLLBACK_REQUESTED, ROLLBACK_IN_PROGRESS, ROLLBACK_SUCCESSFUL,
//!   ROLLBACK_FAILED).
//! * `ListServiceDeployments`: "A service deployment happens when you release a software
//!   update for the service."
//! * `DeploymentCircuitBreaker$rollback`: "If rollback is on, when a service deployment
//!   fails, the service is rolled back to the last deployment that completed successfully."
//! * `ContinueServiceDeployment`: "Use this API to either continue the deployment to the next
//!   stage or roll back to the previous service revision."
//! * `StopServiceDeployment$stopType`: "How you want Amazon ECS to stop the service. The
//!   valid values are ROLLBACK."
//!
//! The rows differ along WHAT STARTED the rollback — the three doors to one rollback: the
//! stop type a caller asks a stop with, a request that names no stop type at all, the
//! `ROLLBACK` action taken on a paused lifecycle hook, and the deployment circuit breaker
//! firing by itself. Where a row's rollback was asked for is the only thing that differs; what
//! it is recorded as is the rule every row asserts.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeServices`, `ListServiceDeployments` and `DescribeServiceDeployments`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, describe_service_deployment, list, ok, poll_deployment_status,
    poll_until, service, string,
};

/// The cluster every service below runs in.
const CLUSTER: &str = "rollbackrecords";

/// The `statusReason` and `Rollback$reason` a rollout the deployment circuit breaker rolled
/// back carries. Fixed text, with no part of it taken from the request.
const CIRCUIT_BREAKER_ROLLED_BACK_REASON: &str =
    "Service deployment rolled back because the circuit breaker threshold was exceeded.";

/// The command the container of a revision that stays up runs.
///
/// A wait is SPENT, wherever it stands in the program (`container_program_waits.rs` pins that
/// rule), so a container told to wait inside a LOOP has no end of its own to reach at all,
/// which is what the sibling suites that need a revision to stay up run too.
const STAYS_UP: [&str; 3] = ["sh", "-c", "while true; do sleep 1; done"];

/// The command the container of the revision the circuit-breaker row rolls out runs: it ends
/// the instant it starts, so every task that rollout places is one the service cannot keep.
const EXITS_AT_ONCE: [&str; 3] = ["sh", "-c", "exit 1"];

/// How the rollback of one row was ASKED FOR.
#[derive(Clone, Copy, Debug)]
enum Door {
    /// `StopServiceDeployment`, with the stop type the request names (or none).
    Stop(Option<&'static str>),
    /// The `ROLLBACK` action taken on the PAUSE lifecycle hook the rollout waits at.
    LifecycleHook,
    /// Nothing a caller asked for: the deployment circuit breaker failed the rollout and
    /// rolled it back itself.
    CircuitBreaker,
}

/// One row: the door the rollback came through, and the `statusReason` the rolled-back
/// rollout must carry when that door words one.
struct RollbackCase {
    case: &'static str,
    door: Door,
    /// The sentence the rolled-back rollout's `statusReason` and `rollback.reason` must be,
    /// for the door whose sentence AWS publishes; `None` where only the rollback's presence
    /// is the subject.
    reason: Option<&'static str>,
}

/// One container definition running `command`.
fn container(name: &str, command: &[&str]) -> Value {
    json!({
        "name": name,
        "image": "public.ecr.aws/nginx/nginx:latest",
        // "at least one of 'memory' or 'memoryReservation' must be specified" for a
        // container of a revision this service places.
        "memory": 128,
        "essential": true,
        "command": command,
    })
}

/// Register one revision of `family` whose container runs `command`, and return its ARN.
async fn register_running(
    router: &MultiServiceRouter,
    family: &str,
    command: &[&str],
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container("app", command)],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Create a service on `definition` whose rollout the ECS scheduler owns, with
/// `configuration` as its deployment configuration when the row needs one, and return the
/// ARN of the service deployment that rollout is recorded as.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    configuration: Option<Value>,
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": CLUSTER,
        "serviceName": name,
        "taskDefinition": definition,
        "desiredCount": 1,
        "launchType": "EC2",
    });
    if let Some(configuration) = configuration {
        request["deploymentConfiguration"] = configuration;
    }
    let body = ok(router, "CreateService", request, case).await;
    string(&body["service"], "currentServiceDeployment", case)
}

/// Move `name` onto `definition`, with `configuration` when the row needs one, and return the
/// ARN of the service deployment the update started.
async fn update_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    configuration: Option<Value>,
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": CLUSTER,
        "service": name,
        "taskDefinition": definition,
    });
    if let Some(configuration) = configuration {
        request["deploymentConfiguration"] = configuration;
    }
    let body = ok(router, "UpdateService", request, case).await;
    string(&body["service"], "currentServiceDeployment", case)
}

/// The stored service, as `DescribeServices` reports it.
async fn describe_service(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described"))
        .clone()
}

/// The briefs `ListServiceDeployments` reports for `name`, newest first — the SECOND reader
/// of the same records, so a history only the describe view agrees with cannot pass.
async fn list_deployments(router: &MultiServiceRouter, name: &str, case: &str) -> Vec<Value> {
    let body = ok(
        router,
        "ListServiceDeployments",
        json!({ "cluster": CLUSTER, "service": name }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
}

/// The `PAUSE` hook the lifecycle-hook row's rollout waits at.
fn blue_green_configuration() -> Value {
    json!({
        "strategy": "BLUE_GREEN",
        // "You must provide this parameter when you use the BLUE_GREEN deployment strategy."
        "bakeTimeInMinutes": 1,
        "lifecycleHooks": [{
            "targetType": "PAUSE",
            "lifecycleStages": ["POST_SCALE_UP"],
        }],
    })
}

/// The deployment configuration the circuit-breaker row runs under: the breaker turned on
/// with the rollback it decides, and a threshold of ONE failed task
/// (`ThresholdConfiguration$type`: "For `COUNT`, Amazon ECS uses `value` directly as the
/// threshold"), so the row spends one lost launch rather than the clamped default of three.
fn breaker_configuration() -> Value {
    json!({
        "deploymentCircuitBreaker": {
            "enable": true,
            "rollback": true,
            "thresholdConfiguration": { "type": "COUNT", "value": 1 },
        },
        // A floor of none lets the scheduler take the task set being replaced off before a
        // replacement is up, so the rollout's own task set is all that stands for it.
        "minimumHealthyPercent": 0,
        "maximumPercent": 100,
    })
}

/// The hook of `deployment` that is waiting for an action.
async fn awaiting_hook_id(router: &MultiServiceRouter, arn: &str, case: &str) -> String {
    let deployment = poll_until(
        case,
        "the rollout to reach the stage its hook pauses it at",
        || describe_service_deployment(router, arn, case),
        |deployment| {
            list(deployment, "lifecycleHookDetails", case)
                .iter()
                .any(|hook| hook["status"] == json!("AWAITING_ACTION"))
        },
    )
    .await;
    let hook = list(&deployment, "lifecycleHookDetails", case)
        .into_iter()
        .find(|hook| hook["status"] == json!("AWAITING_ACTION"))
        .unwrap_or_else(|| panic!("case {case:?}: expected a waiting hook, got {deployment}"));
    string(&hook, "hookId", case)
}

/// Ask for the rollback of `arn` through `door`.
async fn start_the_rollback(
    router: &MultiServiceRouter,
    arn: &str,
    door: Door,
    case: &str,
) {
    match door {
        Door::Stop(stop_type) => {
            let mut request = json!({ "serviceDeploymentArn": arn });
            if let Some(stop_type) = stop_type {
                request["stopType"] = json!(stop_type);
            }
            let body = ok(router, "StopServiceDeployment", request, case).await;
            assert_eq!(
                string(&body, "serviceDeploymentArn", case),
                arn,
                "case {case:?}: expected the stopped deployment's own ARN"
            );
        }
        Door::LifecycleHook => {
            let hook_id = awaiting_hook_id(router, arn, case).await;
            let body = ok(
                router,
                "ContinueServiceDeployment",
                json!({
                    "serviceDeploymentArn": arn,
                    "hookId": hook_id,
                    "action": "ROLLBACK",
                }),
                case,
            )
            .await;
            assert_eq!(
                string(&body, "serviceDeploymentArn", case),
                arn,
                "case {case:?}: expected the rolled-back deployment's own ARN"
            );
        }
        // Nothing is asked for: the breaker fires on the launches the rollout loses.
        Door::CircuitBreaker => {}
    }
}

/// RULE: a rollback is recorded on the rollout it UNDID, whichever door asked for it, and the
/// redeployment that puts the previous revision back is no release of its own.
#[tokio::test]
async fn a_rollback_is_recorded_on_the_rollout_it_undid() {
    let cases = [
        RollbackCase {
            case: "a stop asked for with the rollback stop type",
            door: Door::Stop(Some("ROLLBACK")),
            reason: None,
        },
        RollbackCase {
            case: "a stop that names no stop type",
            door: Door::Stop(None),
            reason: None,
        },
        RollbackCase {
            case: "the rollback action on a paused lifecycle hook",
            door: Door::LifecycleHook,
            reason: None,
        },
        RollbackCase {
            case: "the deployment circuit breaker firing by itself",
            door: Door::CircuitBreaker,
            reason: Some(CIRCUIT_BREAKER_ROLLED_BACK_REASON),
        },
    ];

    let router = service();
    cluster_with_capacity(&router, CLUSTER, "capacity").await;

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let family = format!("rolledback{index}");
        let name = format!("rolled-back-service-{index}");
        let breaker = matches!(row.door, Door::CircuitBreaker);
        // The revision the service runs first, and the one the rolled-back rollout releases.
        // Only the circuit-breaker row needs a revision whose tasks the service cannot keep:
        // the other doors roll back a rollout that is perfectly healthy.
        let first = register_running(&router, &family, &STAYS_UP, case).await;
        let second = if breaker {
            register_running(&router, &family, &EXITS_AT_ONCE, case).await
        } else {
            register_running(&router, &family, &STAYS_UP, case).await
        };
        let configuration = match row.door {
            Door::CircuitBreaker => Some(breaker_configuration()),
            _ => None,
        };
        let released = create_service(&router, &name, &first, configuration.clone(), case).await;
        // The rollback puts back "the last deployment that completed successfully", so the
        // first rollout has to have reached its own end before the second is released. That
        // end is the CLOCK's, so it is polled for rather than timed.
        let first_revision = {
            let deployment = poll_deployment_status(&router, &released, "SUCCESSFUL", case).await;
            string(&deployment["targetServiceRevision"], "arn", case)
        };

        let hooked = matches!(row.door, Door::LifecycleHook);
        let rolled_back = update_service(
            &router,
            &name,
            &second,
            if hooked {
                Some(blue_green_configuration())
            } else {
                configuration
            },
            case,
        )
        .await;
        assert_ne!(
            rolled_back, released,
            "case {case:?}: expected the release of the second revision to be a rollout of \
             its own"
        );
        let second_revision = {
            let deployment = describe_service_deployment(&router, &rolled_back, case).await;
            string(&deployment["targetServiceRevision"], "arn", case)
        };

        start_the_rollback(&router, &rolled_back, row.door, case).await;

        // Where the rollback leaves the rollout is the CLOCK's: polled for, never timed.
        let deployment =
            poll_deployment_status(&router, &rolled_back, "ROLLBACK_SUCCESSFUL", case).await;
        assert!(
            !deployment["finishedAt"].is_null(),
            "case {case:?}: a rolled-back rollout reports when it ended, got {deployment}"
        );
        // The rollback is recorded ON this rollout: the revision deployed as part of it is
        // the one the service was running before it.
        assert_eq!(
            deployment["rollback"]["serviceRevisionArn"],
            json!(first_revision),
            "case {case:?}: expected the rollback to name the revision it put back, got \
             {deployment}"
        );
        assert!(
            !deployment["rollback"]["startedAt"].is_null(),
            "case {case:?}: a rollback reports when it started, got {deployment}"
        );
        let rollback_reason = string(&deployment["rollback"], "reason", case);
        assert!(
            !rollback_reason.is_empty(),
            "case {case:?}: a rollback reports why it happened, got {deployment}"
        );
        // And the rollout it undid still reports the revisions it was moving the service
        // BETWEEN — the one being deployed as its target, the one being replaced as its
        // source — never the other way round.
        assert_eq!(
            deployment["targetServiceRevision"]["arn"],
            json!(second_revision),
            "case {case:?}: expected the rolled-back rollout to target the revision it was \
             rolling out, got {deployment}"
        );
        let sources = list(&deployment, "sourceServiceRevisions", case);
        assert_eq!(
            sources
                .iter()
                .map(|source| string(source, "arn", case))
                .collect::<Vec<String>>(),
            vec![first_revision.clone()],
            "case {case:?}: expected the rolled-back rollout to come from the revision the \
             service was running, got {deployment}"
        );
        if let Some(reason) = row.reason {
            assert_eq!(
                string(&deployment, "statusReason", case),
                reason,
                "case {case:?}: expected the status reason the door that rolled it back \
                 publishes, got {deployment}"
            );
            assert_eq!(
                rollback_reason, reason,
                "case {case:?}: expected the rollback to carry that same reason, got \
                 {deployment}"
            );
        }

        // The redeployment that put the revision back is NO release of its own: the history
        // holds exactly the two releases this service had, newest first.
        let listed = list_deployments(&router, &name, case).await;
        let arns: Vec<String> = listed
            .iter()
            .map(|brief| string(brief, "serviceDeploymentArn", case))
            .collect();
        assert_eq!(
            arns,
            vec![rolled_back.clone(), released.clone()],
            "case {case:?}: expected the two releases this service had, newest first, got \
             {listed:?}"
        );
        // And the listing reads the same records, so it reports the same ends and the same
        // revisions.
        assert_eq!(
            (
                string(&listed[0], "status", case),
                string(&listed[0], "targetServiceRevisionArn", case),
            ),
            ("ROLLBACK_SUCCESSFUL".to_string(), second_revision.clone()),
            "case {case:?}: expected the newest release to be the rolled-back one, got \
             {listed:?}"
        );
        assert_eq!(
            (
                string(&listed[1], "status", case),
                string(&listed[1], "targetServiceRevisionArn", case),
            ),
            ("SUCCESSFUL".to_string(), first_revision.clone()),
            "case {case:?}: expected the release before it to keep its own end and revision, \
             got {listed:?}"
        );

        // The service runs the revision the rollback put back, and the rolled-back rollout is
        // the service deployment it is on: there is no later one for it to be on.
        let stored = describe_service(&router, &name, case).await;
        assert_eq!(
            stored["taskDefinition"],
            json!(first),
            "case {case:?}: expected the service back on the previous revision, got {stored}"
        );
        assert_eq!(
            string(&stored, "currentServiceDeployment", case),
            rolled_back,
            "case {case:?}: expected the rolled-back rollout to be the service's current \
             service deployment, got {stored}"
        );
    }
}
