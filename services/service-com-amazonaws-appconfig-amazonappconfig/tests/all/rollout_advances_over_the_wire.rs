// SPDX-License-Identifier: CC0-1.0

//! A gradual rollout is OBSERVABLE over the wire, on the grid its growth type defines, and
//! it settles on its own — so a caller can watch it, read the share it has reached, and
//! withdraw it part-way through.
//!
//! RULE: a deployment started with a strategy that declares a rollout window advances
//! through the cumulative percentages its growth type prescribes — `LINEAR` "deploys the
//! configuration to 10 percent of the hosts. After those deployments are complete, the
//! system deploys the configuration to the next 10 percent … until 100% of the targets
//! have successfully received the configuration", and `EXPONENTIAL` "processes the
//! deployment exponentially using the following formula: `G*(2^N)` … until the
//! configuration has been deployed to all targets"
//! (`smithy_query(query="doc",shape="CreateDeploymentStrategyRequest",member="GrowthType")`),
//! each percentage capped at the 100 that is the upper bound of the `Percentage` shape the
//! deployment's `GrowthFactor` and `PercentageComplete` both target. Every reader of the
//! deployment sees that progression: `PercentageComplete` — "the percentage of targets for
//! which the deployment is available" — takes each grid value in turn and never goes
//! backwards, one `PERCENTAGE_UPDATED` entry is logged per grid step, and the deployment
//! reaches the terminal `COMPLETE` with `PercentageComplete` 100 and a `CompletedAt`
//! without anybody stopping it. A rollout of more than one step therefore has an instant
//! at which it is PART-WAY through — `0 < PercentageComplete < 100`, with the configuration
//! it deploys in force for exactly that share of the targets and the previous one still in
//! force for the rest — and a `StopDeployment` issued at such an instant withdraws it:
//! HTTP 202 with the deployment `ROLLED_BACK`, available to no target, the rollback's
//! completion on its event log, the environment reporting `RolledBack`, and a second stop
//! refused with the modelled `BadRequestException`
//! (`smithy_query(query="operation",operation="StartDeployment,GetDeployment,StopDeployment")`).
//!
//! The rows span what the rule generalises over: both `GrowthType` values, growth factors
//! that take a different number of intervals to reach every target (including both
//! published bounds of `Percentage`, so a one-interval rollout that is never partial and
//! the longest grid the model can express are both covered), the declared rollout window
//! (one minute, a few, and the published maximum of `MinutesBetween0And24Hours`), and what
//! the stop request says about a revert. Each row's grid is written out from the model's
//! own formula above rather than read back from the emulator. Every assertion names its
//! case and prints what it read.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};
use crate::deployment_wire_fidelity::{
    create_strategy, event_log, event_types, host_version, member, Row,
};
use crate::stop_deployment::{get_deployment, start, stop};

/// How long a rollout is given to settle, or to show itself part-way through, before the
/// suite fails.
///
/// A ceiling on the WAIT rather than a statement about the span a growth interval takes: a
/// rollout that has not moved by here is one nothing is advancing, which is the defect
/// these suites pin.
const SETTLE_BY: Duration = Duration::from_secs(120);

/// How often a rollout under observation is re-read.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a WITHDRAWN rollout is watched for to show it stays where the stop left it:
/// past the growth interval that would have advanced it had the stop not withdrawn it.
const WITHDRAWN_STAYS_PUT_FOR: Duration =
    state::STAGE_DURATION.saturating_add(Duration::from_millis(500));

/// The `DeploymentState` values a deployment stops at, after which no reader sees it move.
const TERMINAL_STATES: &[&str] = &["COMPLETE", "ROLLED_BACK", "REVERTED"];

/// How far two percentages may differ and still be the same point of the grid: the members
/// are floats on the wire, so they are compared with slack rather than for bit equality.
const PERCENTAGE_SLACK: f64 = 0.01;

/// The percentage a rollout that has reached every target reports.
const FULL_PERCENTAGE: f64 = 100.0;

/// Whether `value` is one of the percentages `grid` prescribes.
fn is_on(grid: &[f64], value: f64) -> bool {
    grid.iter()
        .any(|point| (point - value).abs() < PERCENTAGE_SLACK)
}

/// The `PercentageComplete` a Deployment document reports.
fn percentage(case: &str, body: &Value) -> f64 {
    member(body, "PercentageComplete").as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: a Deployment document reports a PercentageComplete, got {body}")
    })
}

/// The `State` a Deployment document reports.
fn deployment_state(case: &str, body: &Value) -> String {
    member(body, "State")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a Deployment document reports a State, got {body}"))
        .to_string()
}

/// The `DeploymentNumber` a Deployment document reports.
fn deployment_number(case: &str, body: &Value) -> i32 {
    let number = member(body, "DeploymentNumber").as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: a Deployment document reports a DeploymentNumber, got {body}")
    });
    i32::try_from(number)
        .unwrap_or_else(|_| panic!("case {case:?}: DeploymentNumber {number} is not an Integer"))
}

/// The `State` `GetEnvironment` reports for one environment, in `EnvironmentState`'s own
/// CamelCase wire form.
async fn environment_state(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Option<String> {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await
    .string("State")
}

/// One rollout to watch: the strategy pacing it and the cumulative percentage of targets
/// each of its growth intervals reaches.
struct Grid {
    case: &'static str,
    duration_in_minutes: i32,
    growth_type: &'static str,
    growth_factor: f64,
    /// The percentages the growth type's own formula prescribes for this growth factor,
    /// in order, each capped at 100.
    grid: &'static [f64],
}

/// The row of [`GRIDS`] that rolls out exponentially in tenths, and the row that rolls the
/// same growth factor out linearly: the pair the step-count comparison below reads, because
/// the two formulas differ in how many intervals one growth factor takes.
const EXPONENTIAL_TENTHS: usize = 0;
/// See [`EXPONENTIAL_TENTHS`].
const LINEAR_TENTHS: usize = 1;

const GRIDS: &[Grid] = &[
    Grid {
        case: "exponential, a tenth per interval",
        duration_in_minutes: 4,
        growth_type: "EXPONENTIAL",
        growth_factor: 10.0,
        grid: &[10.0, 20.0, 40.0, 80.0, 100.0],
    },
    Grid {
        case: "linear, a tenth per interval",
        duration_in_minutes: 4,
        growth_type: "LINEAR",
        growth_factor: 10.0,
        grid: &[
            10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0,
        ],
    },
    Grid {
        case: "exponential, a quarter per interval",
        duration_in_minutes: 1,
        growth_type: "EXPONENTIAL",
        growth_factor: 25.0,
        grid: &[25.0, 50.0, 100.0],
    },
    Grid {
        case: "exponential, a third per interval",
        duration_in_minutes: 60,
        growth_type: "EXPONENTIAL",
        growth_factor: 33.0,
        grid: &[33.0, 66.0, 100.0],
    },
    Grid {
        case: "exponential at the smallest growth factor the model allows",
        duration_in_minutes: 1440,
        growth_type: "EXPONENTIAL",
        growth_factor: 1.0,
        grid: &[1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 100.0],
    },
    Grid {
        case: "linear at the largest growth factor the model allows",
        duration_in_minutes: 1440,
        growth_type: "LINEAR",
        growth_factor: 100.0,
        grid: &[100.0],
    },
];

/// RULE over HTTP: a rollout reports every percentage its growth type's grid prescribes,
/// in order and never going backwards, logs one `PERCENTAGE_UPDATED` per step, and settles
/// on `COMPLETE` at 100% without anybody stopping it — and a rollout of more than one
/// interval is observable part-way through, while one that reaches every target in a single
/// interval never is.
#[tokio::test]
async fn a_rollout_advances_over_the_wire_through_its_growth_types_grid() {
    let router = router();
    let application = create_application(&router, "rollout-grid").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // Every strategy and environment is created BEFORE any deployment starts, so the rows
    // are all started within milliseconds of each other and one row's rollout is not under
    // way while the next is still being set up.
    let mut paced: Vec<(String, String)> = Vec::with_capacity(GRIDS.len());
    for (index, row) in GRIDS.iter().enumerate() {
        let criteria: Row = (
            row.case,
            row.duration_in_minutes,
            row.growth_factor,
            row.growth_type,
            0,
        );
        let strategy = create_strategy(&router, index, &criteria).await;
        let environment =
            create_environment(&router, &application, &format!("grid-env-{index}")).await;
        paced.push((strategy, environment));
    }

    let mut numbers: Vec<i32> = Vec::with_capacity(GRIDS.len());
    let mut observed: Vec<Vec<f64>> = Vec::with_capacity(GRIDS.len());
    for (row, (strategy, environment)) in GRIDS.iter().zip(&paced) {
        let case = row.case;
        let started = start(&router, &application, environment, strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let body = started.json();
        assert_eq!(
            deployment_state(case, &body),
            "DEPLOYING",
            "case {case:?}: a strategy that declares a rollout window leaves the deployment \
             rolling out, got {body}"
        );
        let at_start = percentage(case, &body);
        assert_eq!(
            at_start, 0.0,
            "case {case:?}: a rollout nothing has elapsed for has reached no target, got \
             {at_start}"
        );
        numbers.push(deployment_number(case, &body));
        observed.push(vec![at_start]);
    }

    // One loop over every row, so the rows are watched concurrently and the suite waits as
    // long as its longest rollout rather than as long as all of them put together.
    let mut settled: Vec<Option<Value>> = GRIDS.iter().map(|_| None).collect();
    let deadline = Instant::now() + SETTLE_BY;
    loop {
        let mut pending: Vec<&str> = Vec::new();
        for (index, (_, environment)) in paced.iter().enumerate() {
            if settled[index].is_some() {
                continue;
            }
            let case = GRIDS[index].case;
            let answer = get_deployment(&router, &application, environment, numbers[index]).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: GetDeployment must answer 200, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let body = answer.json();
            let reached = percentage(case, &body);
            if observed[index]
                .last()
                .is_none_or(|last| (last - reached).abs() >= PERCENTAGE_SLACK)
            {
                observed[index].push(reached);
            }
            if TERMINAL_STATES.contains(&deployment_state(case, &body).as_str()) {
                settled[index] = Some(body);
            } else {
                pending.push(case);
            }
        }
        if pending.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "cases {pending:?}: these rollouts reached no terminal state within {}s; what each \
             reported so far: {observed:?}",
            SETTLE_BY.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let mut steps_logged: Vec<usize> = Vec::with_capacity(GRIDS.len());
    for (index, row) in GRIDS.iter().enumerate() {
        let case = row.case;
        let body = settled[index]
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the row settled, so it has a document"));
        let seen = &observed[index];

        assert_eq!(
            deployment_state(case, &body),
            "COMPLETE",
            "case {case:?}: a rollout nobody stopped completes, got {body}"
        );
        assert_eq!(
            percentage(case, &body),
            FULL_PERCENTAGE,
            "case {case:?}: a completed rollout has reached every target, got {body}"
        );
        assert!(
            !member(&body, "CompletedAt").is_null(),
            "case {case:?}: a completed deployment reports when it completed, got {body}"
        );
        // The deployment is paced by the strategy it copied, and reports that pacing back.
        assert_eq!(
            member(&body, "GrowthType").as_str(),
            Some(row.growth_type),
            "case {case:?}: the deployment reports the growth type it was started with, got {body}"
        );
        assert_eq!(
            member(&body, "GrowthFactor").as_f64(),
            Some(row.growth_factor),
            "case {case:?}: the deployment reports the growth factor it was started with, got \
             {body}"
        );

        for value in seen {
            assert!(
                *value == 0.0 || is_on(row.grid, *value),
                "case {case:?}: every percentage this rollout reports lies on the grid {:?} its \
                 growth type prescribes, got {value} (all of them: {seen:?})",
                row.grid
            );
        }
        for pair in seen.windows(2) {
            assert!(
                pair[1] >= pair[0] - PERCENTAGE_SLACK,
                "case {case:?}: a rollout never withdraws from targets it reached, so its \
                 percentages are non-decreasing, got {seen:?}"
            );
        }
        assert_eq!(
            seen.last().copied(),
            Some(FULL_PERCENTAGE),
            "case {case:?}: the last percentage observed is the whole fleet, got {seen:?}"
        );

        let partial: Vec<f64> = seen
            .iter()
            .copied()
            .filter(|value| *value > 0.0 && *value < FULL_PERCENTAGE)
            .collect();
        if row.grid.len() > 1 {
            assert!(
                !partial.is_empty(),
                "case {case:?}: a rollout of {} intervals must be observable part-way through, \
                 got {seen:?}",
                row.grid.len()
            );
            for value in &partial {
                assert!(
                    is_on(row.grid, *value),
                    "case {case:?}: a partial rollout sits on a step of {:?}, got {value}",
                    row.grid
                );
            }
        } else {
            assert!(
                partial.is_empty(),
                "case {case:?}: a rollout that reaches every target in one interval is never \
                 partial, got {partial:?}"
            );
        }

        let log = event_types(&event_log(case, "GetDeployment", &body));
        let logged = log
            .iter()
            .filter(|event| event.as_str() == "PERCENTAGE_UPDATED")
            .count();
        assert_eq!(
            logged,
            row.grid.len(),
            "case {case:?}: one PERCENTAGE_UPDATED is logged per step of {:?}, got the log \
             {log:?}",
            row.grid
        );
        steps_logged.push(logged);
    }

    // The two formulas differ in how many steps ONE growth factor takes: doubling the share
    // each interval reaches every target in fewer steps than adding it does, which is what
    // tells a reader of the log which growth type paced the rollout.
    assert!(
        steps_logged[EXPONENTIAL_TENTHS] < steps_logged[LINEAR_TENTHS],
        "case {:?} against {:?}: an exponential rollout must take fewer steps than a linear one \
         at the same growth factor, got {} against {}",
        GRIDS[EXPONENTIAL_TENTHS].case,
        GRIDS[LINEAR_TENTHS].case,
        steps_logged[EXPONENTIAL_TENTHS],
        steps_logged[LINEAR_TENTHS]
    );
}

/// One rollout to stop part-way through: the strategy pacing it, and what the stop request
/// says about a revert — which changes nothing for a deployment that has not completed.
struct Withdrawal {
    case: &'static str,
    duration_in_minutes: i32,
    growth_type: &'static str,
    growth_factor: f64,
    allow_revert: Option<bool>,
}

const WITHDRAWALS: &[Withdrawal] = &[
    Withdrawal {
        case: "linear, a fifth per interval, with no revert header",
        duration_in_minutes: 5,
        growth_type: "LINEAR",
        growth_factor: 20.0,
        allow_revert: None,
    },
    Withdrawal {
        case: "exponential, a tenth per interval, with a revert disallowed",
        duration_in_minutes: 4,
        growth_type: "EXPONENTIAL",
        growth_factor: 10.0,
        allow_revert: Some(false),
    },
    Withdrawal {
        case: "exponential at the smallest growth factor, with a revert allowed",
        duration_in_minutes: 1440,
        growth_type: "EXPONENTIAL",
        growth_factor: 1.0,
        allow_revert: Some(true),
    },
];

/// Poll `GetDeployment` until the rollout is part-way through, and answer the percentage it
/// reported then. Fails the row when it settles, or runs out of time, without ever being
/// observable part-way through — the instant a stop has to be issued at.
async fn observed_part_way_through(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    number: i32,
) -> f64 {
    let deadline = Instant::now() + SETTLE_BY;
    loop {
        let answer = get_deployment(router, application, environment, number).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: GetDeployment must answer 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        let reached = percentage(case, &body);
        let reported = deployment_state(case, &body);
        assert!(
            !TERMINAL_STATES.contains(&reported.as_str()),
            "case {case:?}: the rollout settled on {reported:?} without ever being observable \
             part-way through, so no stop could be issued mid-rollout"
        );
        if reached > 0.0 && reached < FULL_PERCENTAGE {
            return reached;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no partial rollout within {}s; last read {reached}% / {reported:?}",
            SETTLE_BY.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE over HTTP: a rollout under way can be WITHDRAWN. A stop issued while
/// `0 < PercentageComplete < 100` answers 202 with the deployment `ROLLED_BACK` and
/// available to no target, the rollback's completion newest on its event log; the reader and
/// the environment report that outcome, the clock does not resume the rollout, and a second
/// stop is refused with the modelled `BadRequestException` naming the deployment it refused.
#[tokio::test]
async fn a_stop_part_way_through_a_rollout_withdraws_it_terminally() {
    let router = router();
    let application = create_application(&router, "stop-mid-rollout").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    for (index, row) in WITHDRAWALS.iter().enumerate() {
        let case = row.case;
        let criteria: Row = (
            case,
            row.duration_in_minutes,
            row.growth_factor,
            row.growth_type,
            0,
        );
        let strategy = create_strategy(&router, index, &criteria).await;
        // One environment per row, so no stop reaches another row's deployment.
        let environment =
            create_environment(&router, &application, &format!("withdraw-env-{index}")).await;

        let started = start(&router, &application, &environment, &strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let number = deployment_number(case, &started.json());

        let reached =
            observed_part_way_through(&router, case, &application, &environment, number).await;

        let stopped = stop(&router, &application, &environment, number, row.allow_revert).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer the modelled 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let stop_body = stopped.json();
        assert_eq!(
            deployment_state(case, &stop_body),
            "ROLLED_BACK",
            "case {case:?}: a deployment stopped at {reached}% of its targets is rolled back, got \
             {stop_body}"
        );
        assert_eq!(
            percentage(case, &stop_body),
            0.0,
            "case {case:?}: a rollout withdrawn at {reached}% is available to no target, got \
             {stop_body}"
        );
        assert!(
            member(&stop_body, "CompletedAt").is_null(),
            "case {case:?}: a rolled-back deployment never completed, so it reports no completion \
             instant, got {}",
            member(&stop_body, "CompletedAt")
        );
        let log = event_types(&event_log(case, "StopDeployment", &stop_body));
        assert_eq!(
            log.first().map(String::as_str),
            Some("ROLLBACK_COMPLETED"),
            "case {case:?}: the newest event of a stopped deployment is its rollback finishing, \
             got {log:?}"
        );
        assert!(
            log.iter().any(|event| event == "PERCENTAGE_UPDATED"),
            "case {case:?}: the rollout the stop withdrew had reached {reached}% of its targets, \
             so its steps are on the log, got {log:?}"
        );

        // The round trip: the reader answers the record the stop settled.
        let fetched = get_deployment(&router, &application, &environment, number).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeployment must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.json(),
            stop_body,
            "case {case:?}: GetDeployment must report the settled record the stop answered"
        );
        let environment_now = environment_state(&router, &application, &environment).await;
        assert_eq!(
            environment_now.as_deref(),
            Some("RolledBack"),
            "case {case:?}: an environment whose rollout was withdrawn reports it, got \
             {environment_now:?}"
        );

        // A second stop has nothing left to stop, and says which state it refused, in the
        // words AppConfig words that refusal in.
        let again = stop(&router, &application, &environment, number, row.allow_revert).await;
        assert_eq!(
            again.status,
            400,
            "case {case:?}: stopping a terminal deployment must be refused with the modelled \
             BadRequestException's HTTP 400, got {} / {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert!(
            again.error_code().contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            again.error_code()
        );
        assert_eq!(
            again.string("Message").unwrap_or_default(),
            format!(
                "Deployment {number} for App {application} and Environment {environment} cannot \
                 be {}, it has a status of ROLLED_BACK.",
                // The verb the refusal states is the one the request asked for: a stop
                // carrying `AllowRevert` asked for a revert.
                if row.allow_revert.unwrap_or(false) {
                    "reverted"
                } else {
                    "stopped"
                }
            ),
            "case {case:?}: the refusal must name the deployment, its application, its \
             environment, the verb it was asked for and the state it refused"
        );

        // The clock does not resume a rollout a caller withdrew: the record is re-read
        // across a further growth interval and stays exactly where the stop left it at
        // EVERY look. That is an invariant rather than a condition to wait for, so it is
        // asserted on each re-read instead of once after a fixed wait.
        let hold_until = Instant::now() + WITHDRAWN_STAYS_PUT_FOR;
        loop {
            let later = get_deployment(&router, &application, &environment, number).await;
            let later_body = later.json();
            assert_eq!(
                deployment_state(case, &later_body),
                "ROLLED_BACK",
                "case {case:?}: a withdrawn rollout stays withdrawn as the clock runs on, got \
                 {later_body}"
            );
            assert_eq!(
                percentage(case, &later_body),
                0.0,
                "case {case:?}: a withdrawn rollout reaches no further target, got {later_body}"
            );
            if Instant::now() >= hold_until {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The bytes of the two hosted versions the data-plane row deploys, which tell apart which
/// version a client was served.
const VERSION_ONE: &[u8] = br#"{"tier":"one"}"#;
/// See [`VERSION_ONE`].
const VERSION_TWO: &[u8] = br#"{"tier":"two"}"#;

/// How many clients one sweep of the fleet reads with.
///
/// Each client's place in the rollout is independent of every other's, so a sweep is a
/// SAMPLE: this many makes the chance of a rollout part-way through drawing a fleet that is
/// entirely on one version vanishing.
const CLIENTS: usize = 60;

/// What one sweep of the fleet was served.
struct Sweep {
    /// How many clients were served the new version's bytes.
    on_new: usize,
    /// How many were served neither deployed version's bytes.
    unexpected: usize,
}

/// Write one hosted configuration version of `profile`.
async fn host(router: &MultiServiceRouter, application: &str, profile: &str, content: &[u8]) {
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `StartDeployment` of one configuration version, answering the deployment it minted.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    version: &str,
) -> Value {
    let answer: Answer = json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: StartDeployment must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// Read the configuration once for each of [`CLIENTS`] clients and report what they were
/// served.
async fn sweep(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
) -> Sweep {
    let mut swept = Sweep {
        on_new: 0,
        unexpected: 0,
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
            answer.status,
            200,
            "client client-{client:04}: GetConfiguration must answer 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = answer.body.as_ref();
        if served == VERSION_TWO {
            swept.on_new += 1;
        } else if served != VERSION_ONE {
            swept.unexpected += 1;
        }
    }
    swept
}

/// RULE over HTTP, on the data plane: while a rollout is part-way through, the share of
/// targets it has reached is served the version it deploys and every other target is still
/// served the version that preceded it — so both versions are in service AT ONCE, which is
/// the window a caller watching a rollout acts in — and once the rollout completes every
/// target is on the new version.
#[tokio::test]
async fn a_rollout_part_way_through_serves_both_versions_at_once() {
    let case = "a fifth of the fleet per interval";
    let router = router();
    let application = create_application(&router, "canary-window").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_profile(&router, &application, "config").await;
    host(&router, &application, &profile, VERSION_ONE).await;
    host(&router, &application, &profile, VERSION_TWO).await;

    // Version 1 goes out with a strategy that declares no window at all, so it is in force
    // for the whole fleet one growth interval after it was started — which the suite waits
    // for, because reaching the targets is a transition of its own.
    let instant = create_strategy(&router, 0, &("all at once", 0, 100.0, "LINEAR", 0)).await;
    let first = deploy(
        &router,
        case,
        &application,
        &environment,
        &profile,
        &instant,
        "1",
    )
    .await;
    let first = crate::appconfig_workflow::deployment_in_state(
        &router,
        case,
        &application,
        &environment,
        i64::from(deployment_number(case, &first)),
        &["COMPLETE"],
    )
    .await;
    assert_eq!(
        deployment_state(case, &first),
        "COMPLETE",
        "case {case:?}: a strategy with no window and no bake completes a span after it \
         started, got {first}"
    );
    let before = sweep(&router, &application, &environment, &profile).await;
    assert_eq!(
        before.on_new, 0,
        "case {case:?}: before the gradual rollout every client is on version 1, {} of {CLIENTS} \
         were not",
        before.on_new
    );

    // Version 2 then rolls out gradually, and the fleet is swept at each share it reaches.
    let gradual = create_strategy(&router, 1, &(case, 5, 20.0, "LINEAR", 0)).await;
    let number = deployment_number(
        case,
        &deploy(
            &router,
            case,
            &application,
            &environment,
            &profile,
            &gradual,
            "2",
        )
        .await,
    );

    let mut both_in_service: Option<(f64, usize)> = None;
    let mut swept_at: Option<f64> = None;
    let deadline = Instant::now() + SETTLE_BY;
    loop {
        let answer = get_deployment(&router, &application, &environment, number).await;
        let body = answer.json();
        let reached = percentage(case, &body);
        let terminal = TERMINAL_STATES.contains(&deployment_state(case, &body).as_str());
        // Swept once per share the rollout reaches, rather than once per read: the sweep is
        // what the share is asserted through, and re-reading the same share says no more.
        let unswept = swept_at.is_none_or(|last| (last - reached).abs() >= PERCENTAGE_SLACK);
        if !terminal && reached > 0.0 && reached < FULL_PERCENTAGE && unswept {
            let share = sweep(&router, &application, &environment, &profile).await;
            swept_at = Some(reached);
            assert_eq!(
                share.unexpected, 0,
                "case {case:?} at {reached}%: every client must be served one of the two deployed \
                 versions' exact bytes, {} of {CLIENTS} were served something else",
                share.unexpected
            );
            if share.on_new > 0 && share.on_new < CLIENTS {
                both_in_service = Some((reached, share.on_new));
            }
        }
        if terminal {
            assert_eq!(
                deployment_state(case, &body),
                "COMPLETE",
                "case {case:?}: a rollout nobody stopped completes, got {body}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the rollout reached no terminal state within {}s; last read \
             {reached}%",
            SETTLE_BY.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    assert!(
        both_in_service.is_some(),
        "case {case:?}: a rollout of five intervals must have an instant at which both versions \
         are in service at once; the last share swept was {swept_at:?}"
    );
    let after = sweep(&router, &application, &environment, &profile).await;
    assert_eq!(
        after.on_new, CLIENTS,
        "case {case:?}: once the rollout has reached every target the whole fleet is on version \
         2, {} of {CLIENTS} were",
        after.on_new
    );
}
