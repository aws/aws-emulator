// SPDX-License-Identifier: CC0-1.0

//! The final bake time a strategy declares is a WINDOW the deployment is held NON-TERMINAL
//! in, and only the clock moves it out of that window.
//!
//! RULE: `FinalBakeTimeInMinutes` is "the amount of time that AppConfig monitors for Amazon
//! CloudWatch alarms before considering the deployment to be complete and no longer eligible
//! for automatic rollback" (`smithy_query(query="shape",shape="DeploymentStrategy")`), so
//! once a deployment's rollout has reached every target the deployment is held in the
//! `DeploymentState` `BAKING` for the minutes the strategy declared: it is not terminal
//! while they run, so reading it reports that window instead of consuming it — consecutive
//! `GetDeployment` calls inside the window all report `BAKING`, at 100% of targets, with no
//! `CompletedAt` and with nothing added to the event log — and it becomes `COMPLETE` only
//! once the declared window has elapsed. A `StopDeployment` arriving inside the window is
//! therefore a stop of a deployment AppConfig is still working on: it is accepted (the
//! operation's modelled 202) and settles the deployment `ROLLED_BACK`, available to no
//! target, with the `BAKE_TIME_STARTED` entry the bake wrote still on its log, after which
//! the now-terminal deployment refuses a second stop with the modelled `BadRequestException`
//! (HTTP 400) naming the deployment, the application, the environment, the verb the caller
//! asked for and the state it refused
//! (`smithy_query(query="operation",operation="GetDeployment,StopDeployment")`). A strategy
//! that declares no bake time declares no such window and never enters `BAKING` at all.
//!
//! The rows span what the rule generalises over: the declared bake minutes — the shortest
//! the model's `MinutesBetween0And24Hours` (`range 0..1440`) allows, a value between the
//! bounds, the published maximum, and the 0 that is the member's other bound and its default
//! — and the rollout the window follows (none declared at all, and rollouts of both
//! `GrowthType` values whose growth factors take a different number of intervals to reach
//! every target). What the stop request says about a revert differs row by row too, because
//! it is the one part of the refusal that moves with the request.
//!
//! Every wait here is on the CONDITION a transition makes true, re-read until it holds; the
//! end of a declared window is reached by moving the instant the deployment started BACK —
//! driving the clock the transition is owed by — so nothing waits out the declared minutes
//! and no assertion is about how long anything took.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, ComAmazonawsAppconfigAmazonappconfigState,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, deployment_in_state, router, router_on,
};
use crate::deployment_wire_fidelity::{
    create_strategy, event_log, event_types, host_version, member, Row,
};
use crate::stop_deployment::{get_deployment, start, stop};

/// The strategies whose declared bake time this suite holds a deployment in: the published
/// bounds of `MinutesBetween0And24Hours` above 0 and a value between them, each after a
/// different rollout.
const BAKE_ROWS: [Row; 4] = [
    (
        "the shortest declared bake, with no rollout window",
        0,
        100.0,
        "LINEAR",
        1,
    ),
    (
        "a two-minute bake after a linear rollout in halves",
        5,
        50.0,
        "LINEAR",
        2,
    ),
    (
        "an hour-long bake after an exponential rollout",
        20,
        25.0,
        "EXPONENTIAL",
        60,
    ),
    (
        "the longest declared bake the model allows",
        1440,
        100.0,
        "LINEAR",
        1440,
    ),
];

/// A strategy at the OTHER bound of `FinalBakeTimeInMinutes`, which is also its default: no
/// bake time, so no window to hold anything in.
const NO_BAKE_ROW: Row = ("no declared bake at all", 1, 100.0, "LINEAR", 0);

/// How many times in a row a deployment inside its window is read, to show that reading
/// reports the window rather than consuming it. More than two, so a reader that advanced the
/// record only on its second call would be caught as well.
const READS_INSIDE_THE_WINDOW: usize = 3;

/// The event a bake writes when it begins, and the one a completion writes.
const BAKE_TIME_STARTED: &str = "BAKE_TIME_STARTED";
/// See [`BAKE_TIME_STARTED`].
const DEPLOYMENT_COMPLETED: &str = "DEPLOYMENT_COMPLETED";
/// The event a stop of a deployment still in flight writes.
const ROLLBACK_COMPLETED: &str = "ROLLBACK_COMPLETED";

/// An application with one hosted configuration version to deploy, answering
/// `(application_id, profile_id)`.
async fn given_a_configuration(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let application = create_application(router, name).await;
    let profile = create_profile(router, &application, &format!("{name}-profile")).await;
    host_version(router, &application, &profile).await;
    (application, profile)
}

/// The `DeploymentNumber` a start minted.
fn number_of(case: &str, body: &Value) -> i32 {
    let number = member(body, "DeploymentNumber").as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: StartDeployment answered no DeploymentNumber, got {body}")
    });
    i32::try_from(number).unwrap_or_else(|_| {
        panic!("case {case:?}: a minted DeploymentNumber is an Integer, got {number}")
    })
}

/// Start one deployment of version 1 of `profile` into `environment`, paced by `strategy`,
/// and answer the number it minted.
async fn started_deployment(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    strategy: &str,
    profile: &str,
) -> i32 {
    let answer = start(router, application, environment, strategy, profile).await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: StartDeployment must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    number_of(case, &answer.json())
}

/// `GetDeployment` of one deployment, asserted to answer 200.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    number: i32,
) -> Value {
    let answer = get_deployment(router, application, environment, number).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetDeployment must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// The `EventType` of every entry of a document's event log, newest first.
fn logged(case: &str, document: &str, body: &Value) -> Vec<String> {
    event_types(&event_log(case, document, body))
}

/// Everything the rule requires of a deployment the declared window still holds: the
/// non-terminal `BAKING`, every target reached by the rollout the window follows, no
/// completion instant, and a log that carries the bake's own event and not the completion
/// that window has not reached yet.
fn assert_inside_the_window(case: &str, label: &str, document: &Value) {
    assert_eq!(
        member(document, "State").as_str(),
        Some("BAKING"),
        "case {case:?} ({label}): the declared bake time holds the deployment in the \
         non-terminal BAKING, got {document}"
    );
    assert_eq!(
        member(document, "PercentageComplete").as_f64(),
        Some(100.0),
        "case {case:?} ({label}): the bake follows a rollout that reached every target, got \
         {document}"
    );
    assert!(
        member(document, "CompletedAt").is_null(),
        "case {case:?} ({label}): a deployment AppConfig is still monitoring has not \
         completed, got {document}"
    );
    let events = logged(case, "GetDeployment", document);
    assert!(
        events.iter().any(|event| event == BAKE_TIME_STARTED),
        "case {case:?} ({label}): the bake begins with its own event, got {events:?}"
    );
    assert!(
        !events.iter().any(|event| event == DEPLOYMENT_COMPLETED),
        "case {case:?} ({label}): a deployment inside its bake window has not completed, got \
         {events:?}"
    );
}

/// Bring the end of a deployment's declared window `by` closer, by moving the instant it
/// started back.
///
/// The declared minutes are the window a caller asked AppConfig to monitor the deployment
/// for, so a row about the END of that window drives the clock the transition is owed by
/// instead of waiting the minutes out. Nothing here moves a state, writes an event or
/// touches any other record: the transition itself stays the service's.
fn bring_the_window_forward(
    shared: &Arc<engine::InMemoryEngine>,
    case: &str,
    application_id: &str,
    environment_id: &str,
    number: i32,
    by: Duration,
) {
    let held = shared.state::<ComAmazonawsAppconfigAmazonappconfigState>();
    let mut store = held.lock();
    let deployment = store
        .application_mut(application_id)
        .and_then(|application| application.environments.get_mut(environment_id))
        .and_then(|environment| environment.deployments.get_mut(&number))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the environment must hold the deployment {number} that was \
                 started into it"
            )
        });
    deployment.started_at -= by;
}

/// RULE: inside the declared window the deployment reports `BAKING` however often it is
/// read, the window lasts the minutes that were declared, and only its elapsing completes
/// the deployment.
#[tokio::test]
async fn reads_inside_the_declared_bake_window_report_baking_without_consuming_it() {
    // The engine behind the router, because reaching the END of a declared window means
    // driving the clock that window is measured on rather than waiting it out.
    let shared: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&shared);
    let (application, profile) = given_a_configuration(&router, "bake-window").await;

    for (index, row) in BAKE_ROWS.iter().enumerate() {
        let (case, _duration, _growth_factor, _growth_type, bake) = *row;
        let strategy = create_strategy(&router, index, row).await;
        // One environment per row: an environment holds one deployment at a time.
        let environment =
            create_environment(&router, &application, &format!("bake-window-{index}")).await;
        let number = started_deployment(
            &router,
            case,
            &application,
            &environment,
            &strategy,
            &profile,
        )
        .await;

        // The bake begins on its own, once the rollout has reached every target.
        let first =
            deployment_in_state(&router, case, &application, &environment, i64::from(number), &[
                "BAKING",
            ])
            .await;
        assert_inside_the_window(case, "the read that found the window", &first);
        let log_at_the_first_read = logged(case, "GetDeployment", &first);

        // Reading is REPORTING: every read inside the window answers the same window, and
        // none of them writes an event or moves the deployment on.
        for round in 1..=READS_INSIDE_THE_WINDOW {
            let again = read(&router, case, &application, &environment, number).await;
            let label = format!("read {round} of {READS_INSIDE_THE_WINDOW} inside the window");
            assert_inside_the_window(case, &label, &again);
            assert_eq!(
                logged(case, "GetDeployment", &again),
                log_at_the_first_read,
                "case {case:?} ({label}): reading a deployment logs nothing, got {again}"
            );
        }

        // Half of the declared window is behind the deployment now, and that half is the
        // declared minutes' own half — so a deployment still reported BAKING here is one
        // whose window is as long as the strategy declared rather than a span of its own.
        // The half still to run is the room this read has, and it is minutes, not
        // milliseconds.
        let declared = state::bake_duration(bake);
        bring_the_window_forward(
            &shared,
            case,
            &application,
            &environment,
            number,
            declared / 2,
        );
        let half_way = read(&router, case, &application, &environment, number).await;
        assert_inside_the_window(case, "half of the declared window elapsed", &half_way);

        // And once the whole declared window has elapsed, the deployment completes without
        // anybody stopping it.
        bring_the_window_forward(&shared, case, &application, &environment, number, declared);
        let completed =
            deployment_in_state(&router, case, &application, &environment, i64::from(number), &[
                "COMPLETE",
            ])
            .await;
        assert_eq!(
            member(&completed, "PercentageComplete").as_f64(),
            Some(100.0),
            "case {case:?}: a completed deployment reached every target, got {completed}"
        );
        assert!(
            !member(&completed, "CompletedAt").is_null(),
            "case {case:?}: a completed deployment reports when it completed, got {completed}"
        );
        let events = logged(case, "GetDeployment", &completed);
        assert_eq!(
            events.first().map(String::as_str),
            Some(DEPLOYMENT_COMPLETED),
            "case {case:?}: the newest event of a deployment whose window elapsed is its \
             completion, got {events:?}"
        );
        assert!(
            events.iter().any(|event| event == BAKE_TIME_STARTED),
            "case {case:?}: the bake it went through stays on the log, got {events:?}"
        );
    }
}

/// RULE: a stop inside the declared window is accepted and settles the deployment
/// `ROLLED_BACK`; the terminal deployment then refuses a second stop with the modelled
/// `BadRequestException`.
#[tokio::test]
async fn a_stop_inside_the_declared_bake_window_rolls_the_deployment_back() {
    let router = router();
    let (application, profile) = given_a_configuration(&router, "bake-stop").await;

    for (index, row) in BAKE_ROWS.iter().enumerate() {
        let (case, ..) = *row;
        // What the second stop asks for differs row by row: `AllowRevert` is the one part of
        // the refusal that moves with the request, and a stop that carries it is asking for
        // a revert instead.
        let allow_revert = if index % 2 == 0 { None } else { Some(true) };
        let verb = if allow_revert == Some(true) {
            "reverted"
        } else {
            "stopped"
        };
        let strategy = create_strategy(&router, index, row).await;
        let environment =
            create_environment(&router, &application, &format!("bake-stop-{index}")).await;
        let number = started_deployment(
            &router,
            case,
            &application,
            &environment,
            &strategy,
            &profile,
        )
        .await;
        let baking =
            deployment_in_state(&router, case, &application, &environment, i64::from(number), &[
                "BAKING",
            ])
            .await;
        assert_inside_the_window(case, "the read before the stop", &baking);

        let stopped = stop(&router, &application, &environment, number, None).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: a stop inside the bake window is accepted with the operation's \
             modelled 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let settled = stopped.json();
        assert_eq!(
            member(&settled, "State").as_str(),
            Some("ROLLED_BACK"),
            "case {case:?}: the stop settles the rollback it performed, got {settled}"
        );
        assert_eq!(
            member(&settled, "PercentageComplete").as_f64(),
            Some(0.0),
            "case {case:?}: a rolled-back deployment is available to no target, got {settled}"
        );
        assert!(
            member(&settled, "CompletedAt").is_null(),
            "case {case:?}: a deployment the stop rolled back never completed, got {settled}"
        );
        let from_the_stop = logged(case, "StopDeployment", &settled);
        assert_eq!(
            from_the_stop.first().map(String::as_str),
            Some(ROLLBACK_COMPLETED),
            "case {case:?}: the newest event is the rollback the request asked for, got \
             {from_the_stop:?}"
        );
        assert!(
            from_the_stop.iter().any(|event| event == BAKE_TIME_STARTED),
            "case {case:?}: the bake the deployment was stopped during stays on the log, got \
             {from_the_stop:?}"
        );
        assert!(
            !from_the_stop
                .iter()
                .any(|event| event == DEPLOYMENT_COMPLETED),
            "case {case:?}: a deployment stopped inside its window never completed, got \
             {from_the_stop:?}"
        );

        // The reader answers the same outcome: the stop left nothing for a later caller.
        let read_back = read(&router, case, &application, &environment, number).await;
        assert_eq!(
            member(&read_back, "State").as_str(),
            Some("ROLLED_BACK"),
            "case {case:?}: nothing is left for a reader to finish, got {read_back}"
        );
        assert_eq!(
            logged(case, "GetDeployment", &read_back),
            from_the_stop,
            "case {case:?}: GetDeployment answers the log the stop wrote, got {read_back}"
        );

        // And the terminal deployment refuses a second stop, in the words AppConfig words
        // that refusal in: every value in the sentence comes from the request or the record.
        let again = stop(&router, &application, &environment, number, allow_revert).await;
        assert_eq!(
            again.status,
            400,
            "case {case:?}: a second stop of a terminal deployment is refused with the \
             modelled BadRequestException's HTTP 400, got {} / {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert!(
            again.error_code().contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            again.error_code()
        );
        let expected = format!(
            "Deployment {number} for App {application} and Environment {environment} cannot \
             be {verb}, it has a status of ROLLED_BACK."
        );
        assert_eq!(
            again.string("Message").unwrap_or_default(),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            again.string("Message")
        );
    }
}

/// RULE: a strategy that declares no bake time declares no window — such a deployment never
/// enters `BAKING` and logs no bake event on its way to `COMPLETE`.
#[tokio::test]
async fn a_strategy_with_no_declared_bake_has_no_window_to_hold_anything_in() {
    let router = router();
    let (application, profile) = given_a_configuration(&router, "no-bake").await;

    let (case, _duration, _growth_factor, _growth_type, bake) = NO_BAKE_ROW;
    let strategy = create_strategy(&router, 0, &NO_BAKE_ROW).await;
    let environment = create_environment(&router, &application, "no-bake-env").await;
    let number = started_deployment(
        &router,
        case,
        &application,
        &environment,
        &strategy,
        &profile,
    )
    .await;

    let completed =
        deployment_in_state(&router, case, &application, &environment, i64::from(number), &[
            "COMPLETE",
        ])
        .await;
    assert_eq!(
        member(&completed, "FinalBakeTimeInMinutes").as_i64(),
        Some(i64::from(bake)),
        "case {case:?}: the deployment reports the bake time its strategy declared, got \
         {completed}"
    );
    let events = logged(case, "GetDeployment", &completed);
    assert!(
        !events.iter().any(|event| event == BAKE_TIME_STARTED),
        "case {case:?}: a strategy with no declared bake time monitors nothing, so no bake \
         ever begins, got {events:?}"
    );
    assert_eq!(
        events.first().map(String::as_str),
        Some(DEPLOYMENT_COMPLETED),
        "case {case:?}: such a deployment goes from its rollout straight to completion, got \
         {events:?}"
    );

    // A stop of the terminal deployment is refused, naming the state it is in — the same
    // refusal a deployment that went through a bake window answers once that window is over.
    let refused = stop(&router, &application, &environment, number, None).await;
    assert_eq!(
        refused.status,
        400,
        "case {case:?}: a completed deployment cannot be stopped, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    let expected = format!(
        "Deployment {number} for App {application} and Environment {environment} cannot be \
         stopped, it has a status of COMPLETE."
    );
    assert_eq!(
        refused.string("Message").unwrap_or_default(),
        expected,
        "case {case:?}: expected the refusal {expected:?}, got {:?}",
        refused.string("Message")
    );
}
