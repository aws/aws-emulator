// SPDX-License-Identifier: CC0-1.0

//! A deployment SETTLES: nobody has to stop it for it to reach a terminal state.
//!
//! The rule under test: a deployment is paced by the strategy it was started with, and
//! every stage that pacing schedules is owed by the CLOCK — so a strategy that paces any
//! stage at all, a rollout window or a final bake time, leaves `StartDeployment` answering
//! the deployment at its first stage, `DEPLOYING` at 0% of its targets; a strategy that
//! paces NEITHER leaves AppConfig nothing to pace or monitor, so its deployment is already
//! `COMPLETE` when the start answers; and every one of them reaches `COMPLETE` without a
//! caller stopping it
//! (`smithy_query(query="operation",operation="StartDeployment,GetDeployment")`: both
//! report `State` as a `DeploymentState`, `PercentageComplete`, `CompletedAt` and the
//! `EventLog` the stages write).
//!
//! The rows span what the rule generalises over: whether the strategy declares a rollout
//! window, whether it declares a final bake time, and the magnitude of each — both
//! published bounds of `MinutesBetween0And24Hours` (`range 0..1440`) and a value between
//! them — because a deployment must settle whatever the declared minutes are. Every
//! assertion names its case and prints what it read.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, ComAmazonawsAppconfigAmazonappconfigState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, deployment_in_state, router_on,
};
use crate::extension_action_points::{
    create_version, get_deployment, given_strategy, start_deployment,
};

/// How long a row waits for the deployment it started to settle before failing.
///
/// A ceiling on the WAIT, not a statement about the span a stage takes: a deployment that
/// has not settled by here is one nothing is moving, which is the defect this suite pins.
const SETTLE_BY: Duration = Duration::from_secs(60);

/// How often a row re-reads the deployment while waiting for it to settle.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The `DeploymentState` values a deployment stops at: "COMPLETE", a rollback that
/// finished, and a revert of one that had completed. The states this suite waits for.
const TERMINAL_STATES: &[&str] = &["COMPLETE", "ROLLED_BACK", "REVERTED"];

/// One row: the case, the strategy's declared rollout minutes and final bake minutes, the
/// state and share of targets `StartDeployment` must answer, and the deployment-event
/// types the stages the strategy schedules must have logged by the time it settles.
struct Row {
    case: &'static str,
    duration_in_minutes: i32,
    bake_in_minutes: i32,
    state_at_start: &'static str,
    percentage_at_start: f64,
    logged_events: &'static [&'static str],
}

/// Bring the end of the declared final bake time forward for every deployment the
/// environment holds, by moving the instant each one started back by that bake.
///
/// The bake keeps the minutes the caller declared (`state::bake_duration`) — it is the
/// window they asked AppConfig to monitor the deployment for — so a row about the
/// deployment SETTLING drives the clock that transition is owed by instead of waiting out
/// the declared minutes. The transition itself is still the service's: nothing here moves
/// a state or writes an event.
fn bring_the_bake_forward(
    shared: &Arc<engine::InMemoryEngine>,
    application_id: &str,
    environment_id: &str,
    bake_in_minutes: i32,
) {
    let held = shared.state::<ComAmazonawsAppconfigAmazonappconfigState>();
    let mut store = held.lock();
    let Some(environment) = store
        .application_mut(application_id)
        .and_then(|application| application.environments.get_mut(environment_id))
    else {
        panic!("the environment the deployment was started into must be held");
    };
    for deployment in environment.deployments.values_mut() {
        deployment.started_at -= state::bake_duration(bake_in_minutes);
    }
}

/// Poll `GetDeployment` until the deployment reports a terminal state, and answer the
/// document that reported it. Fails the row — naming it and the last state read — when
/// nothing settles within [`SETTLE_BY`].
pub async fn settled(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    number: i32,
) -> Value {
    let deadline = Instant::now() + SETTLE_BY;
    loop {
        let read = get_deployment(router, application, environment, number).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetDeployment expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let document = read.json();
        let state = document
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if TERMINAL_STATES.contains(&state.as_str()) {
            return document;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: deployment {number} did not reach a terminal state within {}s; \
             last state {state:?}",
            SETTLE_BY.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `EventType` of every entry in a deployment's event log.
fn logged(document: &Value) -> BTreeSet<String> {
    document
        .get("EventLog")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("EventType").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_deployment_reaches_a_terminal_state_whatever_pacing_its_strategy_declares() {
    // The engine behind the router, because a row whose strategy declares a final bake
    // time drives the clock that bake is owed by (`bring_the_bake_forward`) instead of
    // waiting out the declared minutes.
    let shared: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&shared);
    let application = create_application(&router, "settling-app").await;
    let profile = create_profile(&router, &application, "settling-profile").await;
    let seeded = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"enabled":true}"#,
    )
    .await;
    assert_eq!(
        seeded.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        seeded.status,
        String::from_utf8_lossy(&seeded.body)
    );

    let rows = [
        Row {
            case: "neither a rollout window nor a bake time",
            duration_in_minutes: 0,
            bake_in_minutes: 0,
            // Nothing to pace and nothing to monitor, so the deployment is over when the
            // start answers.
            state_at_start: "COMPLETE",
            percentage_at_start: 100.0,
            logged_events: &["DEPLOYMENT_STARTED", "DEPLOYMENT_COMPLETED"],
        },
        Row {
            case: "a rollout window and no bake time",
            duration_in_minutes: 1,
            bake_in_minutes: 0,
            state_at_start: "DEPLOYING",
            percentage_at_start: 0.0,
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
        Row {
            case: "a bake time and no rollout window",
            duration_in_minutes: 0,
            bake_in_minutes: 1,
            // A bake time is a stage AppConfig monitors AFTER the rollout, so the rollout
            // itself happens after the call too: the start answers 0% of targets reached.
            state_at_start: "DEPLOYING",
            percentage_at_start: 0.0,
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "BAKE_TIME_STARTED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
        Row {
            case: "a rollout window and a bake time",
            duration_in_minutes: 30,
            bake_in_minutes: 10,
            state_at_start: "DEPLOYING",
            percentage_at_start: 0.0,
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "BAKE_TIME_STARTED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
        Row {
            case: "both at the published upper bound of 1440 minutes",
            duration_in_minutes: 1440,
            bake_in_minutes: 1440,
            state_at_start: "DEPLOYING",
            percentage_at_start: 0.0,
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "BAKE_TIME_STARTED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One environment per row: an environment holds one deployment at a time.
        let environment =
            create_environment(&router, &application, &format!("settling-env-{index}")).await;
        let strategy = given_strategy(
            &router,
            &format!("settling-{index}"),
            row.duration_in_minutes,
            row.bake_in_minutes,
        )
        .await;
        let started = start_deployment(
            &router,
            &application,
            &environment,
            &serde_json::json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {:?}: StartDeployment expected 201, got {} / {}",
            row.case,
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let start_document = started.json();
        // The stages after the one the start answers are AppConfig's, not the caller's: a
        // strategy that paces any stage leaves the deployment at its first one.
        assert_eq!(
            start_document.get("State").and_then(Value::as_str),
            Some(row.state_at_start),
            "case {:?}: expected State {:?} when the start answers, got {start_document}",
            row.case,
            row.state_at_start
        );
        assert_eq!(
            start_document
                .get("PercentageComplete")
                .and_then(Value::as_f64),
            Some(row.percentage_at_start),
            "case {:?}: expected {}% of targets reached when the start answers, got \
             {start_document}",
            row.case,
            row.percentage_at_start
        );
        // The declared minutes are the strategy's and are reported back as declared: the
        // pacing of a stage is not a value a reader of the deployment is told about.
        assert_eq!(
            start_document
                .get("DeploymentDurationInMinutes")
                .and_then(Value::as_i64),
            Some(i64::from(row.duration_in_minutes)),
            "case {:?}: expected the declared duration, got {start_document}",
            row.case
        );
        assert_eq!(
            start_document
                .get("FinalBakeTimeInMinutes")
                .and_then(Value::as_i64),
            Some(i64::from(row.bake_in_minutes)),
            "case {:?}: expected the declared bake time, got {start_document}",
            row.case
        );
        let number = start_document
            .get("DeploymentNumber")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartDeployment answered no DeploymentNumber, got \
                     {start_document}",
                    row.case
                )
            }) as i32;

        // A row whose strategy declares a final bake time: the bake BEGINS on its own, one
        // span after the rollout reached its targets, and lasts the minutes the caller
        // declared — so the row waits for the bake to begin and then brings its end
        // forward, rather than waiting those minutes out.
        if row.bake_in_minutes > 0 {
            deployment_in_state(
                &router,
                row.case,
                &application,
                &environment,
                i64::from(number),
                &["BAKING"],
            )
            .await;
            bring_the_bake_forward(&shared, &application, &environment, row.bake_in_minutes);
        }

        let document = settled(&router, row.case, &application, &environment, number).await;
        assert_eq!(
            document.get("State").and_then(Value::as_str),
            Some("COMPLETE"),
            "case {:?}: a deployment nobody stopped completes, got {document}",
            row.case
        );
        assert_eq!(
            document.get("PercentageComplete").and_then(Value::as_f64),
            Some(100.0),
            "case {:?}: a completed deployment reached every target, got {document}",
            row.case
        );
        assert!(
            document.get("CompletedAt").is_some(),
            "case {:?}: a completed deployment reports when it completed, got {document}",
            row.case
        );
        let events = logged(&document);
        for expected in row.logged_events {
            assert!(
                events.contains(*expected),
                "case {:?}: expected the stage {expected:?} on the event log, got {events:?}",
                row.case
            );
        }
        // A stage the strategy does not schedule logs nothing: no bake time, no bake event.
        if row.bake_in_minutes == 0 {
            assert!(
                !events.contains("BAKE_TIME_STARTED"),
                "case {:?}: a strategy with no bake time bakes nothing, got {events:?}",
                row.case
            );
        }

        // The environment is released by the deployment settling, so the next deployment
        // into it is accepted rather than refused as a conflict.
        let again = start_deployment(
            &router,
            &application,
            &environment,
            &serde_json::json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            again.status,
            201,
            "case {:?}: a settled environment accepts the next deployment, got {} / {}",
            row.case,
            again.status,
            String::from_utf8_lossy(&again.body)
        );
    }
}
