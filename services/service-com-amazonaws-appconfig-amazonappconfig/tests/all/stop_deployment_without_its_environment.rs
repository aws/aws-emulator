// SPDX-License-Identifier: CC0-1.0

//! `StopDeployment` against a deployment whose environment has been deleted.
//!
//! The rule under test: a deployment RECORD's lifetime is not its environment's. Deleting
//! an environment removes the environment, so every lookup scoped THROUGH it misses —
//! `GetEnvironment`, `GetDeployment` and `ListDeployments` all refuse — but the deployments
//! started into it are still records the application holds, and `StopDeployment`, which
//! addresses the deployment itself, still settles one by the application id, environment id
//! and deployment number its own path carries. A stop of such a record is judged by the
//! state the record is in exactly as a stop of a deployment whose environment is still
//! there: the stop settles the rollback it performs, so the record is `ROLLED_BACK` when the
//! stop answers, with the rollback's completion in the deployment's own event log.
//!
//! The tables span what that rule generalises over: the pacing of the deployment at the
//! instant its environment is deleted (both published bounds of
//! `MinutesBetween0And24Hours`, both `GrowthType` values, a deployment rolling out and one
//! baking, and one that had already completed), whether the configuration profile being
//! deployed was deleted too, what the request says about a revert (absent, `false`,
//! `true`), how many environments were deleted out from under their deployments, and how
//! long after completion a revert is asked for. The recorded request is one row of it.
//!
//! The clock-driven half is asserted against `state::Application` and `state::Deployment`
//! directly, the way the rest of this target asserts a deployment's schedule, so three days
//! of revert window can be checked without any test sleeping.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, Application, Deployment, DeploymentStrategy, Environment, StopRefusal,
};

use crate::appconfig_workflow::{
    account, create_application, create_environment, create_profile, json_call, new_deployment,
    router, send, Answer,
};
use crate::deployment_wire_fidelity::{
    create_strategy, event_log, event_types, host_version, member, Row,
};
use crate::stop_deployment::{get_deployment, start, stop, IDENTITY_MEMBERS};

/// `DELETE` one resource, which is how every delete in this suite is driven.
async fn delete(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, "DELETE", uri, "application/json", Vec::new(), &[]).await
}

/// `GetEnvironment`, the reader that must keep missing after the stop: settling a
/// deployment must not bring its environment back.
async fn get_environment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await
}

/// `ListDeployments`, which reports against the ENVIRONMENT it lists under. `pub(crate)`
/// because more than one suite lists an environment's deployments this way.
pub(crate) async fn list_deployments(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await
}

/// A lookup that must miss, with the modelled `ResourceNotFoundException` and the message
/// `must_name` every value of.
fn assert_not_found(case: &str, operation: &str, answer: &Answer, must_name: &[&str]) {
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: {operation} must answer 404 for a resource it cannot resolve, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("ResourceNotFoundException"),
        "case {case:?}: {operation} must answer the modelled ResourceNotFoundException, got {:?}",
        answer.error_code()
    );
    let message = answer.string("Message").unwrap_or_default();
    for expected in must_name {
        assert!(
            message.contains(expected),
            "case {case:?}: {operation}'s refusal must name {expected:?}, got {message:?}"
        );
    }
}

/// A stop that must be refused with the operation's own modelled `BadRequestException`,
/// reporting the state it refused.
fn assert_bad_request(case: &str, answer: &Answer, must_name: &[&str]) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: StopDeployment must refuse with the modelled BadRequestException's \
         HTTP 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("BadRequestException"),
        "case {case:?}: expected the modelled BadRequestException, got {:?}",
        answer.error_code()
    );
    let message = answer.string("Message").unwrap_or_default();
    for expected in must_name {
        assert!(
            message.contains(expected),
            "case {case:?}: the refusal must say why, naming {expected:?}, got {message:?}"
        );
    }
}

/// What one row of the end-to-end suite deploys and deletes: the strategy's pacing, what
/// the stop's request says about a revert, and whether the configuration profile being
/// deployed is deleted before the environment is.
struct Teardown {
    row: Row,
    allow_revert: Option<bool>,
    delete_profile: bool,
}

/// Delete the hosted version and then the configuration profile a deployment is rolling
/// out, both of which AppConfig allows while that deployment is in flight.
async fn delete_profile_being_deployed(
    case: &str,
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
) {
    for (operation, uri) in [
        (
            "DeleteHostedConfigurationVersion",
            format!("/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/1"),
        ),
        (
            "DeleteConfigurationProfile",
            format!("/applications/{application}/configurationprofiles/{profile}"),
        ),
    ] {
        let answer = delete(router, &uri).await;
        assert_eq!(
            answer.status,
            204,
            "case {case:?}: {operation} must answer 204 mid-deployment, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

/// A deployment in flight survives the deletion of its environment: the reads scoped
/// through that environment miss, and `StopDeployment` still settles the record, answering
/// 202 with the deployment it was started as, now `ROLLED_BACK`.
#[tokio::test]
async fn a_stop_settles_a_deployment_whose_environment_was_deleted() {
    let router = router();
    let account = account();
    let application = create_application(&router, "stop-orphaned").await;

    // Rows differ along the pacing of the deployment the delete arrives in the middle of —
    // the published bounds of `MinutesBetween0And24Hours` (range 0..1440) and of
    // `Percentage` (range 1..100), both `GrowthType` values, and a deployment rolling out
    // as against one baking — along what the request says about a revert, which changes
    // nothing for a deployment that has not completed, and along whether the configuration
    // profile being deployed was deleted first.
    let rows: [Teardown; 5] = [
        Teardown {
            row: ("the longest rollout the model allows", 1440, 100.0, "LINEAR", 0),
            allow_revert: None,
            delete_profile: true,
        },
        Teardown {
            row: ("no rollout window, so it is baking", 0, 100.0, "LINEAR", 1440),
            allow_revert: None,
            delete_profile: false,
        },
        Teardown {
            row: ("an exponential rollout at the smallest growth factor", 60, 1.0, "EXPONENTIAL", 10),
            allow_revert: Some(true),
            delete_profile: false,
        },
        Teardown {
            row: ("a short rollout with a bake time, with a revert disallowed", 5, 10.0, "LINEAR", 10),
            allow_revert: Some(false),
            delete_profile: true,
        },
        Teardown {
            row: ("a rollout in progress, with a revert allowed", 1440, 50.0, "LINEAR", 1440),
            allow_revert: Some(true),
            delete_profile: false,
        },
    ];

    for (index, teardown) in rows.iter().enumerate() {
        let case = teardown.row.0;
        let strategy = create_strategy(&router, index, &teardown.row).await;
        // A profile of its own per row, so a row that deletes the profile it deployed
        // leaves every other row's configuration where it was.
        let profile = create_profile(&router, &application, &format!("config-{index}")).await;
        host_version(&router, &application, &profile).await;
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;

        let started = start(&router, &application, &environment, &strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let start_body = started.json();
        let at_start = member(&start_body, "State");
        let at_start = at_start.as_str().unwrap_or_default();
        assert!(
            matches!(at_start, "DEPLOYING" | "BAKING"),
            "case {case:?}: this row's strategy must leave the deployment in flight when the \
             environment is deleted, got {at_start:?}"
        );

        if teardown.delete_profile {
            delete_profile_being_deployed(case, &router, &application, &profile).await;
        }

        let deleted = delete(
            &router,
            &format!("/applications/{application}/environments/{environment}"),
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: DeleteEnvironment must answer 204 with a deployment in flight, got \
             {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );

        // Everything addressed THROUGH the environment misses: the environment itself, the
        // deployment report that reads it out of the environment (reported against the
        // deployment, the resource that path addresses) and the listing (reported against
        // the environment it lists under).
        let deployment_template = format!(
            "Deployment with Application Id {application}, Environment Id {environment}, and \
             deploymentNumber 1 could not be found for account {account}."
        );
        assert_not_found(
            case,
            "GetEnvironment",
            &get_environment(&router, &application, &environment).await,
            &[
                "Environment with Application Id",
                application.as_str(),
                environment.as_str(),
            ],
        );
        assert_not_found(
            case,
            "GetDeployment",
            &get_deployment(&router, &application, &environment, 1).await,
            &[deployment_template.as_str()],
        );
        assert_not_found(
            case,
            "ListDeployments",
            &list_deployments(&router, &application, &environment).await,
            &[
                "Environment with Application Id",
                application.as_str(),
                environment.as_str(),
            ],
        );

        // The deployment RECORD is still there, and the stop settles it.
        let stopped = stop(&router, &application, &environment, 1, teardown.allow_revert).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer the modelled 202 for a deployment whose \
             environment was deleted, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let stop_body = stopped.json();
        assert_eq!(
            member(&stop_body, "State").as_str(),
            Some("ROLLED_BACK"),
            "case {case:?}: a stop settles the rollback it performs, so the deployment is \
             rolled back when the stop answers, got {}",
            member(&stop_body, "State")
        );
        assert!(
            member(&stop_body, "CompletedAt").is_null(),
            "case {case:?}: a rolled-back deployment never completed, so it reports no \
             completion instant, got {}",
            member(&stop_body, "CompletedAt")
        );
        let log = event_types(&event_log(case, "StopDeployment", &stop_body));
        assert_eq!(
            log.first().map(String::as_str),
            Some("ROLLBACK_COMPLETED"),
            "case {case:?}: the newest event of a rolled-back deployment is its rollback \
             finishing, got {log:?}"
        );
        assert_eq!(
            log.last().map(String::as_str),
            Some("DEPLOYMENT_STARTED"),
            "case {case:?}: the stop appends to the deployment's own log, whose oldest entry is \
             its start, got {log:?}"
        );
        for name in IDENTITY_MEMBERS {
            assert_eq!(
                member(&stop_body, name),
                member(&start_body, name),
                "case {case:?}: the stop must answer the deployment StartDeployment minted, so \
                 its {name} is unchanged, got {} then {}",
                member(&start_body, name),
                member(&stop_body, name)
            );
        }
        // Including the configuration profile it was deploying, which a row may have
        // deleted: the record names what it deployed, not what still exists.
        assert_eq!(
            member(&stop_body, "ConfigurationProfileId").as_str(),
            Some(profile.as_str()),
            "case {case:?}: the stopped deployment must still name the profile it deployed, got {}",
            member(&stop_body, "ConfigurationProfileId")
        );

        // Settling the deployment did not bring its environment back, and the report still
        // reads the deployment through that environment.
        assert_not_found(
            case,
            "GetEnvironment after the stop",
            &get_environment(&router, &application, &environment).await,
            &[environment.as_str()],
        );
        assert_not_found(
            case,
            "GetDeployment after the stop",
            &get_deployment(&router, &application, &environment, 1).await,
            &[deployment_template.as_str()],
        );

        // The round trip: the stop was written to the surviving record, so a second stop
        // sees the state the first one left and refuses it, naming that state.
        let again = stop(&router, &application, &environment, 1, teardown.allow_revert).await;
        assert_bad_request(
            case,
            &again,
            &[
                "Deployment 1",
                application.as_str(),
                environment.as_str(),
                "ROLLED_BACK",
            ],
        );

        // A number no deployment of that environment ever carried is still a miss, so what
        // survives is the RECORD rather than the whole path.
        for number in [2, 4096] {
            let expected = format!(
                "Deployment with Application Id {application}, Environment Id {environment}, and \
                 deploymentNumber {number} could not be found for account {account}."
            );
            assert_not_found(
                &format!("{case}, deployment number {number}"),
                "StopDeployment",
                &stop(&router, &application, &environment, number, teardown.allow_revert).await,
                &[expected.as_str()],
            );
        }
    }
}

/// The surviving records are keyed by the environment that issued them as well as by the
/// number: deleting several environments, each holding its own deployment 1, leaves one
/// settleable record per environment, each answering its own ids.
#[tokio::test]
async fn deleting_several_environments_leaves_one_record_per_environment() {
    let router = router();
    let application = create_application(&router, "stop-orphaned-many").await;
    // The longest rollout the model allows, so every deployment is still in flight when its
    // environment is deleted.
    let row: Row = ("deleted together", 1440, 100.0, "LINEAR", 0);
    let strategy = create_strategy(&router, 0, &row).await;

    // Rows differ along how many environments were torn down before any of them was
    // settled: each one's deployment is number 1, so a record keyed by the number alone
    // would answer the wrong environment.
    let labels = ["first", "second", "third"];
    let mut torn_down = Vec::new();
    for label in labels {
        let profile = create_profile(&router, &application, &format!("config-{label}")).await;
        host_version(&router, &application, &profile).await;
        let environment = create_environment(&router, &application, &format!("env-{label}")).await;
        let started = start(&router, &application, &environment, &strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {label:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let deleted = delete(
            &router,
            &format!("/applications/{application}/environments/{environment}"),
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {label:?}: DeleteEnvironment must answer 204, got {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        torn_down.push((label, environment, profile));
    }

    for (label, environment, profile) in &torn_down {
        let stopped = stop(&router, &application, environment, 1, None).await;
        assert_eq!(
            stopped.status,
            202,
            "case {label:?}: StopDeployment must settle this environment's own record, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let body = stopped.json();
        assert_eq!(
            member(&body, "EnvironmentId").as_str(),
            Some(environment.as_str()),
            "case {label:?}: the settled record must name the environment it was started into, \
             got {}",
            member(&body, "EnvironmentId")
        );
        assert_eq!(
            member(&body, "ConfigurationProfileId").as_str(),
            Some(profile.as_str()),
            "case {label:?}: the settled record must name the profile it was deploying, got {}",
            member(&body, "ConfigurationProfileId")
        );
        assert_eq!(
            member(&body, "DeploymentNumber").as_i64(),
            Some(1),
            "case {label:?}: each environment numbers its own deployments from 1, got {}",
            member(&body, "DeploymentNumber")
        );
    }
}

/// A deployment that had already COMPLETED before its environment was deleted is still
/// judged by the state it is in: the stop applies only when the request allows a revert,
/// and reverting it is terminal at once.
#[tokio::test]
async fn a_completed_deployment_without_its_environment_is_only_revertable() {
    let router = router();
    let application = create_application(&router, "revert-orphaned").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;
    // No rollout window and no bake time: the deployment this paces has nothing to wait
    // out but the one growth interval every rollout takes, so it completes a span after it
    // was started — COMPLETE being the one state a revert applies to.
    let row: Row = ("all at once", 0, 100.0, "LINEAR", 0);
    let strategy = create_strategy(&router, 0, &row).await;

    // Rows differ along what the request says about a revert — absent, false, true — which
    // is the whole of what decides a stop of a completed deployment. One environment per
    // row, since a revert is terminal.
    let rows: [(&str, Option<bool>); 3] = [
        ("no Allow-Revert header at all", None),
        ("Allow-Revert: false", Some(false)),
        ("Allow-Revert: true", Some(true)),
    ];

    for (index, (case, allow_revert)) in rows.iter().enumerate() {
        let environment = create_environment(&router, &application, &format!("done-{index}")).await;
        let started = start(&router, &application, &environment, &strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        // Completing is a transition the clock owes, so the row waits for the condition
        // before deleting the environment the completed deployment was started into.
        let start_body = crate::appconfig_workflow::deployment_in_state(
            &router,
            case,
            &application,
            &environment,
            1,
            &["COMPLETE"],
        )
        .await;
        assert_eq!(
            member(&start_body, "State").as_str(),
            Some("COMPLETE"),
            "case {case:?}: a deployment nobody stopped completes on its own, got {}",
            member(&start_body, "State")
        );

        let deleted = delete(
            &router,
            &format!("/applications/{application}/environments/{environment}"),
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: DeleteEnvironment must answer 204, got {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );

        let answer = stop(&router, &application, &environment, 1, *allow_revert).await;
        if *allow_revert == Some(true) {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: a revert the request allows must answer the modelled 202, got \
                 {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let body = answer.json();
            assert_eq!(
                member(&body, "State").as_str(),
                Some("REVERTED"),
                "case {case:?}: a reverted deployment reports REVERTED, got {}",
                member(&body, "State")
            );
            assert_eq!(
                member(&body, "CompletedAt"),
                member(&start_body, "CompletedAt"),
                "case {case:?}: the revert is a later event of its own, so the instant the \
                 deployment completed is unchanged, got {}",
                member(&body, "CompletedAt")
            );
            let log = event_types(&event_log(case, "StopDeployment", &body));
            assert_eq!(
                log.first().map(String::as_str),
                Some("REVERT_COMPLETED"),
                "case {case:?}: the newest event of a reverted deployment is its revert, got \
                 {log:?}"
            );
        } else {
            // The record IS there — a 404 would say it had been destroyed with the
            // environment — and the state it is in is what refuses the stop.
            assert_bad_request(
                case,
                &answer,
                &[
                    "Deployment 1",
                    application.as_str(),
                    environment.as_str(),
                    "COMPLETE",
                ],
            );
        }
    }
}

/// An application holding one environment with one deployment in it, paced by `strategy`
/// and started at `started_at`, for the clock-driven suites below. The ids are the ones
/// [`new_deployment`] names, so the environment holds the deployment recorded into it.
fn application_with_a_deployment(
    strategy: &DeploymentStrategy,
    started_at: SystemTime,
) -> Application {
    let new = new_deployment();
    let mut environment = Environment::new(
        new.environment_id.clone(),
        new.application_id.clone(),
        "deployed-into".to_string(),
        None,
        Vec::new(),
        HashMap::new(),
    );
    environment.record_deployment(Deployment::start(new_deployment(), strategy, started_at));
    let mut application = Application::new(
        new.application_id.clone(),
        "torn-down".to_string(),
        None,
        HashMap::new(),
    );
    assert!(
        application.try_insert_environment(environment).is_some(),
        "the environment must be stored under the application"
    );
    application
}

/// A strategy with one row's pacing.
fn strategy_of(
    duration: i32,
    growth_type: &str,
    growth_factor: f32,
    bake: i32,
) -> DeploymentStrategy {
    DeploymentStrategy {
        id: "orph001".to_string(),
        name: "orphaned".to_string(),
        description: None,
        deployment_duration_in_minutes: duration,
        growth_type: growth_type.to_string(),
        growth_factor,
        final_bake_time_in_minutes: bake,
        replicate_to: state::REPLICATE_TO_NONE.to_string(),
        tags: HashMap::new(),
    }
}

/// The event types of a stored deployment's log, newest first.
fn stored_event_types(deployment: &Deployment) -> Vec<String> {
    deployment
        .event_log_newest_first()
        .iter()
        .filter_map(|event| event.event_type.clone())
        .collect()
}

/// Deleting an environment keeps the deployments it held, and the clock then owes such a
/// record nothing: the rollout it was pacing was into an environment that no longer exists,
/// so the record keeps the state the delete left it in however long passes — and a stop
/// rolls it back, finished, at the instant it arrives.
#[test]
fn a_deployment_without_its_environment_is_owed_nothing_by_the_clock_and_rolls_back_at_once() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Rows differ along the pacing of the deployment the delete arrives in the middle of —
    // both published bounds of `MinutesBetween0And24Hours` and both `GrowthType` values —
    // and along how far into the deployment the environment is deleted. Each row's instant
    // is inside a stage its strategy declares: the rollout's intervals are one short span
    // each (`state::STAGE_DURATION`) and the bake keeps its declared minutes
    // (`state::bake_duration`).
    let rows: [(&str, i32, &str, f32, i32, u64); 5] = [
        ("deleted a second into the longest rollout", 1440, state::GROWTH_LINEAR, 100.0, 0, 1),
        ("deleted at the instant it started", 60, state::GROWTH_EXPONENTIAL, 1.0, 10, 0),
        ("deleted while baking", 0, state::GROWTH_LINEAR, 100.0, 1440, 1),
        ("deleted mid-rollout with the longest bake", 1440, state::GROWTH_LINEAR, 50.0, 1440, 2),
        ("deleted a second before a short rollout ends", 5, state::GROWTH_LINEAR, 10.0, 0, 2),
    ];

    for (case, duration, growth_type, growth_factor, bake, elapsed_secs) in rows {
        let strategy = strategy_of(duration, growth_type, growth_factor, bake);
        let mut application = application_with_a_deployment(&strategy, started_at);
        let new = new_deployment();
        let deleted_at = started_at + Duration::from_secs(elapsed_secs);
        if let Some(environment) = application.environments.get_mut(&new.environment_id) {
            environment.materialize(deleted_at);
        }

        let removed = application.delete_environment(&new.environment_id);
        assert!(
            removed.is_some(),
            "case {case:?}: deleting the environment must answer the record it removed"
        );
        assert!(
            !application.environments.contains_key(&new.environment_id),
            "case {case:?}: the environment itself is gone, so every lookup through it misses"
        );

        let deployment = application
            .deployment_without_environment_mut(&new.environment_id, new.deployment_number)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the deployment started into the deleted environment is still \
                     a record the application holds"
                )
            });
        let state_at_delete = deployment.state.clone();
        assert!(
            deployment.is_in_flight(),
            "case {case:?}: this row's strategy must leave the deployment in flight at the \
             delete, got {state_at_delete:?}"
        );
        let events_at_delete = deployment.event_log.len();

        // The clock owes it nothing: the rollout it was pacing has nowhere to go, so no
        // growth step, bake or completion falls due however long passes.
        deployment.materialize_without_environment(deleted_at + Duration::from_secs(90 * 86_400));
        assert_eq!(
            deployment.state, state_at_delete,
            "case {case:?}: a record whose environment is gone keeps the state the delete left \
             it in, got {:?}",
            deployment.state
        );
        assert_eq!(
            deployment.event_log.len(),
            events_at_delete,
            "case {case:?}: no later read appends an event the clock does not owe"
        );
        assert!(
            deployment.completed_at.is_none(),
            "case {case:?}: nothing completed a rollout into an environment that is gone, got \
             {:?}",
            deployment.completed_at
        );

        // The stop rolls it back, and the rollback is over when the stop answers.
        let stopped_at = deleted_at + Duration::from_secs(5);
        assert_eq!(
            deployment.stop(false, stopped_at),
            Ok(()),
            "case {case:?}: a deployment in flight can be stopped once its environment is gone"
        );
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLED_BACK,
            "case {case:?}: a stop settles the rollback it performs, so the record is rolled \
             back, got {:?}",
            deployment.state
        );
        assert_eq!(
            deployment.completed_at, None,
            "case {case:?}: a deployment the stop rolled back never completed, so it holds no \
             completion instant, got {:?}",
            deployment.completed_at
        );
        assert_eq!(
            deployment.rollback_completion_due, None,
            "case {case:?}: nothing is due on a rollback that is already over, got {:?}",
            deployment.rollback_completion_due
        );
        assert!(
            !deployment.is_active(),
            "case {case:?}: a rolled-back deployment is one nothing is working on any more"
        );
        let log = stored_event_types(deployment);
        assert_eq!(
            log.first().map(String::as_str),
            Some(state::EVENT_ROLLBACK_COMPLETED),
            "case {case:?}: the newest event is the rollback finishing, got {log:?}"
        );
        assert_eq!(
            log.last().map(String::as_str),
            Some(state::EVENT_DEPLOYMENT_STARTED),
            "case {case:?}: the oldest event is the deployment's start, got {log:?}"
        );

        // And nothing is left to stop, whatever the next request allows.
        let settled = deployment.clone();
        for allow_revert in [false, true] {
            assert_eq!(
                deployment.stop(allow_revert, stopped_at + Duration::from_secs(1)),
                Err(StopRefusal::NotStoppable),
                "case {case:?}: a rolled-back deployment cannot be stopped again (AllowRevert \
                 {allow_revert})"
            );
            assert_eq!(
                deployment.state, settled.state,
                "case {case:?}: a refused stop leaves the record exactly as it was, got {:?}",
                deployment.state
            );
            assert_eq!(
                deployment.event_log.len(),
                settled.event_log.len(),
                "case {case:?}: a refused stop appends no event"
            );
        }
    }
}

/// A rollback AppConfig had ALREADY started before the environment was deleted is the one
/// transition such a record is still owed: it was due at an instant that has passed whether
/// or not any environment still held the deployment, so the clock finishes it then — and
/// not before.
#[test]
fn the_clock_still_finishes_a_rollback_started_before_the_environment_was_deleted() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Rows differ along the pacing of the deployment that was stopped.
    let rows: [(&str, i32, i32); 3] = [
        ("the longest rollout the model allows", 1440, 0),
        ("no rollout window, so it is baking", 0, 1440),
        ("a short rollout with a bake time", 5, 10),
    ];

    for (case, duration, bake) in rows {
        let strategy = strategy_of(duration, state::GROWTH_LINEAR, 100.0, bake);
        let mut application = application_with_a_deployment(&strategy, started_at);
        let new = new_deployment();

        let due = {
            let deployment = application
                .environments
                .get_mut(&new.environment_id)
                .and_then(|environment| {
                    environment.deployments.get_mut(&new.deployment_number)
                })
                .unwrap_or_else(|| panic!("case {case:?}: the recorded deployment is readable"));
            assert_eq!(
                deployment.begin_rollback(started_at),
                Ok(()),
                "case {case:?}: a deployment in flight can be rolled back"
            );
            deployment.rollback_completion_due.unwrap_or_else(|| {
                panic!("case {case:?}: a rolling-back deployment records when its rollback falls due")
            })
        };

        assert!(
            application.delete_environment(&new.environment_id).is_some(),
            "case {case:?}: deleting the environment must answer the record it removed"
        );
        let deployment = application
            .deployment_without_environment_mut(&new.environment_id, new.deployment_number)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the rolling-back deployment survives its environment")
            });
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLING_BACK,
            "case {case:?}: the delete does not finish a rollback early, got {:?}",
            deployment.state
        );

        deployment.materialize_without_environment(due - Duration::from_millis(1));
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLING_BACK,
            "case {case:?}: a read before the rollback falls due must not finish it, got {:?}",
            deployment.state
        );

        deployment.materialize_without_environment(due);
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLED_BACK,
            "case {case:?}: the rollback finishes when it fell due, got {:?}",
            deployment.state
        );
        assert_eq!(
            deployment.completed_at, None,
            "case {case:?}: a rolled-back deployment never completed, so it holds no completion \
             instant, got {:?}",
            deployment.completed_at
        );
        let log = stored_event_types(deployment);
        assert_eq!(
            log.first().map(String::as_str),
            Some(state::EVENT_ROLLBACK_COMPLETED),
            "case {case:?}: the newest event is the rollback finishing, got {log:?}"
        );
    }
}

/// The window the model documents for a revert — "AppConfig only allows a revert within 72
/// hours of deployment completion" — governs a completed record whose environment has been
/// deleted exactly as it governs one whose environment is still there.
#[test]
fn the_documented_revert_window_governs_a_record_without_its_environment() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let window = state::REVERT_WINDOW;

    // Rows walk the window: inside it, on the documented bound, one second past it, and far
    // past it.
    let rows: [(&str, Duration, bool); 5] = [
        ("at the instant it completed", Duration::ZERO, true),
        ("an hour later", Duration::from_secs(3600), true),
        ("exactly on the documented bound", window, true),
        ("a second past the bound", window + Duration::from_secs(1), false),
        ("far past the bound", window * 2, false),
    ];

    for (case, elapsed, inside_window) in rows {
        // No rollout window and no bake time, so the deployment completes one growth
        // interval after it started and the window is measured from an instant this test
        // knows. The completion is owed while the environment still holds the deployment,
        // so the record is brought up to it before the environment is deleted.
        let strategy = strategy_of(0, state::GROWTH_LINEAR, 100.0, 0);
        let mut application = application_with_a_deployment(&strategy, started_at);
        let new = new_deployment();
        application
            .environments
            .get_mut(&new.environment_id)
            .unwrap_or_else(|| panic!("case {case:?}: the environment holds the deployment"))
            .materialize(started_at + state::STAGE_DURATION);
        assert!(
            application.delete_environment(&new.environment_id).is_some(),
            "case {case:?}: deleting the environment must answer the record it removed"
        );
        let deployment = application
            .deployment_without_environment_mut(&new.environment_id, new.deployment_number)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the completed deployment survives its environment")
            });
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_COMPLETE,
            "case {case:?}: this row needs a completed deployment, got {:?}",
            deployment.state
        );
        let completed_at = deployment.completed_at.unwrap_or_else(|| {
            panic!("case {case:?}: a completed deployment carries the instant it completed")
        });
        let at = completed_at + elapsed;
        let events_before = deployment.event_log.len();

        // A stop that does not ask for a revert never applies to a completed deployment,
        // whenever it arrives and whether or not its environment is still there.
        assert_eq!(
            deployment.stop(false, at),
            Err(StopRefusal::NotStoppable),
            "case {case:?}: a completed deployment can only be reverted, not stopped"
        );
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_COMPLETE,
            "case {case:?}: a refused stop leaves the record exactly as it was, got {:?}",
            deployment.state
        );

        let outcome = deployment.stop(true, at);
        if inside_window {
            assert_eq!(
                outcome,
                Ok(()),
                "case {case:?}: a revert inside the documented window is allowed"
            );
            assert_eq!(
                deployment.state,
                state::DEPLOYMENT_REVERTED,
                "case {case:?}: a reverted deployment reports REVERTED, got {:?}",
                deployment.state
            );
            assert_eq!(
                deployment.completed_at,
                Some(completed_at),
                "case {case:?}: the revert does not move the instant the deployment completed, \
                 got {:?}",
                deployment.completed_at
            );
            let log = stored_event_types(deployment);
            assert_eq!(
                log.first().map(String::as_str),
                Some(state::EVENT_REVERT_COMPLETED),
                "case {case:?}: the newest event of a reverted deployment is its revert, got \
                 {log:?}"
            );
        } else {
            assert_eq!(
                outcome,
                Err(StopRefusal::RevertWindowElapsed),
                "case {case:?}: a revert past the documented window is refused"
            );
            assert_eq!(
                deployment.state,
                state::DEPLOYMENT_COMPLETE,
                "case {case:?}: a refused revert leaves the record exactly as it was, got {:?}",
                deployment.state
            );
            assert_eq!(
                deployment.event_log.len(),
                events_before,
                "case {case:?}: a refused revert appends no event"
            );
        }
    }
}

/// Create environment `index` under `application`, start one deployment into it paced by
/// `strategy` at `started_at`, and delete the environment again — the loop the retention
/// bound governs. Answers the environment's id.
///
/// `settled` brings the deployment up to what the clock owes it one growth interval in,
/// before the environment goes: every rollout takes an interval, so a record whose
/// environment is deleted at the instant it started is one still in flight.
fn tear_down_one_environment(
    application: &mut Application,
    index: usize,
    strategy: &DeploymentStrategy,
    started_at: SystemTime,
    settled: bool,
) -> String {
    let environment_id = format!("env{index:04}");
    let mut environment = Environment::new(
        environment_id.clone(),
        application.id.clone(),
        format!("env-{index}"),
        None,
        Vec::new(),
        HashMap::new(),
    );
    let mut new = new_deployment();
    new.environment_id = environment_id.clone();
    environment.record_deployment(Deployment::start(new, strategy, started_at));
    if settled {
        environment.materialize(started_at + state::STAGE_DURATION);
    }
    assert!(
        application.try_insert_environment(environment).is_some(),
        "environment {index} must be stored before it is deleted"
    );
    assert!(
        application.delete_environment(&environment_id).is_some(),
        "deleting environment {index} must answer the record it removed"
    );
    environment_id
}

/// An application with nothing under it, for the retention suites below.
fn empty_application() -> Application {
    Application::new(
        "abcd123".to_string(),
        "torn-down-in-a-loop".to_string(),
        None,
        HashMap::new(),
    )
}

/// The bound on the records an application keeps for environments it no longer has binds
/// UNCONDITIONALLY: tearing environments down in a loop keeps the most recently started
/// records and drops the earliest, whatever state those records are in — the live
/// environment that would otherwise cap them is freed by the very delete that retains the
/// record.
#[test]
fn the_retention_bound_on_records_without_an_environment_binds_unconditionally() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let bound = state::MAX_RETAINED_DEPLOYMENTS_WITHOUT_ENVIRONMENT;
    let count = bound + 2;

    // Rows differ along what state the deployments were in when their environments were
    // deleted: the longest rollout the model allows leaves every record in flight, and a
    // strategy with no rollout window and no bake time leaves every one complete.
    let rows: [(&str, DeploymentStrategy, bool); 2] = [
        (
            "every record still rolling out",
            strategy_of(1440, state::GROWTH_LINEAR, 100.0, 0),
            false,
        ),
        (
            "every record already complete",
            strategy_of(0, state::GROWTH_LINEAR, 100.0, 0),
            true,
        ),
    ];

    for (case, strategy, settled) in rows {
        let mut application = empty_application();
        let mut ids = Vec::new();
        for index in 0..count {
            // One second apart, so which record started earliest is a total order.
            ids.push(tear_down_one_environment(
                &mut application,
                index,
                &strategy,
                started_at + Duration::from_secs(index as u64),
                settled,
            ));
        }

        let kept = application.deployments_without_environment.len();
        assert_eq!(
            kept, bound,
            "case {case:?}: the records kept for deleted environments must stay at the bound, \
             got {kept}"
        );
        for (index, environment_id) in ids.iter().enumerate() {
            let held = application
                .deployment_without_environment_mut(environment_id, 1)
                .is_some();
            let expected = index >= count - bound;
            assert_eq!(
                held, expected,
                "case {case:?}: the most recently started {bound} records are the ones kept, so \
                 record {index} must be held: {expected}, got {held}"
            );
        }
    }
}

/// Within that bound, a record the service has FINISHED with is dropped before one a caller
/// could still settle: the active record is the one about to be stopped.
#[test]
fn the_retention_bound_drops_finished_records_before_active_ones() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let bound = state::MAX_RETAINED_DEPLOYMENTS_WITHOUT_ENVIRONMENT;
    // The longest rollout the model allows leaves a record in flight; no rollout window and
    // no bake time leaves one complete.
    let in_flight = strategy_of(1440, state::GROWTH_LINEAR, 100.0, 0);
    let complete = strategy_of(0, state::GROWTH_LINEAR, 100.0, 0);

    let mut application = empty_application();
    // One short of the bound in flight, so the records added next are what the bound has to
    // choose between.
    let mut active_ids = Vec::new();
    for index in 0..bound - 1 {
        active_ids.push(tear_down_one_environment(
            &mut application,
            index,
            &in_flight,
            started_at + Duration::from_secs(index as u64),
            false,
        ));
    }

    // Three finished records on top of that: the first fills the bound, and each one after
    // it is over it, so a finished record goes every time while every in-flight record
    // stays.
    let mut finished_ids = Vec::new();
    for offset in 0..3 {
        let index = bound - 1 + offset;
        finished_ids.push(tear_down_one_environment(
            &mut application,
            index,
            &complete,
            started_at + Duration::from_secs(index as u64),
            true,
        ));
        let kept = application.deployments_without_environment.len();
        assert!(
            kept <= bound,
            "the records kept must never exceed the bound of {bound}, got {kept} after adding \
             finished record {offset}"
        );
    }

    for (index, environment_id) in active_ids.iter().enumerate() {
        assert!(
            application
                .deployment_without_environment_mut(environment_id, 1)
                .is_some(),
            "a record the service is still working on must be kept, so in-flight record {index} \
             must still be held"
        );
    }
    let finished_held = finished_ids
        .iter()
        .filter(|environment_id| {
            application
                .deployments_without_environment
                .contains_key(&((*environment_id).clone(), 1))
        })
        .count();
    assert_eq!(
        finished_held, 1,
        "the bound had room for one more record, so exactly one of the three finished ones is \
         kept, got {finished_held}"
    );
    assert!(
        application
            .deployment_without_environment_mut(
                finished_ids.last().map(String::as_str).unwrap_or_default(),
                1
            )
            .is_some(),
        "and the one kept is the most recently started of them"
    );
}

/// Deleting an environment must not make the deployment record reachable where it was not
/// before: the record answers only the environment id it was started into, and only while
/// the application that holds it is there.
#[tokio::test]
async fn a_surviving_record_is_reachable_only_under_the_ids_it_was_started_with() {
    let router = router();
    let account = account();
    let application = create_application(&router, "stop-orphaned-scope").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;
    // The longest rollout the model allows, so the deployment is in flight throughout.
    let row: Row = ("still rolling out", 1440, 100.0, "LINEAR", 0);
    let strategy = create_strategy(&router, 0, &row).await;
    let environment = create_environment(&router, &application, "deleted").await;
    let sibling = create_environment(&router, &application, "untouched").await;

    let started = start(&router, &application, &environment, &strategy, &profile).await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let deleted = delete(
        &router,
        &format!("/applications/{application}/environments/{environment}"),
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "DeleteEnvironment must answer 204, got {} / {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // Rows differ along which part of the path does not name the surviving record: a
    // sibling environment that never issued that number, an environment that never
    // existed, and an application the account does not hold — which is resolved first and
    // so keeps its own template.
    let rows: [(&str, &str, &str, String); 3] = [
        (
            "a sibling environment that never issued the number",
            &application,
            &sibling,
            format!(
                "Deployment with Application Id {application}, Environment Id {sibling}, and \
                 deploymentNumber 1 could not be found for account {account}."
            ),
        ),
        (
            "an environment that never existed",
            &application,
            "nosuchenv",
            format!(
                "Deployment with Application Id {application}, Environment Id nosuchenv, and \
                 deploymentNumber 1 could not be found for account {account}."
            ),
        ),
        (
            "an application the account does not hold",
            "zzzzzzz",
            &environment,
            format!("Application with Id zzzzzzz could not be found for account {account}."),
        ),
    ];

    for (case, application_id, environment_id, expected) in rows {
        let answer = stop(&router, application_id, environment_id, 1, None).await;
        assert_not_found(case, "StopDeployment", &answer, &[expected.as_str()]);
    }

    // The record itself is still settleable under the ids it was started with.
    let stopped = stop(&router, &application, &environment, 1, None).await;
    assert_eq!(
        stopped.status,
        202,
        "the surviving record must still be settleable, got {} / {}",
        stopped.status,
        String::from_utf8_lossy(&stopped.body)
    );
}
