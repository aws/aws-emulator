// SPDX-License-Identifier: CC0-1.0

//! A caller's stop of a release that is still rolling out WITHDRAWS it — from the data
//! plane as well as from the record — rather than freezing it where it had got to.
//!
//! RULE, one level above the request that exposed it: while `0 < PercentageComplete < 100`
//! a deployment is "available" only to the share of targets its rollout has reached
//! (`smithy_query(query="shape", shape="Deployment")` for `PercentageComplete`: "the
//! percentage of targets for which the deployment is available"), so the environment is
//! serving two versions at once; stopping such a deployment is a withdrawal that is
//! already SETTLED when the call answers — the deployment is in its terminal rolled-back
//! state at a `PercentageComplete` of 0, so it is available to NO target, its rollback is
//! on its event log, the environment reports the rolled-back state and every fresh
//! configuration session is served the exact bytes of the version that was in force
//! before the rollout started, whatever share it had reached and whatever paced it — and a
//! second stop of that now-terminal deployment is refused `BadRequestException` (HTTP 400)
//! naming the deployment, the application and the environment from the request and the
//! state read off the record.
//!
//! The companion suite `rollout_percentage_splits_sessions` asserts the gate itself (which
//! version one session at one position is served); this one asserts what a STOP does to a
//! fleet that is split across the two versions, which is the one thing a caller cannot see
//! unless the withdrawal reaches the data plane.
//!
//! The rows span what the rule generalises over — the share the rollout had reached when
//! the stop arrived (just started, the recorded fifth, half, almost everybody) and the
//! growth type and factor pacing it — because the withdrawal is the same whatever the
//! rollout had done. How far a rollout has got is owed by the CLOCK and no operation sets
//! it, so each row writes the share it reads at rather than sleeping through a schedule,
//! exactly as the companion suite does.

use serde_json::Value;

use bytes::Bytes;

use super::configuration_session_poll::{Emulator, Hierarchy, Reply};
use super::rollout_percentage_splits_sessions::{
    create_strategy, fix_position, host_version, pin_percentage, start_deployment, NEW, PREVIOUS,
};

/// The `DeploymentState` wire token a withdrawn release reports, as the recorded stop
/// answers it — `DeploymentState` has no `STOPPED` member: a stop of a deployment still in
/// progress is a rollback.
const ROLLED_BACK: &str = "ROLLED_BACK";
/// The `EnvironmentState` wire token the environment of a withdrawn release reports, as the
/// recording carries it.
const ROLLED_BACK_ENVIRONMENT: &str = "RolledBack";
/// The `DeploymentEventType` the finished rollback is logged under.
const ROLLBACK_COMPLETED: &str = "ROLLBACK_COMPLETED";

/// One row: how the rollout under test is paced, and how far it had got when the stop
/// arrived.
struct Row {
    case: &'static str,
    /// A `GrowthType` the model declares (`EXPONENTIAL | LINEAR`).
    growth_type: &'static str,
    /// The strategy's `GrowthFactor` (`Percentage`, range 1..100).
    growth_factor: f32,
    /// The share of targets the rollout had reached: strictly between nobody and
    /// everybody, which is what makes the fleet split.
    reached: f32,
}

/// The rows: a partial rollout at four different shares, paced four different ways.
const ROWS: &[Row] = &[
    Row {
        case: "a LINEAR rollout stopped one growth step in",
        growth_type: "LINEAR",
        growth_factor: 20.0,
        reached: 20.0,
    },
    Row {
        case: "a LINEAR rollout stopped half way",
        growth_type: "LINEAR",
        growth_factor: 50.0,
        reached: 50.0,
    },
    Row {
        case: "a LINEAR rollout stopped at its smallest share",
        growth_type: "LINEAR",
        growth_factor: 1.0,
        reached: 1.0,
    },
    Row {
        case: "an EXPONENTIAL rollout stopped with almost every target reached",
        growth_type: "EXPONENTIAL",
        growth_factor: 10.0,
        reached: 99.0,
    },
];

/// How many independently opened sessions each post-stop sample holds. The sample is taken
/// twice, as a caller checking that a withdrawal stuck would.
const SAMPLE: usize = 8;

/// RULE: a stop of a rollout that is part-way through withdraws the release from every
/// session, settles the record in its terminal rolled-back state at no share of the
/// targets, and leaves a second stop of that deployment refused.
#[tokio::test]
async fn a_stop_mid_rollout_withdraws_the_release_from_every_session() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("withdrawn-app", "production", "flags")
            .await;

        // The previous version goes out with a strategy that has nothing to wait for and
        // is left to settle, so it is the version in force when the rollout starts.
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                PREVIOUS,
                "application/json",
                None,
            )
            .await;
        emulator.settle_deployments(&hierarchy);

        // The rollout under test: a strategy with a rollout window and a bake time, so the
        // deployment stays in flight at whatever share the row reads it at.
        let version = host_version(&emulator, &hierarchy, NEW).await;
        let strategy = create_strategy(
            &emulator,
            &format!("gradual-{}-{}", row.growth_type, row.growth_factor),
            row.growth_type,
            row.growth_factor,
        )
        .await;
        let number = start_deployment(&emulator, &hierarchy, &strategy, &version).await;
        pin_percentage(&emulator, &hierarchy, row.reached);

        // The precondition the stop is judged against: the environment is serving BOTH
        // versions. The two positions are fixed rather than sampled, one on each side of
        // the share reached, so the split is a fact of the row and not of where the mint
        // happened to put a session.
        let reached_target = row.reached / 2.0;
        let unreached_target = row.reached + (100.0 - row.reached) / 2.0;
        assert_served(
            &emulator,
            &hierarchy,
            &format!("{case}: a session the rollout had reached"),
            Some(reached_target),
            NEW,
        )
        .await;
        assert_served(
            &emulator,
            &hierarchy,
            &format!("{case}: a session the rollout had not reached"),
            Some(unreached_target),
            PREVIOUS,
        )
        .await;

        // THE STOP: it answers the withdrawal already settled.
        let stopped = stop(&emulator, &hierarchy, number).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer 202, got {} body {}",
            stopped.status,
            stopped.text()
        );
        let document = stopped.json();
        assert_state(case, "the stop's own answer", &document, ROLLED_BACK);
        assert_withdrawn(case, "the stop's own answer", &document);

        // And the record reports the same thing, so nothing was left for a reader to
        // finish: the deployment is terminal, available to no target, with its rollback on
        // its event log.
        let read = get_deployment(&emulator, &hierarchy, number).await;
        let read_document = read.json();
        assert_state(case, "GetDeployment", &read_document, ROLLED_BACK);
        assert_withdrawn(case, "GetDeployment", &read_document);

        // The environment follows the deployment a caller withdrew.
        let environment = get_environment(&emulator, &hierarchy).await;
        let environment_state = environment.json()["State"].as_str().map(str::to_string);
        assert_eq!(
            environment_state.as_deref(),
            Some(ROLLED_BACK_ENVIRONMENT),
            "case {case:?}: GetEnvironment must report {ROLLED_BACK_ENVIRONMENT:?} after the \
             stop, got {environment_state:?}"
        );

        // The withdrawal reached the data plane: every fresh session is served the previous
        // version's exact bytes, in two successive samples, and the two fixed positions
        // that were on opposite sides of the share reached are now both on it.
        for round in 1..=2 {
            for index in 0..SAMPLE {
                assert_served(
                    &emulator,
                    &hierarchy,
                    &format!("{case}: sample {round}, session {index}"),
                    None,
                    PREVIOUS,
                )
                .await;
            }
        }
        for position in [0.0, reached_target, unreached_target, 99.9] {
            assert_served(
                &emulator,
                &hierarchy,
                &format!("{case}: a session at {position}% after the stop"),
                Some(position),
                PREVIOUS,
            )
            .await;
        }

        // A second stop of the now-terminal deployment is refused, and the refusal states
        // the deployment the request named, the application and environment it scoped the
        // lookup to, and the state the record is actually in.
        let refused = stop(&emulator, &hierarchy, number).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: a second StopDeployment must be refused 400, got {} body {}",
            refused.status,
            refused.text()
        );
        let code = refused.error_code();
        assert!(
            code.contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {code:?}"
        );
        let expected = format!(
            "Deployment {number} for App {} and Environment {} cannot be stopped, it has a \
             status of {ROLLED_BACK}.",
            hierarchy.application_id, hierarchy.environment_id
        );
        let message = refused.json()["Message"].as_str().map(str::to_string);
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        // And the refused stop left the deployment exactly as the first one had.
        let after = get_deployment(&emulator, &hierarchy, number).await;
        assert_state(
            case,
            "GetDeployment after a refused second stop",
            &after.json(),
            ROLLED_BACK,
        );
    }
}

/// Assert the `State` of a `Deployment` document.
fn assert_state(case: &str, source: &str, document: &Value, expected: &str) {
    let state = document["State"].as_str().map(str::to_string);
    assert_eq!(
        state.as_deref(),
        Some(expected),
        "case {case:?}: {source} must report State {expected:?}, got {state:?} in {document}"
    );
}

/// Assert a `Deployment` document reports a release available to NO target: a
/// `PercentageComplete` of 0 and the rollback's completion on its event log.
fn assert_withdrawn(case: &str, source: &str, document: &Value) {
    let percentage = document["PercentageComplete"].as_f64();
    assert_eq!(
        percentage,
        Some(0.0),
        "case {case:?}: {source} must report PercentageComplete 0 for a withdrawn release, \
         got {percentage:?}"
    );
    let logged = event_types(document);
    assert!(
        logged.iter().any(|event| event == ROLLBACK_COMPLETED),
        "case {case:?}: {source} must log {ROLLBACK_COMPLETED:?}, got {logged:?}"
    );
}

/// The `EventType` of every entry of a `Deployment` document's `EventLog`.
fn event_types(document: &Value) -> Vec<String> {
    document["EventLog"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["EventType"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Open one fresh configuration session over the hierarchy's triple — at `position` in the
/// rollout when the row fixes one, and at the position the service minted for it when it
/// does not — poll it once, and assert the bytes it was served.
async fn assert_served(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    case: &str,
    position: Option<f32>,
    expected: &[u8],
) {
    let token = emulator
        .session_token(
            &hierarchy.application_id,
            &hierarchy.environment_id,
            &hierarchy.profile_id,
        )
        .await;
    if let Some(position) = position {
        fix_position(emulator, &token, position);
    }
    let reply = emulator.get_latest_configuration(&token).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: GetLatestConfiguration must answer 200, got {} body {}",
        reply.status,
        reply.text()
    );
    assert_eq!(
        reply.body.as_ref(),
        expected,
        "case {case:?}: expected {:?}, got {:?}",
        String::from_utf8_lossy(expected),
        reply.text()
    );
}

/// `StopDeployment` as a caller withdraws a release: no `AllowRevert`, since what is being
/// stopped is a deployment the service is still working on.
async fn stop(emulator: &Emulator, hierarchy: &Hierarchy, number: i64) -> Reply {
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    emulator
        .send(
            "DELETE",
            &format!(
                "/applications/{application_id}/environments/{environment_id}\
                 /deployments/{number}"
            ),
            "application/json",
            &[],
            Bytes::new(),
        )
        .await
}

/// `GetDeployment` for one deployment of the hierarchy's environment.
async fn get_deployment(emulator: &Emulator, hierarchy: &Hierarchy, number: i64) -> Reply {
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    let read = emulator
        .send(
            "GET",
            &format!(
                "/applications/{application_id}/environments/{environment_id}\
                 /deployments/{number}"
            ),
            "application/json",
            &[],
            Bytes::new(),
        )
        .await;
    assert_eq!(
        read.status,
        200,
        "GetDeployment({number}): expected 200, got {} body {}",
        read.status,
        read.text()
    );
    read
}

/// `GetEnvironment` for the hierarchy's environment.
async fn get_environment(emulator: &Emulator, hierarchy: &Hierarchy) -> Reply {
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    let read = emulator
        .send(
            "GET",
            &format!("/applications/{application_id}/environments/{environment_id}"),
            "application/json",
            &[],
            Bytes::new(),
        )
        .await;
    assert_eq!(
        read.status,
        200,
        "GetEnvironment: expected 200, got {} body {}",
        read.status,
        read.text()
    );
    read
}
