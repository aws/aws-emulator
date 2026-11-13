// SPDX-License-Identifier: CC0-1.0

//! A rollout's percentage GATES what the data plane serves, so a partial rollout splits
//! the configuration sessions polling it.
//!
//! RULE: a deployment's `PercentageComplete` is "the percentage of targets for which the
//! deployment is available", so while a rollout is under way the configuration it
//! deploys is in force for exactly the share of targets it has already reached. A
//! configuration session is one such target: independently opened sessions over the same
//! (application, environment, configuration profile) triple are therefore SPLIT while a
//! rollout is part-way through — some polls answer the new version's exact bytes and the
//! rest the previous version's exact bytes, never a blend — a session keeps whichever
//! version its own position in the rollout puts it on for as long as the rollout stands,
//! and the fleet converges on the new version only once the rollout has reached every
//! target. A caller's stop withdraws the new version from the share that had received it,
//! so every session is served the previous version again.
//!
//! The gate is asserted three ways, because a session's position in a rollout is minted
//! at random by design:
//!
//! * the positions themselves: successive sessions are minted at positions spread over
//!   the whole range, which is what makes a partial rollout split the fleet at all;
//! * the gate exactly: a table of (position, percentage reached) rows, with each
//!   session's position FIXED in the store before it polls, so each row's expected
//!   version is a fact rather than a sample — the rows sit below, at and above the
//!   percentage, and span a rollout that has reached nobody, part of the fleet and
//!   everybody;
//! * end to end: sessions opened with nothing written by hand are split mid-rollout, and
//!   a stop puts every session back on the previous version.
//!
//! Every request goes through the router harness the polling suite sets up
//! ([`super::configuration_session_poll::Emulator`]), which mounts AppConfigData and the
//! AppConfig control plane over one shared engine: the hierarchy, the hosted versions and
//! the deployments are all created through AppConfig's own operations, exactly as an
//! operator would. What the tests write by hand is what no operation exposes: how far the
//! rollout has got (owed by the CLOCK, so a test needing the fleet read at a given share
//! writes that share rather than sleeping through the schedule) and, for the exact table,
//! the position a session was minted at.

use std::time::{Duration, SystemTime};

use bytes::Bytes;

use service_com_amazonaws_appconfig_amazonappconfig::state::RolloutBucket;
use service_com_amazonaws_appconfigdata_appconfigdata::state::ComAmazonawsAppconfigdataAppconfigdataState;

use super::configuration_session_poll::{Emulator, Hierarchy};

/// The two versions deployed, told apart by their bytes.
pub(crate) const PREVIOUS: &[u8] = br#"{"tier":"one"}"#;
/// See [`PREVIOUS`].
pub(crate) const NEW: &[u8] = br#"{"tier":"two"}"#;

/// RULE: the position a session is minted at is its own — successive sessions are spread
/// over the whole range of percentage points a rollout advances through, so a rollout
/// part-way through has reached some sessions and not others.
///
/// Driven against the store rather than over HTTP: what is asserted is the spread over
/// many sessions, and the instant is fixed, so the positions this walks are the same ones
/// on every run.
#[test]
fn the_positions_sessions_are_minted_at_spread_over_the_whole_range() {
    let mut store = ComAmazonawsAppconfigdataAppconfigdataState::default();
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    const SESSIONS: usize = 200;
    const BANDS: usize = 10;

    let mut per_band = [0usize; BANDS];
    for _ in 0..SESSIONS {
        let position = store.mint_rollout_bucket(now);
        // Minted exactly as `StartConfigurationSession` mints one, so what is counted is
        // what a fleet of sessions would be assigned.
        store.start_session("MyApp", "Prod", "MyProfile", None, None, position, now);
        let percentile = position.percentile();
        assert!(
            (0.0..100.0).contains(&percentile),
            "case a minted position: a position must be a percentage point in 0..100, got \
             {percentile}"
        );
        per_band[(percentile / (100.0 / BANDS as f32)) as usize % BANDS] += 1;
    }

    for (band, count) in per_band.iter().enumerate() {
        let from = band * (100 / BANDS);
        assert!(
            *count > 0,
            "case the {from}% band: {SESSIONS} sessions must reach every tenth of the \
             range, none landed in this one (bands {per_band:?})"
        );
        assert!(
            *count < SESSIONS / 2,
            "case the {from}% band: no tenth of the range may hold half the sessions, this \
             one holds {count} of {SESSIONS} (bands {per_band:?})"
        );
    }
}

/// One row of the exact table: where the session stands, how far the rollout has got, and
/// the version the session must therefore be served.
struct Row {
    /// The percentage point the polling session stands at.
    position: f32,
    /// The percentage of targets the rollout has reached.
    reached: f32,
    /// The bytes the poll must answer.
    expected: &'static [u8],
}

/// The rows span a rollout that has reached nobody, part of the fleet and everybody, and
/// within each one the positions below, exactly at and above what it reached — a target
/// AT the percentage reached is not reached yet, which is what makes 0% reach nobody.
const ROWS: &[Row] = &[
    Row { position: 0.0, reached: 0.0, expected: PREVIOUS },
    Row { position: 0.0, reached: 25.0, expected: NEW },
    Row { position: 24.9, reached: 25.0, expected: NEW },
    Row { position: 25.0, reached: 25.0, expected: PREVIOUS },
    Row { position: 60.0, reached: 25.0, expected: PREVIOUS },
    Row { position: 49.9, reached: 50.0, expected: NEW },
    Row { position: 50.0, reached: 50.0, expected: PREVIOUS },
    Row { position: 99.9, reached: 100.0, expected: NEW },
    Row { position: 0.0, reached: 100.0, expected: NEW },
];

/// RULE, exactly: a session is served the new version when the rollout has passed its
/// position and the previous version's exact bytes when it has not — and a stop withdraws
/// the new version from a session that had already been served it.
#[tokio::test]
async fn a_session_is_served_the_version_its_position_in_the_rollout_puts_it_on() {
    let emulator = Emulator::new();
    let hierarchy = emulator
        .create_hierarchy("canary-app", "production", "flags")
        .await;
    let number = deploy_two_versions(&emulator, &hierarchy).await;

    for row in ROWS {
        let case = format!(
            "a session at {}% of a rollout that reached {}%",
            row.position, row.reached
        );
        let served = poll_at(&emulator, &hierarchy, row.position, row.reached).await;
        assert_eq!(
            served.as_slice(),
            row.expected,
            "case {case}: expected {:?}, got {:?}",
            String::from_utf8_lossy(row.expected),
            String::from_utf8_lossy(&served)
        );
    }

    // A session the rollout HAD reached is put back on the previous version when the
    // caller stops the deployment: the same session, polled again through the token the
    // first poll handed it.
    pin_percentage(&emulator, &hierarchy, 50.0);
    let token = emulator
        .session_token(
            &hierarchy.application_id,
            &hierarchy.environment_id,
            &hierarchy.profile_id,
        )
        .await;
    fix_position(&emulator, &token, 10.0);
    let reply = emulator.get_latest_configuration(&token).await;
    assert_eq!(
        reply.body.as_ref(),
        NEW,
        "case a session the rollout reached: expected the new version, got {:?}",
        reply.text()
    );
    let next = reply.next_token();
    emulator.stop_deployment(&hierarchy, number).await;
    let reply = emulator.get_latest_configuration(&next).await;
    assert_eq!(
        reply.status,
        200,
        "case a stopped rollout: the session must still poll, got {} body {}",
        reply.status,
        reply.text()
    );
    assert_eq!(
        reply.body.as_ref(),
        PREVIOUS,
        "case a stopped rollout: a session that had been served the new version must be \
         served the previous version's bytes again, got {:?}",
        reply.text()
    );
}

/// How many independently opened sessions the end-to-end sweep samples. Each session's
/// position is independent of every other's, so with this many the chance that a
/// half-finished rollout has reached all of them, or none of them, is below one in ten
/// thousand.
const SAMPLE: usize = 16;

/// RULE end to end: with nothing written by hand but how far the rollout has got,
/// independently opened sessions over one triple are served BOTH versions while the
/// rollout is half-way through, and each is served one version's exact bytes.
#[tokio::test]
async fn independently_opened_sessions_are_split_mid_rollout() {
    let emulator = Emulator::new();
    let hierarchy = emulator
        .create_hierarchy("canary-app", "production", "flags")
        .await;
    deploy_two_versions(&emulator, &hierarchy).await;

    let mut on_new = 0;
    let mut on_previous = 0;
    for index in 0..SAMPLE {
        // Re-pinned per session, so the share each session reads at is the same one
        // however long the sweep takes.
        pin_percentage(&emulator, &hierarchy, 50.0);
        let token = emulator
            .session_token(
                &hierarchy.application_id,
                &hierarchy.environment_id,
                &hierarchy.profile_id,
            )
            .await;
        let reply = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            reply.status,
            200,
            "session {index}: GetLatestConfiguration must answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        match reply.body.as_ref() {
            NEW => on_new += 1,
            PREVIOUS => on_previous += 1,
            other => panic!(
                "session {index}: a session must be served one of the two deployed \
                 versions' exact bytes, got {:?}",
                String::from_utf8_lossy(other)
            ),
        }
    }
    assert!(
        on_new > 0 && on_previous > 0,
        "case a half-finished rollout: both versions must be in service at the same time, \
         got {on_new} of {SAMPLE} on the new version and {on_previous} on the previous one"
    );
}

/// Deploy the previous version with a strategy that has nothing to wait for and let it
/// settle, then start the gradual rollout of the new version — answering that rollout's
/// deployment number.
///
/// The gradual strategy's one growth interval falls due at the end of the rollout window
/// it declares and it bakes afterwards, so the deployment stays in flight at whatever
/// share a test pins while its sessions read it.
async fn deploy_two_versions(emulator: &Emulator, hierarchy: &Hierarchy) -> i64 {
    emulator
        .deploy(
            hierarchy,
            &hierarchy.profile_id,
            PREVIOUS,
            "application/json",
            None,
        )
        .await;
    emulator.settle_deployments(hierarchy);
    let version = host_version(emulator, hierarchy, NEW).await;
    let strategy = create_strategy(emulator, "gradual", "LINEAR", 100.0).await;
    start_deployment(emulator, hierarchy, &strategy, &version).await
}

/// Open one session, fix the position it stands at, and poll it once with the rollout
/// pinned at `reached` — answering the bytes it was served.
async fn poll_at(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    position: f32,
    reached: f32,
) -> Vec<u8> {
    pin_percentage(emulator, hierarchy, reached);
    let token = emulator
        .session_token(
            &hierarchy.application_id,
            &hierarchy.environment_id,
            &hierarchy.profile_id,
        )
        .await;
    fix_position(emulator, &token, position);
    let reply = emulator.get_latest_configuration(&token).await;
    assert_eq!(
        reply.status,
        200,
        "a session at {position}% of a rollout at {reached}%: GetLatestConfiguration must \
         answer 200, got {} body {}",
        reply.status,
        reply.text()
    );
    reply.body.to_vec()
}

/// Fix the position the session behind `token` stands at in a rollout.
///
/// A session is minted at a position of its own, spread over the range so that a fleet of
/// sessions is split — which is asserted on its own above. A row that states the version
/// ONE session must be served pins that position instead, so the row is about the gate
/// rather than about where the mint happened to put it.
pub(crate) fn fix_position(emulator: &Emulator, token: &str, position: f32) {
    let state = emulator.state();
    let mut store = state.lock();
    store
        .sessions
        .get_mut(token)
        .expect("the session was persisted under its token")
        .rollout_bucket = RolloutBucket::at_percentile(f64::from(position));
}

/// Pin the in-flight deployment's `PercentageComplete` at `percentage`, with its start
/// instant moved to now so the clock owes the record nothing while the poll below reads
/// it. How far a rollout has got is owed by the clock and no operation sets it.
pub(crate) fn pin_percentage(emulator: &Emulator, hierarchy: &Hierarchy, percentage: f32) {
    let state = emulator.control_plane();
    let mut store = state.lock();
    let environment = store
        .application_mut(&hierarchy.application_id)
        .and_then(|application| application.environments.get_mut(&hierarchy.environment_id))
        .expect("the environment the rollout is deploying into");
    let now = SystemTime::now();
    for deployment in environment.deployments.values_mut() {
        if deployment.is_in_flight() {
            deployment.started_at = now;
            deployment.percentage_complete = percentage;
        }
    }
}

/// Write one hosted configuration version, answering the version number AppConfig minted.
pub(crate) async fn host_version(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    content: &[u8],
) -> String {
    let application_id = &hierarchy.application_id;
    let profile_id = &hierarchy.profile_id;
    let written = emulator
        .send(
            "POST",
            &format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}\
                 /hostedconfigurationversions"
            ),
            "application/json",
            &[],
            Bytes::copy_from_slice(content),
        )
        .await;
    assert_eq!(
        written.status,
        201,
        "CreateHostedConfigurationVersion: expected 201, got {} body {}",
        written.status,
        written.text()
    );
    written
        .header("version-number")
        .unwrap_or_else(|| panic!("no Version-Number header; headers {:?}", written.headers))
        .to_string()
}

/// Create a deployment strategy that paces a gradual rollout: a rollout window and a bake
/// time long enough that the clock owes the deployment nothing while a test reads it, grown
/// by `growth_factor` percent of the targets per interval in the `growth_type` the caller
/// names (the modelled `GrowthType`, `EXPONENTIAL | LINEAR`).
pub(crate) async fn create_strategy(
    emulator: &Emulator,
    name: &str,
    growth_type: &str,
    growth_factor: f32,
) -> String {
    let created = emulator
        .post_json(
            "/deploymentstrategies",
            &format!(
                r#"{{"Name":"{name}","DeploymentDurationInMinutes":1440,
                      "GrowthFactor":{growth_factor},"GrowthType":"{growth_type}",
                      "FinalBakeTimeInMinutes":1440,"ReplicateTo":"NONE"}}"#
            ),
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateDeploymentStrategy({name}): expected 201, got {} body {}",
        created.status,
        created.text()
    );
    created.string("Id")
}

/// Start the gradual deployment of `version`, answering its deployment number.
pub(crate) async fn start_deployment(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    strategy_id: &str,
    version: &str,
) -> i64 {
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    let profile_id = &hierarchy.profile_id;
    let started = emulator
        .post_json(
            &format!("/applications/{application_id}/environments/{environment_id}/deployments"),
            &format!(
                r#"{{"DeploymentStrategyId":"{strategy_id}",
                      "ConfigurationProfileId":"{profile_id}",
                      "ConfigurationVersion":"{version}"}}"#
            ),
        )
        .await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment(version {version}): expected 201, got {} body {}",
        started.status,
        started.text()
    );
    started.json()["DeploymentNumber"]
        .as_i64()
        .unwrap_or_else(|| panic!("no DeploymentNumber in {}", started.text()))
}
