// SPDX-License-Identifier: CC0-1.0

//! WHICH PHASE a deployment is in, and WHEN each phase's transition happens.
//!
//! The rules under test, each one level more general than the recorded request that
//! exposed them:
//!
//! 1. `StartDeployment` answers the deployment at the phase AppConfig has actually reached
//!    by the time it answers. A strategy that paces a stage — a rollout window to grow over
//!    or a final bake time to monitor — leaves every later phase to AppConfig, so the call
//!    answers `DEPLOYING` with `PercentageComplete` 0 and one USER-triggered
//!    `DEPLOYMENT_STARTED` event; a strategy that paces NEITHER leaves nothing to pace or
//!    monitor, so the call answers the deployment already `COMPLETE`.
//! 2. Reaching a share of the targets and entering the final bake are AppConfig's own
//!    transitions, each logged as its own event: `PERCENTAGE_UPDATED` naming the share
//!    reached, then `BAKE_TIME_STARTED` naming no duration, both `TriggeredBy` `APPCONFIG`,
//!    and only then is the deployment `BAKING`.
//! 3. A second deployment into an environment that still holds one is refused
//!    `ConflictException` (HTTP 409) naming the ENVIRONMENT's own state — the
//!    `EnvironmentState` member `DEPLOYING`, which is the one state that enum has for a
//!    deployment in flight, whether that deployment is rolling out or baking.
//! 4. A caller's stop settles the rollback inside the call, and the event log it answers —
//!    which `GetDeployment` answers again — keeps every phase the deployment passed
//!    through, newest first, with `ActionInvocations` on EVERY event: a list, empty when no
//!    extension action fired for it.
//!
//! The rows span what the rules generalise over: whether the strategy declares a rollout
//! window, whether it declares a final bake time, the magnitude of each (including both
//! published bounds of `MinutesBetween0And24Hours`, `range 0..1440`), the growth factor and
//! both `GrowthType` values — so the share each event names differs row by row — and which
//! phase the second deployment and the stop arrive in. Every assertion names its case and
//! prints the document it read.
//!
//! Every wait here is on the CONDITION a phase transition makes true, re-read until it
//! holds: no assertion is about how long a phase took, and no row is pinned to a log a
//! later interval of its own rollout could still add to.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, deployment_in_state, json_call, router,
    send, Answer,
};
use crate::deployment_wire_fidelity::{create_strategy, event_log, host_version, member, Row};

/// The `TriggeredBy` values the events of these phases carry
/// (`smithy_query(query="shape",shape="TriggeredBy")`: `USER | APPCONFIG | CLOUDWATCH_ALARM
/// | INTERNAL_ERROR`): what the caller's own request initiated, and what AppConfig did on
/// its own.
const BY_USER: &str = "USER";
/// See [`BY_USER`].
const BY_APPCONFIG: &str = "APPCONFIG";

/// One entry of an event log, as this suite compares them: its `EventType`, its
/// `TriggeredBy` and its `Description`.
type Entry = (String, String, String);

/// Start one deployment of version 1 of `profile` into `environment`, paced by `strategy`.
async fn start(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
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

/// `StopDeployment` of one deployment, asking for no revert.
async fn stop(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// `GetDeployment` of one deployment.
async fn read(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) -> Value {
    let answer = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "GetDeployment must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// The `DeploymentNumber` a start minted.
fn number_of(case: &str, body: &Value) -> i64 {
    member(body, "DeploymentNumber").as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: StartDeployment answered no DeploymentNumber, got {body}")
    })
}

/// Every entry of a deployment's event log, in the order the document reports them — newest
/// first — asserting on the way that each one carries `ActionInvocations` as a LIST.
fn logged(case: &str, document: &str, body: &Value) -> Vec<Entry> {
    event_log(case, document, body)
        .iter()
        .map(|event| {
            // Rule 4: "the list of extensions that were invoked as part of the deployment"
            // is a list on every event, whichever phase logged it, and empty when no
            // extension action fired for that phase.
            let invocations = member(event, "ActionInvocations");
            assert!(
                invocations.is_array(),
                "case {case:?}: {document} must report ActionInvocations as a list on every \
                 event, got {invocations} on {event}"
            );
            (
                member(event, "EventType")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                member(event, "TriggeredBy")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                member(event, "Description")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// One expected entry, spelled as the rows spell it.
fn entry(event_type: &str, triggered_by: &str, description: &str) -> Entry {
    (
        event_type.to_string(),
        triggered_by.to_string(),
        description.to_string(),
    )
}

/// The sentence AppConfig logs when a rollout reaches `share` percent of its targets.
///
/// The share is spelled with its fraction, as the recorded service does: a rollout that has
/// reached every target reads `100.00%`.
fn available_to(share: f64) -> String {
    format!("Configuration available to {share:.2}% of clients")
}

/// The entry the start of every deployment logs.
fn start_entry() -> Entry {
    entry("DEPLOYMENT_STARTED", BY_USER, "Deployment started")
}

/// An application with one hosted configuration version to deploy, answering
/// `(application_id, profile_id)`.
async fn given_a_configuration(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let application = create_application(router, name).await;
    let profile = create_profile(router, &application, &format!("{name}-profile")).await;
    host_version(router, &application, &profile).await;
    (application, profile)
}

/// RULE 1: the start answers the phase AppConfig has reached — the first one for every
/// strategy that paces a stage, and the terminal one for a strategy that paces none.
#[tokio::test]
async fn a_start_answers_the_phase_appconfig_has_reached() {
    let router = router();
    let (application, profile) = given_a_configuration(&router, "phase-at-start").await;

    // Rows span the two declared windows and their magnitudes, both growth types and the
    // published bounds of the growth factor.
    let rows: [Row; 7] = [
        ("no window and no bake time", 0, 100.0, "LINEAR", 0),
        (
            "no window and no bake time, exponential from the smallest share",
            0,
            1.0,
            "EXPONENTIAL",
            0,
        ),
        ("a bake time and no rollout window", 0, 100.0, "LINEAR", 10),
        ("a bake time at the published maximum", 0, 100.0, "LINEAR", 1440),
        ("a rollout window and no bake time", 5, 20.0, "LINEAR", 0),
        ("a rollout window and a bake time", 20, 10.0, "EXPONENTIAL", 10),
        ("both windows at the published maximum", 1440, 1.0, "LINEAR", 1440),
    ];

    for (index, row) in rows.iter().enumerate() {
        let (case, duration, _growth_factor, _growth_type, bake) = *row;
        let strategy = create_strategy(&router, index, row).await;
        // One environment per row: an environment holds one deployment at a time.
        let environment =
            create_environment(&router, &application, &format!("phase-env-{index}")).await;
        let started = start(&router, &application, &environment, &profile, &strategy).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let body = started.json();

        if duration == 0 && bake == 0 {
            // Nothing to pace and nothing to monitor: the deployment is over.
            assert_eq!(
                member(&body, "State").as_str(),
                Some("COMPLETE"),
                "case {case:?}: a strategy that paces no stage leaves the deployment over \
                 when the start answers, got {body}"
            );
            assert_eq!(
                member(&body, "PercentageComplete").as_f64(),
                Some(100.0),
                "case {case:?}: a deployment that is over reached every target, got {body}"
            );
            assert!(
                !member(&body, "CompletedAt").is_null(),
                "case {case:?}: a completed deployment reports when it completed, got {body}"
            );
            let log = logged(case, "StartDeployment", &body);
            assert_eq!(
                log.first().map(|(event_type, _, _)| event_type.as_str()),
                Some("DEPLOYMENT_COMPLETED"),
                "case {case:?}: the newest event of a completed deployment is its completion, \
                 got {log:?}"
            );
            assert_eq!(
                log.last().cloned(),
                Some(start_entry()),
                "case {case:?}: the oldest event is the deployment's start, got {log:?}"
            );
            continue;
        }

        assert_eq!(
            member(&body, "State").as_str(),
            Some("DEPLOYING"),
            "case {case:?}: a strategy that paces a stage leaves every later phase to \
             AppConfig, so the start answers the first one, got {body}"
        );
        assert_eq!(
            member(&body, "PercentageComplete").as_f64(),
            Some(0.0),
            "case {case:?}: nothing has rolled out at the instant the deployment was asked \
             for, got {body}"
        );
        assert!(
            member(&body, "CompletedAt").is_null(),
            "case {case:?}: a deployment still in flight has not completed, got {body}"
        );
        assert_eq!(
            logged(case, "StartDeployment", &body),
            vec![start_entry()],
            "case {case:?}: the start logs exactly its own USER-triggered event, got {body}"
        );
    }
}

/// RULE 2: the share reached and the bake are AppConfig's own transitions, each logged as
/// its own event, and only then is the deployment `BAKING`.
#[tokio::test]
async fn the_share_reached_and_the_bake_are_appconfigs_own_transitions() {
    let router = router();
    let (application, profile) = given_a_configuration(&router, "phase-bake").await;

    // Rows span the share each growth interval adds — which is the share the oldest
    // `PERCENTAGE_UPDATED` event names — and the declared bake time, which the bake event
    // must NOT spell out.
    let rows: [(Row, f64); 4] = [
        (
            ("every target in one interval", 0, 100.0, "LINEAR", 10),
            100.0,
        ),
        (
            (
                "every target in one interval, with the longest bake",
                0,
                100.0,
                "LINEAR",
                1440,
            ),
            100.0,
        ),
        (
            ("half the fleet per interval", 1, 50.0, "LINEAR", 10),
            50.0,
        ),
        (
            ("a fractional share per interval", 5, 33.3, "LINEAR", 1),
            33.3,
        ),
    ];

    for (index, (row, first_share)) in rows.iter().enumerate() {
        let (case, ..) = *row;
        let strategy = create_strategy(&router, index, row).await;
        let environment =
            create_environment(&router, &application, &format!("bake-env-{index}")).await;
        let started = start(&router, &application, &environment, &profile, &strategy).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let number = number_of(case, &started.json());

        // The bake begins on its own: the suite re-reads until it has.
        let baking =
            deployment_in_state(&router, case, &application, &environment, number, &["BAKING"])
                .await;
        assert_eq!(
            member(&baking, "PercentageComplete").as_f64(),
            Some(100.0),
            "case {case:?}: the bake follows the rollout, so every target has been reached \
             by it, got {baking}"
        );
        assert!(
            member(&baking, "CompletedAt").is_null(),
            "case {case:?}: a deployment monitoring its bake time has not completed, got \
             {baking}"
        );

        let log = logged(case, "GetDeployment", &baking);
        assert_eq!(
            log.first().cloned(),
            Some(entry(
                "BAKE_TIME_STARTED",
                BY_APPCONFIG,
                "Deployment bake time started"
            )),
            "case {case:?}: AppConfig's own bake event is the newest, and it names no \
             duration, got {log:?}"
        );
        assert_eq!(
            log.last().cloned(),
            Some(start_entry()),
            "case {case:?}: the oldest event is still the deployment's start, got {log:?}"
        );
        // Every share the rollout passed through is an APPCONFIG-triggered event of its
        // own: the oldest of them names the first interval's share and the newest names
        // every target reached.
        let shares: Vec<Entry> = log
            .iter()
            .filter(|(event_type, _, _)| event_type == "PERCENTAGE_UPDATED")
            .cloned()
            .collect();
        assert_eq!(
            shares.last().cloned(),
            Some(entry(
                "PERCENTAGE_UPDATED",
                BY_APPCONFIG,
                &available_to(*first_share)
            )),
            "case {case:?}: the first interval's event names the share it reached, got {log:?}"
        );
        assert_eq!(
            shares.first().cloned(),
            Some(entry(
                "PERCENTAGE_UPDATED",
                BY_APPCONFIG,
                &available_to(100.0)
            )),
            "case {case:?}: the last interval's event names every target reached, got {log:?}"
        );
    }
}

/// RULE 3: a second deployment is refused while the environment holds one, and the refusal
/// names the ENVIRONMENT's own state — the same one whether its deployment is rolling out
/// or baking.
#[tokio::test]
async fn a_second_deployment_is_refused_with_the_environments_own_state() {
    let router = router();
    let (application, profile) = given_a_configuration(&router, "phase-conflict").await;

    // Rows differ along the phase the second deployment arrives in: the rollout, and the
    // final bake that follows it. Both are deployments AppConfig is still working on, and
    // `EnvironmentState` has ONE state for that.
    let rows: [(Row, &str); 2] = [
        (
            ("while the first is rolling out", 1440, 1.0, "LINEAR", 0),
            "DEPLOYING",
        ),
        (
            ("while the first is baking", 0, 100.0, "LINEAR", 1440),
            "BAKING",
        ),
    ];

    for (index, (row, phase)) in rows.iter().enumerate() {
        let (case, ..) = *row;
        let strategy = create_strategy(&router, index, row).await;
        let environment =
            create_environment(&router, &application, &format!("conflict-env-{index}")).await;
        let started = start(&router, &application, &environment, &profile, &strategy).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let number = number_of(case, &started.json());
        let held =
            deployment_in_state(&router, case, &application, &environment, number, &[phase]).await;
        assert_eq!(
            member(&held, "State").as_str(),
            Some(*phase),
            "case {case:?}: this row refuses the second deployment in the {phase} phase, got \
             {held}"
        );

        let refused = start(&router, &application, &environment, &profile, &strategy).await;
        assert_eq!(
            refused.status,
            409,
            "case {case:?}: a second deployment into the same environment is refused, got {} \
             / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            refused.error_code()
        );
        // The state in the sentence is the ENVIRONMENT's, so it is the same word in both
        // phases — never the `DeploymentState` of the deployment holding it, which this
        // row's own phase above shows is not always `DEPLOYING`.
        let expected = format!(
            "Environment {environment} currently has a state of DEPLOYING. The current \
             deployment must finish before a new deployment can be started."
        );
        assert_eq!(
            refused.string("Message").unwrap_or_default(),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            refused.string("Message")
        );

        // The environment reports that same state through its own reader, in the CamelCase
        // form `Environment$State` carries on the wire.
        let environment_now = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{environment}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            environment_now.string("State").as_deref(),
            Some("Deploying"),
            "case {case:?}: an environment with a deployment in flight is deploying, got {}",
            environment_now.json()
        );
    }
}

/// One stop row: the strategy that paces the deployment, the phase the stop arrives in, and
/// what the log must then hold.
struct StopRow {
    row: Row,
    /// The `DeploymentState` the row waits for before stopping.
    phase: &'static str,
    /// The WHOLE log, newest first, for a row whose phase pins it: once a deployment is
    /// baking, no further interval can add to its log while the bake runs.
    exact_log: Option<fn() -> Vec<Entry>>,
    /// Event types no phase this row reached ever logs.
    absent: &'static [&'static str],
}

/// The log a deployment stopped while BAKING holds, newest first.
fn log_of_a_stopped_bake() -> Vec<Entry> {
    vec![
        entry(
            "ROLLBACK_COMPLETED",
            BY_USER,
            "Deployment rolled back by user request",
        ),
        entry(
            "BAKE_TIME_STARTED",
            BY_APPCONFIG,
            "Deployment bake time started",
        ),
        entry("PERCENTAGE_UPDATED", BY_APPCONFIG, &available_to(100.0)),
        start_entry(),
    ]
}

/// RULE 4: a caller's stop settles the rollback in the call, and the event log it answers —
/// which `GetDeployment` answers again — keeps every phase the deployment passed through,
/// newest first, with `ActionInvocations` on every event.
#[tokio::test]
async fn a_stop_answers_the_whole_history_of_the_deployment_it_rolled_back() {
    let router = router();
    let (application, profile) = given_a_configuration(&router, "phase-stop").await;

    // Rows differ along the phase the stop arrives in, which is what decides how much
    // history the log holds: a rollout that is still growing, and a deployment baking after
    // its rollout reached every target.
    let rows: [StopRow; 2] = [
        StopRow {
            row: ("stopped while rolling out", 1440, 1.0, "LINEAR", 0),
            phase: "DEPLOYING",
            // A rollout that is still growing can log a further share at any interval, so
            // this row asserts the entries its phases pin rather than a whole log a faster
            // or slower host would change.
            exact_log: None,
            absent: &["BAKE_TIME_STARTED", "DEPLOYMENT_COMPLETED"],
        },
        StopRow {
            row: ("stopped while baking", 0, 100.0, "LINEAR", 1440),
            phase: "BAKING",
            exact_log: Some(log_of_a_stopped_bake),
            absent: &["DEPLOYMENT_COMPLETED", "REVERT_COMPLETED"],
        },
    ];

    for (index, stop_row) in rows.iter().enumerate() {
        let (case, ..) = stop_row.row;
        let strategy = create_strategy(&router, index, &stop_row.row).await;
        let environment =
            create_environment(&router, &application, &format!("stop-env-{index}")).await;
        let started = start(&router, &application, &environment, &profile, &strategy).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let number = number_of(case, &started.json());
        deployment_in_state(
            &router,
            case,
            &application,
            &environment,
            number,
            &[stop_row.phase],
        )
        .await;

        let stopped = stop(&router, &application, &environment, number).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer its modelled 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let settled = stopped.json();
        assert_eq!(
            member(&settled, "State").as_str(),
            Some("ROLLED_BACK"),
            "case {case:?}: the stop settles the rollback it performs, got {settled}"
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

        // The reader answers the same history: the stop left nothing for a later caller to
        // finish.
        let read_back = read(&router, &application, &environment, number).await;
        let from_the_stop = logged(case, "StopDeployment", &settled);
        let from_the_reader = logged(case, "GetDeployment", &read_back);
        assert_eq!(
            from_the_reader, from_the_stop,
            "case {case:?}: GetDeployment answers the log the stop wrote, got {read_back}"
        );
        assert_eq!(
            member(&read_back, "State").as_str(),
            Some("ROLLED_BACK"),
            "case {case:?}: nothing is left for a reader to finish, got {read_back}"
        );
        assert_eq!(
            member(&read_back, "PercentageComplete").as_f64(),
            Some(0.0),
            "case {case:?}: a rolled-back deployment stays available to no target, got \
             {read_back}"
        );

        for (document, log) in [
            ("StopDeployment", &from_the_stop),
            ("GetDeployment", &from_the_reader),
        ] {
            if let Some(expected) = stop_row.exact_log {
                assert_eq!(
                    *log,
                    expected(),
                    "case {case:?}: {document} answers every phase the deployment passed \
                     through, newest first, got {log:?}"
                );
            }
            assert_eq!(
                log.first().cloned(),
                Some(entry(
                    "ROLLBACK_COMPLETED",
                    BY_USER,
                    "Deployment rolled back by user request"
                )),
                "case {case:?}: {document}'s newest event is the rollback the REQUEST asked \
                 for, got {log:?}"
            );
            assert_eq!(
                log.last().cloned(),
                Some(start_entry()),
                "case {case:?}: {document}'s oldest event is the deployment's start, got \
                 {log:?}"
            );
            for unreached in stop_row.absent {
                assert!(
                    !log.iter().any(|(event_type, _, _)| event_type == unreached),
                    "case {case:?}: {document} must log no {unreached:?} for a phase this \
                     deployment never reached, got {log:?}"
                );
            }
        }
    }
}
