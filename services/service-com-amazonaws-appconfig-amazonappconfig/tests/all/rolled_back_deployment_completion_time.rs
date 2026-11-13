// SPDX-License-Identifier: CC0-1.0

//! `CompletedAt` reports the instant a deployment COMPLETED, so a deployment that never
//! completed reports none.
//!
//! The rule under test: only the transition into `COMPLETE` sets `CompletedAt`
//! (`Deployment$CompletedAt` is "the time the deployment completed",
//! `smithy_query(query="shape",shape="Deployment")`), so a deployment that is still
//! rolling out, and a deployment whose rollout was aborted and rolled back, carry no
//! completion instant at all — while a later REVERT of a deployment that did complete
//! keeps the instant it completed at. `CompletedAt` is therefore the member that separates
//! a rollout that finished from one that was abandoned, in every document that reports a
//! deployment.
//!
//! The tables span what the rule generalises over: the pacing of the deployment that is
//! stopped (both `GrowthType` values and the published bounds of
//! `MinutesBetween0And24Hours`, so the stop lands in `DEPLOYING` for some rows and in
//! `BAKING` for others), which document reports the deployment (the stop's own 202,
//! `GetDeployment`, the `ListDeployments` summary), and which terminal state the record
//! reached (`ROLLED_BACK` by a caller's stop, `ROLLED_BACK` by the clock finishing a
//! rollback AppConfig started, `COMPLETE`, `REVERTED`).

use std::time::{Duration, SystemTime};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{self, Deployment};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, new_deployment, router,
};
use crate::deployment_wire_fidelity::{create_strategy, host_version, member, Row};
use crate::stop_deployment::{get_deployment, start, stop};

/// `ListDeployments` for one environment, answering the summary of deployment `number`.
async fn deployment_summary(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) -> Value {
    let body = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await
    .json();
    member(&body, "Items")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|item| member(item, "DeploymentNumber").as_i64() == Some(number))
        .cloned()
        .unwrap_or_else(|| panic!("ListDeployments must report deployment {number}, got {body}"))
}

/// A deployment a caller stopped was abandoned, not completed: every document that reports
/// it — the stop's own answer, `GetDeployment` and the `ListDeployments` summary — reports
/// `StartedAt` and NO `CompletedAt`.
#[tokio::test]
async fn a_rolled_back_deployment_reports_no_completion_time() {
    let router = router();
    let application = create_application(&router, "rollback-completion").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // Rows differ along the pacing of the stopped deployment — the published bounds of
    // `MinutesBetween0And24Hours` (0..1440) and of `Percentage` (1..100), and both
    // `GrowthType` values — so the stop arrives while the rollout is still growing in some
    // rows and while the deployment is baking in others.
    let rows: [Row; 4] = [
        ("the longest rollout the model allows", 1440, 100.0, "LINEAR", 0),
        ("no rollout window, so it is baking", 0, 100.0, "LINEAR", 1440),
        ("an exponential canary with a bake to follow", 60, 1.0, "EXPONENTIAL", 10),
        ("a rollout and the longest bake", 1440, 50.0, "LINEAR", 1440),
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.0;
        let strategy = create_strategy(&router, index, row).await;
        // One environment per row, so no stop reaches another row's deployment.
        let environment =
            create_environment(&router, &application, &format!("env-{strategy}")).await;

        let started = start(&router, &application, &environment, &strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let start_body = started.json();
        assert!(
            member(&start_body, "CompletedAt").is_null(),
            "case {case:?}: a deployment that is still in progress has not completed, got {}",
            member(&start_body, "CompletedAt")
        );

        let stopped = stop(&router, &application, &environment, 1, None).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer the modelled 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );

        // The same deployment through every document that reports one.
        let read_back = get_deployment(&router, &application, &environment, 1).await;
        let summary = deployment_summary(&router, &application, &environment, 1).await;
        for (document, body) in [
            ("StopDeployment", stopped.json()),
            ("GetDeployment", read_back.json()),
            ("ListDeployments", summary),
        ] {
            assert_eq!(
                member(&body, "State").as_str(),
                Some(state::DEPLOYMENT_ROLLED_BACK),
                "case {case:?}: {document} must report the stopped deployment rolled back, got {}",
                member(&body, "State")
            );
            assert!(
                member(&body, "CompletedAt").is_null(),
                "case {case:?}: {document} must report no completion time for a deployment \
                 that was rolled back, got {}",
                member(&body, "CompletedAt")
            );
            assert!(
                !member(&body, "StartedAt").is_null(),
                "case {case:?}: {document} still reports when the deployment started, got {}",
                member(&body, "StartedAt")
            );
        }
    }
}

/// A deployment that DID complete carries the instant it completed at, and a revert — the
/// other terminal state a stop can leave — keeps that instant: the revert is a later event
/// of the deployment's own, not a second completion.
#[tokio::test]
async fn a_completed_deployment_keeps_its_completion_time_through_a_revert() {
    let router = router();
    let application = create_application(&router, "completion-kept").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // No rollout window and no bake time: the deployment has nothing to wait out but the
    // one growth interval every rollout takes, so it completes a span after it started —
    // COMPLETE being the one state a revert applies to.
    let row: Row = ("no rollout window, no bake", 0, 100.0, "LINEAR", 0);
    let strategy = create_strategy(&router, 0, &row).await;
    let environment = create_environment(&router, &application, "completing").await;

    start(&router, &application, &environment, &strategy, &profile).await;
    let start_body = crate::appconfig_workflow::deployment_in_state(
        &router,
        row.0,
        &application,
        &environment,
        1,
        &[state::DEPLOYMENT_COMPLETE],
    )
    .await;
    assert_eq!(
        member(&start_body, "State").as_str(),
        Some(state::DEPLOYMENT_COMPLETE),
        "a deployment nobody stopped completes on its own, got {}",
        member(&start_body, "State")
    );
    let completed_at = member(&start_body, "CompletedAt");
    assert!(
        !completed_at.is_null(),
        "a completed deployment carries the instant it completed, got {completed_at}"
    );

    let reverted = stop(&router, &application, &environment, 1, Some(true)).await;
    assert_eq!(
        reverted.status,
        202,
        "StopDeployment must answer the modelled 202, got {} / {}",
        reverted.status,
        String::from_utf8_lossy(&reverted.body)
    );
    let read_back = get_deployment(&router, &application, &environment, 1).await;
    let summary = deployment_summary(&router, &application, &environment, 1).await;
    for (document, body) in [
        ("StopDeployment", reverted.json()),
        ("GetDeployment", read_back.json()),
        ("ListDeployments", summary),
    ] {
        assert_eq!(
            member(&body, "State").as_str(),
            Some(state::DEPLOYMENT_REVERTED),
            "{document} must report the reverted deployment, got {}",
            member(&body, "State")
        );
        assert_eq!(
            member(&body, "CompletedAt"),
            completed_at,
            "{document}: a revert does not move the instant the deployment completed, got {}",
            member(&body, "CompletedAt")
        );
    }
}

/// Both ways a deployment reaches `ROLLED_BACK` leave it without a completion instant: the
/// caller's stop, which settles the rollback, and the rollback AppConfig starts itself,
/// which the clock finishes. Asserted against the stored record, so the clock-driven half
/// needs no test to sleep.
#[test]
fn neither_rollback_path_records_a_completion() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Rows differ along the pacing of the deployment being aborted and along how far into
    // it the abort arrives, including at the instant it started. Each row's instant is
    // inside a stage its strategy declares: the rollout's intervals are one short span each
    // (`state::STAGE_DURATION`) and the bake keeps its declared minutes
    // (`state::bake_duration`).
    let rows: [(&str, i32, &str, f32, i32, u64); 4] = [
        ("a second into the longest rollout", 1440, state::GROWTH_LINEAR, 100.0, 0, 1),
        ("at the instant it started", 60, state::GROWTH_EXPONENTIAL, 1.0, 10, 0),
        ("while baking", 0, state::GROWTH_LINEAR, 100.0, 1440, 4),
        ("a second before a short rollout ends", 5, state::GROWTH_LINEAR, 10.0, 0, 2),
    ];

    for (case, duration, growth_type, growth_factor, bake, elapsed_secs) in rows {
        let strategy = state::DeploymentStrategy {
            id: "pace001".to_string(),
            name: "paced".to_string(),
            description: None,
            deployment_duration_in_minutes: duration,
            growth_type: growth_type.to_string(),
            growth_factor,
            final_bake_time_in_minutes: bake,
            replicate_to: state::REPLICATE_TO_NONE.to_string(),
            tags: std::collections::HashMap::new(),
        };
        let aborted_at = started_at + Duration::from_secs(elapsed_secs);

        // The caller's stop: terminal the moment it answers, and nothing completed.
        let mut stopped = Deployment::start(new_deployment(), &strategy, started_at);
        stopped.materialize(aborted_at);
        assert_eq!(
            stopped.stop(false, aborted_at),
            Ok(()),
            "case {case:?}: a deployment in progress can be stopped"
        );
        assert_eq!(
            stopped.state,
            state::DEPLOYMENT_ROLLED_BACK,
            "case {case:?}: a stop settles the rollback it performs, got {:?}",
            stopped.state
        );
        assert_eq!(
            stopped.completed_at, None,
            "case {case:?}: a deployment the stop rolled back never completed, got {:?}",
            stopped.completed_at
        );

        // The rollback AppConfig starts itself: the clock finishes it, and that is still
        // not a completion.
        let mut aborted = Deployment::start(new_deployment(), &strategy, started_at);
        aborted.materialize(aborted_at);
        assert_eq!(
            aborted.begin_rollback(aborted_at),
            Ok(()),
            "case {case:?}: a deployment in progress can be rolled back"
        );
        let due = aborted.rollback_completion_due.unwrap_or_else(|| {
            panic!("case {case:?}: a rolling-back deployment records when its rollback falls due")
        });
        aborted.materialize(due + Duration::from_secs(3600));
        assert_eq!(
            aborted.state,
            state::DEPLOYMENT_ROLLED_BACK,
            "case {case:?}: the clock finishes the rollback it owed, got {:?}",
            aborted.state
        );
        assert_eq!(
            aborted.completed_at, None,
            "case {case:?}: a rollback finishing is not the deployment completing, got {:?}",
            aborted.completed_at
        );
    }
}
