// SPDX-License-Identifier: CC0-1.0

//! A deployment's rollout percentage GATES what AppConfig serves, so a partial rollout
//! is partial in what consumers receive.
//!
//! RULE: a deployment's `PercentageComplete` is "the percentage of targets for which the
//! deployment is available", and a strategy's `GrowthFactor` is "the percentage of
//! targets that received a deployed configuration during each interval"
//! (`smithy_query(query="shape",shape="DeploymentStrategy")`,
//! `smithy_query(query="doc",shape="Deployment",member="PercentageComplete")`), so while
//! a rollout is under way the configuration it deploys is in force for exactly the share
//! of targets it has already reached: every other target keeps being served the
//! configuration that was in force before — those exact bytes, never a blend of the two —
//! and the fleet converges on the new version only as the rollout grows to 100%. Which
//! targets are in which share is decided per target and independently; for the
//! `GetConfiguration` read the target is the caller's own `ClientId`, which "enables
//! AppConfig to deploy the configuration in intervals, as defined in the deployment
//! strategy" (`smithy_query(query="doc",shape="GetConfigurationRequest",member="ClientId")`),
//! so one client keeps its place across its reads. A rollout nothing has undone serves
//! the share it reached whether it is `DEPLOYING`, `BAKING` or `COMPLETE`; one a stop
//! rolled back or reverted serves no target at all.
//!
//! The gate is asserted at two levels, because it is one decision read by two surfaces:
//!
//! * against the state records, where a whole rollout schedule can be walked without any
//!   test sleeping — the rows span the growth TYPE (`LINEAR` and `EXPONENTIAL`), the
//!   growth FACTOR, and therefore the number of intervals a rollout takes, and each row is
//!   probed at percentage points below, at and above what the rollout has reached;
//! * over real HTTP through `GetConfiguration`, where the rows span the rollout percentage
//!   (none reached, part reached, all reached) and the clients reading at it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, ComAmazonawsAppconfigAmazonappconfigState, Deployment, DeploymentStrategy, Environment,
    NewDeployment, RolloutBucket,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router_on, send, Answer,
};

/// A strategy record, as `CreateDeploymentStrategy` would have stored one.
fn strategy(
    growth_type: &str,
    growth_factor: f32,
    duration_in_minutes: i32,
    bake_in_minutes: i32,
) -> DeploymentStrategy {
    DeploymentStrategy {
        id: "roll001".to_string(),
        name: "rollout".to_string(),
        description: None,
        deployment_duration_in_minutes: duration_in_minutes,
        growth_type: growth_type.to_string(),
        growth_factor,
        final_bake_time_in_minutes: bake_in_minutes,
        replicate_to: state::REPLICATE_TO_NONE.to_string(),
        tags: HashMap::new(),
    }
}

/// What `StartDeployment` persists, with the deployment number and configuration version
/// a row names.
fn new_deployment(deployment_number: i32, configuration_version: &str) -> NewDeployment {
    NewDeployment {
        deployment_number,
        application_id: "abcd123".to_string(),
        environment_id: "efgh456".to_string(),
        configuration_profile_id: "ijkl789".to_string(),
        configuration_name: "config".to_string(),
        configuration_location_uri: "hosted".to_string(),
        configuration_version: configuration_version.to_string(),
        description: None,
        kms_key_identifier: None,
        version_label: None,
        applied_extensions: Vec::new(),
        tags: HashMap::new(),
    }
}

/// One rollout to walk: the strategy pacing it and the cumulative percentage of targets
/// each of its growth intervals reaches.
///
/// The grid is the model's own formula for the growth type rather than this emulator's:
/// `LINEAR` "deploys the configuration to the targets at a constant rate", so interval
/// `n` has added the growth factor `n + 1` times, and `EXPONENTIAL` "grows the deployment
/// following the equation `G*(2^N)`"
/// (`smithy_query(query="shape",shape="GrowthType")`), each capped at 100%.
struct Schedule {
    case: &'static str,
    growth_type: &'static str,
    growth_factor: f32,
    grid: &'static [f32],
}

/// The rows span both growth types and, within each, factors that take a different
/// number of intervals to reach every target — including the one-interval rollout that
/// is never partial.
const SCHEDULES: &[Schedule] = &[
    Schedule {
        case: "linear, 25% per interval",
        growth_type: state::GROWTH_LINEAR,
        growth_factor: 25.0,
        grid: &[25.0, 50.0, 75.0, 100.0],
    },
    Schedule {
        case: "linear, 20% per interval",
        growth_type: state::GROWTH_LINEAR,
        growth_factor: 20.0,
        grid: &[20.0, 40.0, 60.0, 80.0, 100.0],
    },
    Schedule {
        case: "linear, 50% per interval",
        growth_type: state::GROWTH_LINEAR,
        growth_factor: 50.0,
        grid: &[50.0, 100.0],
    },
    Schedule {
        case: "linear, every target in one interval",
        growth_type: state::GROWTH_LINEAR,
        growth_factor: 100.0,
        grid: &[100.0],
    },
    Schedule {
        case: "exponential, 10% canary",
        growth_type: state::GROWTH_EXPONENTIAL,
        growth_factor: 10.0,
        grid: &[10.0, 20.0, 40.0, 80.0, 100.0],
    },
    Schedule {
        case: "exponential, 25% canary",
        growth_type: state::GROWTH_EXPONENTIAL,
        growth_factor: 25.0,
        grid: &[25.0, 50.0, 100.0],
    },
];

/// The percentage points a row's targets are probed at: each grid value a rollout passes
/// through, every midpoint between two of them, and the two ends of the range.
fn probes() -> Vec<f32> {
    let mut probes = vec![0.0f32, 0.5, 99.5];
    for step in 0..=20 {
        probes.push(step as f32 * 5.0);
        probes.push(step as f32 * 5.0 + 2.5);
    }
    probes.retain(|probe| *probe < 100.0);
    probes
}

/// RULE, against the state records: at every growth interval of a rollout, the
/// configuration is in force for exactly the targets below the percentage of targets the
/// rollout has reached — so a target AT that percentage point is not reached yet, and one
/// above it is not reached either, whatever the growth type and factor.
#[test]
fn a_rollout_serves_exactly_the_targets_its_percentage_has_reached() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // A bake time keeps the deployment in flight for the whole walk, so what is asserted
    // is the gate rather than the deployment's completion.
    let probes = probes();

    for schedule in SCHEDULES {
        let case = schedule.case;
        let paced_by = strategy(schedule.growth_type, schedule.growth_factor, 30, 30);

        // At the start instant nothing has rolled out, so no target is served — not even
        // the first one.
        let at_start = Deployment::start(new_deployment(1, "2"), &paced_by, started_at);
        for probe in &probes {
            assert!(
                !at_start.serves_the_configuration_to(RolloutBucket::at_percentile(f64::from(
                    *probe
                ))),
                "case {case:?} at the start instant: a rollout that has reached 0% of its \
                 targets serves none of them, but it serves the target at {probe}% \
                 (PercentageComplete {})",
                at_start.percentage_complete
            );
        }

        for (interval, reached) in schedule.grid.iter().enumerate() {
            let mut deployment = Deployment::start(new_deployment(1, "2"), &paced_by, started_at);
            // Each growth interval of the rollout is paced by one span
            // (`state::STAGE_DURATION`), so the walk steps through the schedule in those
            // spans rather than in the strategy's declared minutes.
            let elapsed = state::STAGE_DURATION
                * u32::try_from(interval + 1).expect("an interval index fits");
            deployment.materialize(started_at + elapsed + Duration::from_millis(1));

            assert!(
                (deployment.percentage_complete - reached).abs() < 0.01,
                "case {case:?} interval {interval}: expected PercentageComplete {reached}, \
                 got {}",
                deployment.percentage_complete
            );
            for probe in &probes {
                let target = RolloutBucket::at_percentile(f64::from(*probe));
                let expected = *probe < *reached;
                assert_eq!(
                    deployment.serves_the_configuration_to(target),
                    expected,
                    "case {case:?} interval {interval} ({reached}% of targets reached): the \
                     target at {probe}% expected served={expected}, got served={}",
                    deployment.serves_the_configuration_to(target)
                );
            }
        }
    }
}

/// RULE: a target a rollout has not reached keeps being served the deployment that was in
/// force before it — and a caller's stop withdraws the new configuration from the share
/// that did receive it, so every target is back on the previous one.
#[test]
fn a_target_the_rollout_has_not_reached_stays_on_the_previous_deployment() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let profile = "ijkl789";
    let mut environment = Environment::new(
        "efgh456".to_string(),
        "abcd123".to_string(),
        "production".to_string(),
        None,
        Vec::new(),
        HashMap::new(),
    );

    // Version 1 is deployed by a strategy with no declared window: its one growth
    // interval reaches every target a span after the deployment started, which is when the
    // environment below is read.
    let all_at_once = strategy(state::GROWTH_LINEAR, 100.0, 0, 0);
    environment.record_deployment(Deployment::start(
        new_deployment(1, "1"),
        &all_at_once,
        started_at,
    ));
    let one_interval = state::STAGE_DURATION;
    let version_one_in_force = started_at + one_interval + Duration::from_millis(1);
    environment.materialize(version_one_in_force);
    for probe in probes() {
        let target = RolloutBucket::at_percentile(f64::from(probe));
        assert_eq!(
            environment
                .in_force_deployment(profile, target)
                .map(|deployment| deployment.configuration_version.as_str()),
            Some("1"),
            "case a completed deployment: the target at {probe}% must be served version 1"
        );
    }

    // Version 2 is then rolled out at 25% of targets per interval, and read one interval
    // in: the targets below 25% are on version 2 and every other target is still on
    // version 1.
    let gradual = strategy(state::GROWTH_LINEAR, 25.0, 30, 30);
    environment.record_deployment(Deployment::start(
        new_deployment(2, "2"),
        &gradual,
        version_one_in_force,
    ));
    let read_at = version_one_in_force + one_interval + Duration::from_millis(1);
    environment.materialize(read_at);
    for probe in probes() {
        let target = RolloutBucket::at_percentile(f64::from(probe));
        let expected = if probe < 25.0 { "2" } else { "1" };
        assert_eq!(
            environment
                .in_force_deployment(profile, target)
                .map(|deployment| deployment.configuration_version.as_str()),
            Some(expected),
            "case a rollout at 25%: the target at {probe}% must be served version \
             {expected}"
        );
    }

    // A stop rolls that deployment back, which returns every target — including the ones
    // the rollout had reached — to the version that preceded it.
    environment
        .deployments
        .get_mut(&2)
        .expect("the rolling-out deployment is held under its number")
        .stop(false, read_at + Duration::from_millis(1))
        .expect("an in-flight deployment is stoppable");
    for probe in probes() {
        let target = RolloutBucket::at_percentile(f64::from(probe));
        assert_eq!(
            environment
                .in_force_deployment(profile, target)
                .map(|deployment| deployment.configuration_version.as_str()),
            Some("1"),
            "case a stopped rollout: the target at {probe}% must be back on version 1"
        );
    }
}

/// RULE: a deployment a stop undid serves NO target, however far its rollout had got —
/// the rollback is returning to the configuration that preceded it — and it reports
/// itself available to none of them, because `PercentageComplete` is "the percentage of
/// targets for which the deployment is available" and the share the rollout had reached
/// is the share it was withdrawn from.
#[test]
fn an_undone_rollout_serves_no_target() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // Each row undoes a deployment at a different point of its rollout: one stopped
    // part-way through, one stopped after every target was reached, one reverted
    // after it completed, and one AppConfig itself aborted while it was rolling out.
    let cases: [(&str, f32, i32, i32, u32, Undone); 4] = [
        (
            "stopped at a quarter of its targets",
            25.0,
            30,
            30,
            1,
            Undone::Stopped,
        ),
        (
            "stopped having reached every target",
            100.0,
            30,
            30,
            1,
            Undone::Stopped,
        ),
        ("reverted after completing", 100.0, 0, 0, 1, Undone::Reverted),
        (
            "rolled back by AppConfig part-way through",
            25.0,
            30,
            30,
            2,
            Undone::RolledBackByAppConfig,
        ),
    ];

    for (case, growth_factor, duration, bake, elapsed_intervals, undone) in cases {
        let paced_by = strategy(state::GROWTH_LINEAR, growth_factor, duration, bake);
        let mut deployment = Deployment::start(new_deployment(1, "2"), &paced_by, started_at);
        let reached_at = started_at
            + state::STAGE_DURATION * elapsed_intervals
            + Duration::from_millis(1);
        deployment.materialize(reached_at);
        let reached = deployment.percentage_complete;
        match undone {
            Undone::Stopped => deployment.stop(false, reached_at),
            Undone::Reverted => deployment.stop(true, reached_at),
            Undone::RolledBackByAppConfig => deployment.begin_rollback(reached_at),
        }
        .unwrap_or_else(|refusal| panic!("case {case:?}: the stop was refused: {refusal:?}"));

        assert_eq!(
            deployment.percentage_complete, 0.0,
            "case {case:?}: a deployment undone at {reached}% of its targets is available \
             to none of them, got PercentageComplete {}",
            deployment.percentage_complete
        );
        for probe in probes() {
            let target = RolloutBucket::at_percentile(f64::from(probe));
            assert!(
                !deployment.serves_the_configuration_to(target),
                "case {case:?}: an undone deployment ({}) must serve no target, but it \
                 serves the one at {probe}% (PercentageComplete {})",
                deployment.state,
                deployment.percentage_complete
            );
        }
    }
}

/// How a row's rollout was undone: by the caller's stop, by the caller's revert of a
/// completed deployment, or by AppConfig itself aborting one in flight.
#[derive(Debug, Clone, Copy)]
enum Undone {
    Stopped,
    Reverted,
    RolledBackByAppConfig,
}

/// How many clients each HTTP row reads the configuration with. Large enough that a
/// partial rollout is split with certainty: the share of clients on either version
/// follows the rollout percentage, so with this many independent clients the chance of
/// a 25%-to-75% rollout reaching all of them or none of them is vanishing.
const CLIENTS: usize = 80;

/// How far the measured share of clients on the new version may sit from the rollout
/// percentage. A client's place in the rollout is independent of every other client's, so
/// the measured share is a sample: this is the sampling slack, not a tolerance on the
/// rule.
const SHARE_SLACK_IN_POINTS: f32 = 30.0;

/// RULE over HTTP: while a rollout is part-way through, the clients reading the
/// configuration are SPLIT — each is served one version's exact bytes, the share on the
/// new version tracks `PercentageComplete`, and a client keeps whichever version it got
/// across its reads; at 0% every client is on the previous version, at 100% every client
/// is on the new one, and a stop puts every client back on the previous version.
#[tokio::test]
async fn clients_are_split_across_a_partial_rollout_and_a_stop_withdraws_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let application = create_application(&router, "canary").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_profile(&router, &application, "flags").await;

    host_version(&router, &application, &profile, b"{\"tier\":\"one\"}").await;
    host_version(&router, &application, &profile, b"{\"tier\":\"two\"}").await;

    // Version 1 goes out with a strategy that declares no window at all, so it is in force
    // for every client one growth interval after it was started — which the suite waits
    // for, because reaching the targets is a transition of its own.
    let instant = create_strategy(&router, "instant", 100.0, 0, 0).await;
    let seeding = deploy(&router, &application, &environment, &profile, &instant, "1").await;
    crate::appconfig_workflow::deployment_in_state(
        &router,
        "before the rollout",
        &application,
        &environment,
        seeding,
        &["COMPLETE"],
    )
    .await;
    let share = measure_share(&router, &application, &environment, &profile).await;
    assert_eq!(
        share.on_new, 0,
        "case before the rollout: every client must be on version 1, {} of {CLIENTS} were \
         not",
        share.on_new
    );

    // Version 2 then rolls out gradually. The strategy's one growth interval falls due at
    // the end of its rollout window, so the deployment stays in flight at whatever
    // percentage each row pins while the clients below read it.
    let gradual = create_strategy(&router, "gradual", 100.0, 1440, 1440).await;
    let number = deploy(&router, &application, &environment, &profile, &gradual, "2").await;

    // The rows span the rollout: none of the fleet reached, three partial shares, and the
    // whole fleet reached.
    for percentage in [0.0f32, 25.0, 50.0, 75.0, 100.0] {
        pin_percentage(&engine, &application, &environment, percentage);
        let share = measure_share(&router, &application, &environment, &profile).await;
        let case = format!("a rollout at {percentage}%");

        assert_eq!(
            share.unexpected, 0,
            "case {case}: every client must be served one of the two deployed versions' \
             exact bytes, {} of {CLIENTS} were served something else",
            share.unexpected
        );
        match percentage {
            0.0 => assert_eq!(
                share.on_new, 0,
                "case {case}: a rollout that has reached no target serves version 2 to \
                 nobody, got {} of {CLIENTS}",
                share.on_new
            ),
            100.0 => assert_eq!(
                share.on_new, CLIENTS,
                "case {case}: a rollout that has reached every target serves version 2 to \
                 all of them, got {} of {CLIENTS}",
                share.on_new
            ),
            _ => {
                assert!(
                    share.on_new > 0 && share.on_new < CLIENTS,
                    "case {case}: a partial rollout must serve BOTH versions, got {} of \
                     {CLIENTS} on version 2",
                    share.on_new
                );
                let measured = share.on_new as f32 / CLIENTS as f32 * 100.0;
                assert!(
                    (measured - percentage).abs() <= SHARE_SLACK_IN_POINTS,
                    "case {case}: the share on version 2 must track the rollout \
                     percentage within {SHARE_SLACK_IN_POINTS} points, got {measured}% \
                     ({} of {CLIENTS})",
                    share.on_new
                );
            }
        }
    }

    // A client keeps its place: read twice at the same percentage, every client is served
    // the same version both times.
    pin_percentage(&engine, &application, &environment, 50.0);
    let first = measure_share(&router, &application, &environment, &profile).await;
    pin_percentage(&engine, &application, &environment, 50.0);
    let second = measure_share(&router, &application, &environment, &profile).await;
    assert_eq!(
        first.per_client, second.per_client,
        "case a client reading twice: each client must keep the version its place in the \
         rollout puts it on"
    );

    // And the stop withdraws version 2 from the share that had it.
    stop_deployment(&router, &application, &environment, number).await;
    let share = measure_share(&router, &application, &environment, &profile).await;
    assert_eq!(
        share.on_new, 0,
        "case a stopped rollout: every client must be back on version 1, {} of {CLIENTS} \
         were not",
        share.on_new
    );
}

/// What one sweep of the fleet measured.
struct Share {
    /// How many clients were served the new version's bytes.
    on_new: usize,
    /// How many were served neither deployed version's bytes.
    unexpected: usize,
    /// Which version each client was served, in client order.
    per_client: Vec<u8>,
}

/// Read the configuration once for each of [`CLIENTS`] clients and report what each was
/// served.
async fn measure_share(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
) -> Share {
    let mut share = Share {
        on_new: 0,
        unexpected: 0,
        per_client: Vec::with_capacity(CLIENTS),
    };
    for client in 0..CLIENTS {
        let answer = send(
            router,
            "GET",
            &format!(
                "/applications/{application}/environments/{environment}/configurations/{profile}\
                 ?client_id=client-{client:04}"
            ),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "client client-{client:04}: GetConfiguration must answer 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = answer.body.as_ref();
        if served == b"{\"tier\":\"two\"}" {
            share.on_new += 1;
            share.per_client.push(2);
        } else if served == b"{\"tier\":\"one\"}" {
            share.per_client.push(1);
        } else {
            share.unexpected += 1;
            share.per_client.push(0);
        }
    }
    share
}

/// Pin the in-flight deployment's `PercentageComplete` at `percentage`, the one thing no
/// operation exposes: how far a rollout has got is owed by the CLOCK, and a test that
/// needs the fleet read at a given share writes that share rather than sleeping through
/// the schedule. The start instant is moved with it, so the clock owes the record nothing
/// while the clients below read it.
fn pin_percentage(
    engine: &Arc<engine::InMemoryEngine>,
    application_id: &str,
    environment_id: &str,
    percentage: f32,
) {
    let state = engine.state::<ComAmazonawsAppconfigAmazonappconfigState>();
    let mut store = state.lock();
    let environment = store
        .application_mut(application_id)
        .and_then(|application| application.environments.get_mut(environment_id))
        .expect("the environment the rollout is deploying into");
    let now = SystemTime::now();
    for deployment in environment.deployments.values_mut() {
        if deployment.is_in_flight() {
            deployment.started_at = now;
            deployment.percentage_complete = percentage;
        }
    }
}

/// Write one hosted configuration version.
async fn host_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content: &[u8],
) {
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}\
             /hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateHostedConfigurationVersion must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// Create a deployment strategy and answer its id.
async fn create_strategy(
    router: &MultiServiceRouter,
    name: &str,
    growth_factor: f32,
    duration_in_minutes: i32,
    bake_in_minutes: i32,
) -> String {
    json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": duration_in_minutes,
            "GrowthFactor": growth_factor,
            "GrowthType": "LINEAR",
            "FinalBakeTimeInMinutes": bake_in_minutes,
            "ReplicateTo": "NONE",
        }),
    )
    .await
    .string("Id")
    .unwrap_or_else(|| panic!("case {name:?}: CreateDeploymentStrategy answered no Id"))
}

/// Deploy one hosted configuration version, answering the deployment number.
async fn deploy(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy_id: &str,
    version: &str,
) -> i64 {
    let answer: Answer = json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy_id,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "StartDeployment(version {version}) must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()["DeploymentNumber"]
        .as_i64()
        .unwrap_or_else(|| panic!("StartDeployment answered no DeploymentNumber"))
}

/// Stop a deployment, as a caller withdraws a rollout under way.
async fn stop_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i64,
) {
    let answer = send(
        router,
        "DELETE",
        &format!(
            "/applications/{application}/environments/{environment}/deployments/{number}"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        202,
        "StopDeployment({number}) must answer 202, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}
