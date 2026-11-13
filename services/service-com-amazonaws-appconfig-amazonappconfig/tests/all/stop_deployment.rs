// SPDX-License-Identifier: CC0-1.0

//! `StopDeployment`: what a stop does to the deployment it names, and when it is refused.
//!
//! The rule under test: a stop applies to a deployment the service is still working on —
//! one rolling out or baking — which it rolls back, and, only when the request allows a
//! revert, to one that completed inside the window AppConfig documents for a revert,
//! which it reverts; a deployment in any other state is refused with the operation's
//! modelled `BadRequestException` and left exactly as it was. Either outcome is SETTLED by
//! the call: the deployment the stop answers is already in the terminal state the stop left
//! it in — `ROLLED_BACK` with the rollback's completion on its event log, or `REVERTED` —
//! so a reader of that deployment, and the environment that holds it, report the same
//! outcome with nothing left in flight.
//!
//! The tables below span what that rule generalises over: the pacing of the deployment
//! being stopped (a rollout window, none at all with a bake time to follow, both growth
//! types, the published bounds of both durations), how far into the deployment the stop
//! arrives, whether the request allows a revert (absent, `false`, `true`), the state the
//! deployment is in when the stop arrives (every one of them, including the two a previous
//! stop left behind) and how long after completion a revert is asked for — at the
//! documented window's bound and past it. The recorded request is one row of it.
//!
//! The clock-driven half is asserted against `state::Deployment` directly, the way the
//! rest of this target asserts a deployment's schedule, so three days of revert window can
//! be checked without any test sleeping.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, Deployment, DeploymentStrategy, Environment, StopRefusal,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, new_deployment, router,
    send, Answer,
};
use crate::deployment_wire_fidelity::{
    create_strategy, event_log, event_types, host_version, member, Row,
};

/// `StartDeployment` of version 1 of `profile` into `environment`.
///
/// `pub(crate)` because more than one suite starts a deployment this way.
pub(crate) async fn start(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    strategy: &str,
    profile: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await
}

/// `StopDeployment`: `DELETE
/// /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}`.
///
/// `AllowRevert` is bound to the `Allow-Revert` HEADER rather than to a body member
/// (`smithy_query(query="operation", operation="StopDeployment")`), so `None` here is a
/// request that carries no such header at all.
///
/// `pub(crate)` because more than one suite stops a deployment this way.
pub(crate) async fn stop(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i32,
    allow_revert: Option<bool>,
) -> Answer {
    let headers: Vec<(&str, String)> = match allow_revert {
        Some(value) => vec![("Allow-Revert", value.to_string())],
        None => Vec::new(),
    };
    send(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        "application/json",
        Vec::new(),
        &headers,
    )
    .await
}

/// `GetDeployment`, the implemented reader a stop is read back through. `pub(crate)`
/// because more than one suite reads a stopped deployment back.
pub(crate) async fn get_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i32,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        &Value::Null,
    )
    .await
}

/// The `State` `GetEnvironment` reports for one environment.
async fn environment_state(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Option<String> {
    let answer = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await
    .json();
    answer
        .get("State")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A refusal that must be the operation's own modelled client error, carrying a message
/// derived from the request and the record it addressed: AppConfig words it `Deployment
/// <number> for App <application id> and Environment <environment id> cannot be stopped,
/// it has a status of <state>.`, so every value in `must_name` is one the request or the
/// record supplied, and `one_of` are the states the record could be in — more than one
/// only where the clock may have moved the record on between two calls.
fn assert_bad_request(case: &str, answer: &Answer, must_name: &[&str], one_of: &[&str]) {
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
    assert!(
        one_of.iter().any(|state| message.contains(state)),
        "case {case:?}: the refusal must report the state it refused, one of {one_of:?}, got \
         {message:?}"
    );
}

/// The `DeploymentState` the document reports, which every assertion below reads through
/// one place.
fn read_deployment_state(case: &str, body: &Value) -> String {
    member(body, "State")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a Deployment document reports a State, got {body}"))
        .to_string()
}

/// The terminal `DeploymentState` a stop of a deployment in progress leaves behind: the
/// stop settles the rollback it performs, so there is no transitional state for a reader
/// to catch.
const ROLLED_BACK: &str = "ROLLED_BACK";

/// The `EnvironmentState` wire value matching [`ROLLED_BACK`].
const ROLLED_BACK_ENVIRONMENT: &str = "RolledBack";

/// The `DeploymentEventType` a settled rollback records.
const ROLLBACK_COMPLETED: &str = "ROLLBACK_COMPLETED";

/// The `Description` AppConfig gives the rollback a request initiated, as the recorded
/// stop reports it.
const ROLLED_BACK_BY_USER: &str = "Deployment rolled back by user request";

/// A lookup that must miss, with the modelled `ResourceNotFoundException`.
fn assert_not_found(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: StopDeployment must answer 404 for a resource it cannot resolve, got \
         {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("ResourceNotFoundException"),
        "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
        answer.error_code()
    );
}

/// The members of a Deployment document that describe WHICH deployment it is, rather than
/// how far along it is: a stop must answer the same deployment `StartDeployment` minted.
/// `pub(crate)` because every suite that stops a deployment asserts them.
pub(crate) const IDENTITY_MEMBERS: &[&str] = &[
    "ApplicationId",
    "EnvironmentId",
    "DeploymentStrategyId",
    "ConfigurationProfileId",
    "DeploymentNumber",
    "ConfigurationName",
    "ConfigurationLocationUri",
    "ConfigurationVersion",
    "DeploymentDurationInMinutes",
    "FinalBakeTimeInMinutes",
    "GrowthType",
    "GrowthFactor",
    "StartedAt",
    "Type",
];

/// A stop of a deployment the service is still working on rolls it back, and the rollback
/// is FINISHED when the stop answers: the 202 reports the deployment `ROLLED_BACK` with the
/// rollback's completion newest on its event log, the implemented reader answers the same
/// record, the environment reports it, and a second stop has nothing left to stop.
#[tokio::test]
async fn stopping_a_deployment_in_progress_rolls_it_back_whatever_paced_it() {
    let router = router();
    let application = create_application(&router, "stop-rollback").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // Rows differ along the pacing of the deployment being stopped — the published bounds
    // of `MinutesBetween0And24Hours` (range 0..1440) and of `Percentage` (range 1..100),
    // and both `GrowthType` values — and along what the request says about a revert, which
    // changes nothing for a deployment that has not completed: "the status of an
    // in-progress deployment will be ROLLED_BACK".
    let rows: [(Row, Option<bool>); 5] = [
        (("the longest rollout the model allows", 1440, 100.0, "LINEAR", 0), None),
        (("no rollout window, so it is baking", 0, 100.0, "LINEAR", 1440), None),
        (
            ("an exponential rollout at the smallest growth factor", 60, 1.0, "EXPONENTIAL", 10),
            None,
        ),
        (("a rollout in progress, with a revert allowed", 1440, 50.0, "LINEAR", 1440), Some(true)),
        (("a short rollout, with a revert disallowed", 5, 10.0, "LINEAR", 0), Some(false)),
    ];

    for (index, (row, allow_revert)) in rows.iter().enumerate() {
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
        let at_start = member(&start_body, "State");
        let at_start = at_start.as_str().unwrap_or_default();
        assert!(
            matches!(at_start, "DEPLOYING" | "BAKING"),
            "case {case:?}: this row's strategy must leave the deployment in progress, got \
             {at_start:?}"
        );

        let stopped = stop(&router, &application, &environment, 1, *allow_revert).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer the modelled 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let stop_body = stopped.json();
        assert_eq!(
            member(&stop_body, "State").as_str(),
            Some(ROLLED_BACK),
            "case {case:?}: a deployment stopped in progress is rolled back when the stop \
             answers, got {}",
            member(&stop_body, "State")
        );
        assert!(
            member(&stop_body, "CompletedAt").is_null(),
            "case {case:?}: a rolled-back deployment never completed, so it reports no \
             completion instant, got {}",
            member(&stop_body, "CompletedAt")
        );
        let stop_events = event_log(case, "StopDeployment", &stop_body);
        let log = event_types(&stop_events);
        assert_eq!(
            log.first().map(String::as_str),
            Some(ROLLBACK_COMPLETED),
            "case {case:?}: the newest event of a stopped deployment is its rollback \
             finishing, got {log:?}"
        );
        let newest = stop_events.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            member(&newest, "Description").as_str(),
            Some(ROLLED_BACK_BY_USER),
            "case {case:?}: the rollback's event reports what initiated it, got {newest}"
        );
        assert_eq!(
            member(&newest, "TriggeredBy").as_str(),
            Some("USER"),
            "case {case:?}: the request initiated the rollback, got {newest}"
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
                "case {case:?}: StopDeployment must answer the deployment StartDeployment \
                 minted, so its {name} is unchanged, got {} then {}",
                member(&start_body, name),
                member(&stop_body, name)
            );
        }

        // The round trip: the stop was persisted, and it settled the deployment, so the
        // implemented reader answers exactly the record the stop answered — no later read
        // moves a deployment nothing is working on any more.
        let fetched = get_deployment(&router, &application, &environment, 1).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeployment must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        let read_body = fetched.json();
        assert_eq!(
            read_body, stop_body,
            "case {case:?}: GetDeployment must report the settled record the stop answered"
        );
        let read_state = read_deployment_state(case, &read_body);
        let read_log = event_types(&event_log(case, "GetDeployment", &read_body));
        assert_eq!(
            read_state, ROLLED_BACK,
            "case {case:?}: the stored deployment is rolled back, got {read_state:?}"
        );
        assert_eq!(
            read_log.last().map(String::as_str),
            Some("DEPLOYMENT_STARTED"),
            "case {case:?}: the oldest entry of the stored log is the deployment's start, got \
             {read_log:?}"
        );
        // The environment reports the deployment it holds, in `EnvironmentState`'s own
        // CamelCase wire form: the rollback is over, so the environment reports the state
        // it left behind.
        let environment_now = environment_state(&router, &application, &environment).await;
        assert_eq!(
            environment_now.as_deref(),
            Some(ROLLED_BACK_ENVIRONMENT),
            "case {case:?}: an environment whose deployment was stopped reports \
             {ROLLED_BACK_ENVIRONMENT:?}, got {environment_now:?}"
        );

        // A second stop has nothing left to stop, and says which state it refused.
        let again = stop(&router, &application, &environment, 1, *allow_revert).await;
        assert_bad_request(
            case,
            &again,
            &["Deployment 1", application.as_str(), environment.as_str()],
            &[ROLLED_BACK],
        );
        let after_refusal = get_deployment(&router, &application, &environment, 1).await;
        assert_eq!(
            read_deployment_state(case, &after_refusal.json()),
            ROLLED_BACK,
            "case {case:?}: a refused stop must leave the record where the stop put it, got {}",
            member(&after_refusal.json(), "State")
        );
    }
}

/// A completed deployment is reverted only by a request that allows a revert: without one
/// the stop is refused and the record is untouched, with one the deployment is `REVERTED`
/// at once — there is no intermediate state for a revert — and a second revert has nothing
/// left to revert.
#[tokio::test]
async fn a_completed_deployment_is_reverted_only_when_the_request_allows_it() {
    let router = router();
    let application = create_application(&router, "stop-revert").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // No rollout window and no bake time: the deployment this paces has nothing to wait
    // out but the one growth interval every rollout takes, so it completes a span after it
    // was started — COMPLETE being the one state a revert applies to.
    let row: Row = ("all at once", 0, 100.0, "LINEAR", 0);
    let strategy = create_strategy(&router, 0, &row).await;
    let environment = create_environment(&router, &application, "reverted").await;

    let started = start(&router, &application, &environment, &strategy, &profile).await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let start_body = crate::appconfig_workflow::deployment_in_state(
        &router,
        "all at once",
        &application,
        &environment,
        1,
        &["COMPLETE"],
    )
    .await;
    assert_eq!(
        member(&start_body, "State").as_str(),
        Some("COMPLETE"),
        "a deployment nobody stopped completes on its own, got {}",
        member(&start_body, "State")
    );
    let completed_at = member(&start_body, "CompletedAt");
    assert!(
        !completed_at.is_null(),
        "a completed deployment carries the instant it completed, got {completed_at}"
    );
    let log_at_start = event_types(&event_log("all at once", "GetDeployment", &start_body));

    // A stop that does not ask for a revert cannot touch a completed deployment, whether
    // the header is absent or explicitly false.
    for (case, allow_revert) in [
        ("no Allow-Revert header at all", None),
        ("Allow-Revert: false", Some(false)),
    ] {
        let refused = stop(&router, &application, &environment, 1, allow_revert).await;
        assert_bad_request(
            case,
            &refused,
            &[
                "Deployment 1",
                application.as_str(),
                environment.as_str(),
                "COMPLETE",
            ],
            &["COMPLETE"],
        );
        let read_back = get_deployment(&router, &application, &environment, 1).await.json();
        assert_eq!(
            member(&read_back, "State").as_str(),
            Some("COMPLETE"),
            "case {case:?}: a refused stop leaves the deployment where it was, got {}",
            member(&read_back, "State")
        );
        assert_eq!(
            event_types(&event_log(case, "GetDeployment", &read_back)),
            log_at_start,
            "case {case:?}: a refused stop appends no event"
        );
        let environment_now = environment_state(&router, &application, &environment).await;
        assert_eq!(
            environment_now.as_deref(),
            Some("ReadyForDeployment"),
            "case {case:?}: a refused stop leaves the environment where it was, got \
             {environment_now:?}"
        );
    }

    // With a revert allowed, the completed deployment is reverted.
    let case = "Allow-Revert: true";
    let reverted = stop(&router, &application, &environment, 1, Some(true)).await;
    assert_eq!(
        reverted.status,
        202,
        "case {case:?}: StopDeployment must answer the modelled 202, got {} / {}",
        reverted.status,
        String::from_utf8_lossy(&reverted.body)
    );
    let revert_body = reverted.json();
    assert_eq!(
        member(&revert_body, "State").as_str(),
        Some("REVERTED"),
        "case {case:?}: a reverted deployment reports REVERTED, got {}",
        member(&revert_body, "State")
    );
    assert_eq!(
        member(&revert_body, "CompletedAt"),
        completed_at,
        "case {case:?}: the revert is a later event of its own, so the instant the deployment \
         completed is unchanged, got {}",
        member(&revert_body, "CompletedAt")
    );
    let revert_log = event_types(&event_log(case, "StopDeployment", &revert_body));
    assert_eq!(
        revert_log.first().map(String::as_str),
        Some("REVERT_COMPLETED"),
        "case {case:?}: the newest event of a reverted deployment is its revert, got {revert_log:?}"
    );
    for name in IDENTITY_MEMBERS {
        assert_eq!(
            member(&revert_body, name),
            member(&start_body, name),
            "case {case:?}: a revert answers the same deployment, so its {name} is unchanged, \
             got {} then {}",
            member(&start_body, name),
            member(&revert_body, name)
        );
    }

    let read_back = get_deployment(&router, &application, &environment, 1).await.json();
    assert_eq!(
        member(&read_back, "State").as_str(),
        Some("REVERTED"),
        "case {case:?}: GetDeployment must report the state the revert wrote, got {}",
        member(&read_back, "State")
    );
    assert_eq!(
        event_types(&event_log(case, "GetDeployment", &read_back)),
        revert_log,
        "case {case:?}: GetDeployment must report the stored event log"
    );
    let environment_now = environment_state(&router, &application, &environment).await;
    assert_eq!(
        environment_now.as_deref(),
        Some("Reverted"),
        "case {case:?}: an environment whose latest deployment was reverted reports \
         \"Reverted\", got {environment_now:?}"
    );

    // Nothing left to revert.
    let again = stop(&router, &application, &environment, 1, Some(true)).await;
    assert_bad_request(
        "a second revert",
        &again,
        &[
            "Deployment 1",
            application.as_str(),
            environment.as_str(),
            "REVERTED",
        ],
        &["REVERTED"],
    );
}

/// A stop of something the account does not hold is a miss, at every level the request
/// names: the application, the environment under it, and the deployment number under
/// that.
#[tokio::test]
async fn stopping_a_deployment_that_does_not_exist_is_refused_as_a_miss() {
    let router = router();
    let application = create_application(&router, "stop-missing").await;
    let environment = create_environment(&router, &application, "empty").await;

    // Rows differ along WHICH part of the path resolves nothing. `Name` is len 1..64, so
    // every id below is inside the modelled bound.
    let rows: [(&str, &str, &str, i32); 4] = [
        ("an application nobody created", "zzzzzzz", "zzzzzzz", 1),
        ("an environment the application does not hold", &application, "zzzzzzz", 1),
        (
            "a deployment number the environment never issued",
            &application,
            &environment,
            1,
        ),
        (
            "a far higher deployment number",
            &application,
            &environment,
            4096,
        ),
    ];

    for (case, application_id, environment_id, number) in rows {
        let answer = stop(&router, application_id, environment_id, number, None).await;
        assert_not_found(case, &answer);
        let with_revert = stop(&router, application_id, environment_id, number, Some(true)).await;
        assert_not_found(&format!("{case}, allowing a revert"), &with_revert);
    }
}

/// A strategy with one row's pacing, for the clock-driven suites below.
fn strategy_of(duration: i32, growth_type: &str, growth_factor: f32, bake: i32) -> DeploymentStrategy {
    DeploymentStrategy {
        id: "stop001".to_string(),
        name: "stopped".to_string(),
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

/// A caller's stop SETTLES the rollback it performs: the record is `ROLLED_BACK` the moment
/// the stop returns, carrying the instant it finished and the rollback's completion event,
/// and no later read moves it or appends anything.
#[test]
fn a_stop_settles_the_rollback_it_performs() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Rows differ along the pacing of the deployment stopped — both published bounds of
    // `MinutesBetween0And24Hours` (range 0..1440) and both `GrowthType` values — and along
    // how far into the deployment the stop arrives, including at the instant it started.
    // Each row's instant is inside a stage its strategy declares: the rollout's intervals
    // are one short span each (`state::STAGE_DURATION`) and the bake keeps its declared
    // minutes (`state::bake_duration`).
    let rows: [(&str, i32, &str, f32, i32, u64); 5] = [
        ("stopped a second into the longest rollout", 1440, state::GROWTH_LINEAR, 100.0, 0, 1),
        ("stopped at the instant it started", 60, state::GROWTH_EXPONENTIAL, 1.0, 10, 0),
        ("stopped while baking", 0, state::GROWTH_LINEAR, 100.0, 1440, 4),
        ("stopped mid-rollout with the longest bake", 1440, state::GROWTH_LINEAR, 50.0, 1440, 2),
        ("stopped a second before a short rollout ends", 5, state::GROWTH_LINEAR, 10.0, 0, 2),
    ];

    for (case, duration, growth_type, growth_factor, bake, elapsed_secs) in rows {
        let strategy = strategy_of(duration, growth_type, growth_factor, bake);
        let mut deployment = Deployment::start(new_deployment(), &strategy, started_at);
        let stopped_at = started_at + Duration::from_secs(elapsed_secs);
        deployment.materialize(stopped_at);
        assert!(
            deployment.is_in_flight(),
            "case {case:?}: this row's strategy must leave the deployment in progress at the \
             stop, got {:?}",
            deployment.state
        );

        assert_eq!(
            deployment.stop(false, stopped_at),
            Ok(()),
            "case {case:?}: a deployment in progress can be stopped"
        );
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLED_BACK,
            "case {case:?}: a stop leaves the deployment rolled back, got {:?}",
            deployment.state
        );
        assert!(
            !deployment.is_active(),
            "case {case:?}: a stopped deployment is one nothing is working on any more, got {:?}",
            deployment.state
        );
        assert_eq!(
            deployment.completed_at, None,
            "case {case:?}: a deployment the stop rolled back never completed, so it holds \
             no completion instant, got {:?}",
            deployment.completed_at
        );
        assert_eq!(
            deployment.rollback_completion_due, None,
            "case {case:?}: nothing is due on a rollback that is already over, got {:?}",
            deployment.rollback_completion_due
        );

        let log = stored_event_types(&deployment);
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
        let newest = deployment
            .event_log_newest_first()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stop records an event"));
        assert_eq!(
            newest.description.as_deref(),
            Some(ROLLED_BACK_BY_USER),
            "case {case:?}: the rollback's event reports what initiated it, got {:?}",
            newest.description
        );
        assert_eq!(
            newest.triggered_by.as_deref(),
            Some(state::TRIGGERED_BY_USER),
            "case {case:?}: the request initiated the rollback, got {:?}",
            newest.triggered_by
        );

        // Reading it again owes nothing: the stop left nothing for the clock to finish.
        let settled = deployment.clone();
        deployment.materialize(stopped_at + Duration::from_secs(3600));
        assert_eq!(
            deployment.state, settled.state,
            "case {case:?}: a later read must not move a rolled-back deployment"
        );
        assert_eq!(
            deployment.event_log.len(),
            settled.event_log.len(),
            "case {case:?}: a later read must not append an event the clock did not owe"
        );

        // And a deployment the service has finished with cannot be stopped again, whatever
        // the request allows.
        for allow_revert in [false, true] {
            assert_eq!(
                deployment.stop(allow_revert, stopped_at + Duration::from_secs(1)),
                Err(StopRefusal::NotStoppable),
                "case {case:?}: a rolled-back deployment cannot be stopped again (AllowRevert \
                 {allow_revert})"
            );
            assert_eq!(
                deployment.state,
                state::DEPLOYMENT_ROLLED_BACK,
                "case {case:?}: a refused stop leaves the record exactly as it was, got {:?}",
                deployment.state
            );
        }
    }
}

/// The rollback AppConfig starts on its OWN initiative — an extension's abort directive,
/// not a caller's stop — is the one owed by the CLOCK: the record is `ROLLING_BACK` while it
/// runs, nothing finishes it early, and it reaches `ROLLED_BACK` at the instant that was
/// due, once however often it is read.
#[test]
fn the_clock_finishes_the_rollback_appconfig_starts() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // The same span of pacings and stop instants as the suite above: what differs is WHO
    // started the rollback, not the deployment it aborts.
    let rows: [(&str, i32, &str, f32, i32, u64); 5] = [
        ("aborted a second into the longest rollout", 1440, state::GROWTH_LINEAR, 100.0, 0, 1),
        ("aborted at the instant it started", 60, state::GROWTH_EXPONENTIAL, 1.0, 10, 0),
        ("aborted while baking", 0, state::GROWTH_LINEAR, 100.0, 1440, 4),
        ("aborted mid-rollout with the longest bake", 1440, state::GROWTH_LINEAR, 50.0, 1440, 2),
        ("aborted a second before a short rollout ends", 5, state::GROWTH_LINEAR, 10.0, 0, 2),
    ];

    for (case, duration, growth_type, growth_factor, bake, elapsed_secs) in rows {
        let strategy = strategy_of(duration, growth_type, growth_factor, bake);
        let mut deployment = Deployment::start(new_deployment(), &strategy, started_at);
        let aborted_at = started_at + Duration::from_secs(elapsed_secs);
        deployment.materialize(aborted_at);

        assert_eq!(
            deployment.begin_rollback(aborted_at),
            Ok(()),
            "case {case:?}: a deployment in progress can be rolled back"
        );
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLING_BACK,
            "case {case:?}: the rollback has started, got {:?}",
            deployment.state
        );
        assert!(
            deployment.is_active() && !deployment.is_in_flight(),
            "case {case:?}: a rolling-back deployment is still being worked on, but is no \
             longer rolling out, got {:?}",
            deployment.state
        );
        assert!(
            deployment.completed_at.is_none(),
            "case {case:?}: the rollback has not finished, so nothing completed, got {:?}",
            deployment.completed_at
        );
        let due = deployment.rollback_completion_due.unwrap_or_else(|| {
            panic!("case {case:?}: a rolling-back deployment records when its rollback falls due")
        });
        assert!(
            due > aborted_at,
            "case {case:?}: the rollback is never already over when it starts, got {due:?} due \
             at the abort instant {aborted_at:?}"
        );

        // Nothing before the due instant finishes it: a read is not a transition.
        let events_at_abort = deployment.event_log.len();
        deployment.materialize(due - Duration::from_millis(1));
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLING_BACK,
            "case {case:?}: a read before the rollback falls due must not finish it, got {:?}",
            deployment.state
        );
        assert_eq!(
            deployment.event_log.len(),
            events_at_abort,
            "case {case:?}: a read before the rollback falls due appends no event"
        );

        // At the due instant the clock owes ROLLED_BACK, with the rollback's own event.
        deployment.materialize(due);
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_ROLLED_BACK,
            "case {case:?}: the rollback finishes when it fell due, got {:?}",
            deployment.state
        );
        assert_eq!(
            deployment.completed_at, None,
            "case {case:?}: a rolled-back deployment never completed, so it holds no \
             completion instant, got {:?}",
            deployment.completed_at
        );
        let log = stored_event_types(&deployment);
        assert_eq!(
            log.first().map(String::as_str),
            Some(state::EVENT_ROLLBACK_COMPLETED),
            "case {case:?}: the newest event is the rollback finishing, got {log:?}"
        );
        assert!(
            log.iter()
                .any(|event| event == state::EVENT_ROLLBACK_STARTED),
            "case {case:?}: the log keeps the rollback's start, got {log:?}"
        );
        assert_eq!(
            log.last().map(String::as_str),
            Some(state::EVENT_DEPLOYMENT_STARTED),
            "case {case:?}: the oldest event is the deployment's start, got {log:?}"
        );

        // Reading it again owes nothing: the clock already gave it everything it had.
        let settled = deployment.clone();
        deployment.materialize(due + Duration::from_secs(3600));
        assert_eq!(
            deployment.state, settled.state,
            "case {case:?}: a later read must not move a rolled-back deployment"
        );
        assert_eq!(
            deployment.event_log.len(),
            settled.event_log.len(),
            "case {case:?}: a later read must not append an event the clock did not owe"
        );

        // And a rollback already finished cannot be started again.
        assert_eq!(
            deployment.begin_rollback(due + Duration::from_secs(1)),
            Err(StopRefusal::NotStoppable),
            "case {case:?}: a rolled-back deployment has nothing left to roll back"
        );
    }
}

/// A revert is allowed only inside the window the model documents on `StopDeployment` —
/// "AppConfig only allows a revert within 72 hours of deployment completion" — measured
/// from the instant the deployment completed, and a refused stop changes nothing.
#[test]
fn a_completed_deployment_is_reverted_only_inside_the_documented_window() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let window = state::REVERT_WINDOW;

    // Rows walk the window: inside it, on the documented bound, one second past it, and
    // far past it.
    let rows: [(&str, Duration, bool); 6] = [
        ("at the instant it completed", Duration::ZERO, true),
        ("an hour later", Duration::from_secs(3600), true),
        ("a second inside the bound", window - Duration::from_secs(1), true),
        ("exactly on the documented bound", window, true),
        ("a second past the bound", window + Duration::from_secs(1), false),
        ("far past the bound", window * 2, false),
    ];

    for (case, elapsed, inside_window) in rows {
        // No rollout window and no bake time, so the deployment completes one growth
        // interval after it started and the window is measured from an instant this test
        // knows.
        let strategy = strategy_of(0, state::GROWTH_LINEAR, 100.0, 0);
        let mut deployment = Deployment::start(new_deployment(), &strategy, started_at);
        deployment.materialize(started_at + state::STAGE_DURATION);
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
        // whenever it arrives.
        let mut without_revert = deployment.clone();
        assert_eq!(
            without_revert.stop(false, at),
            Err(StopRefusal::NotStoppable),
            "case {case:?}: a completed deployment can only be reverted, not stopped"
        );
        assert_eq!(
            without_revert.state,
            state::DEPLOYMENT_COMPLETE,
            "case {case:?}: a refused stop leaves the record exactly as it was, got {:?}",
            without_revert.state
        );
        assert_eq!(
            without_revert.event_log.len(),
            events_before,
            "case {case:?}: a refused stop appends no event"
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
            assert!(
                !deployment.is_active(),
                "case {case:?}: a reverted deployment is one nothing is working on any more"
            );
            let log = stored_event_types(&deployment);
            assert_eq!(
                log.first().map(String::as_str),
                Some(state::EVENT_REVERT_COMPLETED),
                "case {case:?}: the newest event of a reverted deployment is its revert, got \
                 {log:?}"
            );
            assert_eq!(
                deployment.stop(true, at),
                Err(StopRefusal::NotStoppable),
                "case {case:?}: a reverted deployment has nothing left to revert"
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

/// An environment with nothing deployed into it, for the readiness suite below. Its ids
/// are the ones [`new_deployment`] names, so the deployment recorded into it belongs to
/// it.
fn empty_environment() -> Environment {
    let new = new_deployment();
    Environment::new(
        new.environment_id,
        new.application_id,
        "deployed-into".to_string(),
        None,
        Vec::new(),
        HashMap::new(),
    )
}

/// An environment's reported state, and whether it is still working on a deployment,
/// follow the deployment it holds through the whole life of a stop: `DEPLOYING` while the
/// deployment rolls out or bakes, and `ROLLED_BACK` once a caller's stop has settled the
/// rollback — at which point the environment has nothing in flight and takes a new
/// deployment again. A rollback AppConfig starts on its own initiative is the one the
/// environment reports as `ROLLING_BACK`, until the clock finishes it.
///
/// Asserted against the record, at instants this test chooses, so the clock-owed half is
/// deterministic rather than a wall-clock race.
#[test]
fn an_environments_state_and_readiness_follow_the_deployment_it_holds() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Rows differ along the pacing of the deployment the environment holds, which is what
    // decides the state it is in when the stop arrives — none of which the environment's
    // own progression depends on — and along WHO ends the deployment: a caller's stop,
    // which settles it, or AppConfig's own rollback, which the clock finishes.
    let rows: [(&str, i32, i32, bool); 6] = [
        ("the longest rollout the model allows, stopped", 1440, 0, true),
        ("no rollout window, so it is baking, stopped", 0, 1440, true),
        ("a short rollout with a bake time, stopped", 5, 10, true),
        ("the longest rollout the model allows, aborted", 1440, 0, false),
        ("no rollout window, so it is baking, aborted", 0, 1440, false),
        ("a short rollout with a bake time, aborted", 5, 10, false),
    ];

    for (case, duration, bake, by_stop) in rows {
        let strategy = strategy_of(duration, state::GROWTH_LINEAR, 100.0, bake);
        let mut environment = empty_environment();
        assert_eq!(
            environment.state,
            state::ENVIRONMENT_READY_FOR_DEPLOYMENT,
            "case {case:?}: an environment with no deployment is ready for one, got {:?}",
            environment.state
        );

        environment.record_deployment(Deployment::start(new_deployment(), &strategy, started_at));
        environment.materialize(started_at);
        assert_eq!(
            environment.state,
            state::ENVIRONMENT_DEPLOYING,
            "case {case:?}: an environment with a deployment in flight is deploying, got {:?}",
            environment.state
        );
        assert!(
            environment.active_deployment().is_some(),
            "case {case:?}: the environment is working on that deployment, so a second one \
             conflicts with it"
        );

        let number = new_deployment().deployment_number;
        let ended = environment
            .deployments
            .get_mut(&number)
            .unwrap_or_else(|| panic!("case {case:?}: the recorded deployment is readable back"));
        let due = if by_stop {
            assert_eq!(
                ended.stop(false, started_at),
                Ok(()),
                "case {case:?}: a deployment in progress can be stopped"
            );
            None
        } else {
            assert_eq!(
                ended.begin_rollback(started_at),
                Ok(()),
                "case {case:?}: a deployment in progress can be rolled back"
            );
            Some(ended.rollback_completion_due.unwrap_or_else(|| {
                panic!("case {case:?}: a rolling-back deployment records when it falls due")
            }))
        };

        environment.materialize(started_at);
        if let Some(due) = due {
            assert_eq!(
                environment.state,
                state::ENVIRONMENT_ROLLING_BACK,
                "case {case:?}: an environment whose deployment AppConfig is rolling back \
                 reports it, got {:?}",
                environment.state
            );
            assert!(
                environment.active_deployment().is_some(),
                "case {case:?}: the environment is still working on that rollback, so it takes \
                 no new deployment yet"
            );
            environment.materialize(due);
        }

        assert_eq!(
            environment.state,
            state::ENVIRONMENT_ROLLED_BACK,
            "case {case:?}: once the rollback has finished the environment reports it, got {:?}",
            environment.state
        );
        assert!(
            environment.active_deployment().is_none(),
            "case {case:?}: nothing is in flight any more, so the environment takes a new \
             deployment"
        );
    }
}

/// An environment whose latest deployment was REVERTED reports that, and is ready for a
/// new deployment at once — a revert is terminal, so nothing is left in flight.
#[test]
fn an_environment_reports_the_revert_of_the_deployment_it_holds() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // No rollout window and no bake time, so the deployment completes one growth interval
    // after it started, which is the state a revert applies to.
    let strategy = strategy_of(0, state::GROWTH_LINEAR, 100.0, 0);
    let mut environment = empty_environment();
    environment.record_deployment(Deployment::start(new_deployment(), &strategy, started_at));
    let completed_at = started_at + state::STAGE_DURATION;

    environment.materialize(completed_at);
    assert_eq!(
        environment.state,
        state::ENVIRONMENT_READY_FOR_DEPLOYMENT,
        "a completed deployment leaves the environment ready for the next one, got {:?}",
        environment.state
    );

    let number = new_deployment().deployment_number;
    let reverted = environment
        .deployments
        .get_mut(&number)
        .expect("the recorded deployment is readable back");
    assert_eq!(
        reverted.stop(true, completed_at),
        Ok(()),
        "a completed deployment can be reverted by a request that allows it"
    );

    environment.materialize(completed_at);
    assert_eq!(
        environment.state,
        state::ENVIRONMENT_REVERTED,
        "an environment whose latest deployment was reverted reports it, got {:?}",
        environment.state
    );
    assert!(
        environment.active_deployment().is_none(),
        "a revert is terminal, so the environment has nothing in flight"
    );
}
