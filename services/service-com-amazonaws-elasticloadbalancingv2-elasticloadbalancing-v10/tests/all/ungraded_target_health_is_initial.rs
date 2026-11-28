// SPDX-License-Identifier: CC0-1.0

//! What `DescribeTargetHealth` reports for a registration whose health the load balancer's
//! checks have NOT determined.
//!
//! The rule asserted here: **`TargetHealth$State` is a FINDING of the group's health checks. A
//! registration is reported `healthy` only once a check has found it passing, so one no check
//! has found passing — because none has been made yet, because the checks made of it observed
//! NOTHING, or because its checks fail — is reported `initial`, worded as the model words the
//! two reasons for that state: `Elb.RegistrationInProgress` while the registration is being
//! made and `Elb.InitialHealthChecking` while the load balancer is "still sending the target the
//! minimum number of health checks required to determine its health status". A state the checks
//! reached is never given back: no reading after a decided one reports `initial` again. And a
//! group whose health checks are OFF reports its registrations `unavailable`, the model's own
//! `Target.HealthCheckDisabled`, rather than a health nothing is checking.**
//!
//! `TargetHealth$Reason` (`smithy_query(query="doc",shape="TargetHealth",member="Reason")`) is
//! where those words come from, and it also gives the healthy side its shape: "if the target
//! state is `healthy`, a reason code is not provided".
//!
//! The rows differ along what the group's checks can ESTABLISH about the registration, which is
//! what the rule generalises over:
//!
//! * a `lambda` registration whose check is an INVOCATION this emulator cannot carry at all —
//!   no Lambda is mounted beside this service on the suite's engine, so the in-process transport
//!   refuses the call and nothing about the function comes back. Its checks therefore find
//!   nothing, over and over, and the registration must stay `initial`: this is the shape of the
//!   divergence this suite was written for, where an ungraded target was served an optimistic
//!   `healthy` window;
//! * a registration this emulator holds nothing CONTRADICTING — an address in a network the
//!   account's EC2 inventory does not hold — whose checks are answered, so it is reported
//!   `healthy`, and only once a check has found it so: the readings taken while its registration
//!   was still being made are `initial`;
//! * a `lambda` registration of a group whose health checks are off, which nothing checks at
//!   all.
//!
//! The failing side of the same rule — a `lambda` target whose invocation IS carried and whose
//! answer the group's codes reject, which goes from `initial` straight to `unhealthy` without
//! ever being reported `healthy` — is pinned by the sibling suite
//! `all/lambda_target_health_state_sequence.rs`, whose rows run a real function for every answer
//! a check can get.
//!
//! Nothing here is timed by the wall clock. Every row's whole history of readings is recorded,
//! the one wait is a look repeated until the row whose checks pass has reached its verdict, and
//! each look first places the instant the next check of every registration is owed at
//! ([`make_checks_due`]) so a verdict waits for the checks rather than for a cadence to run out
//! on this host.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::alb_lambda_target_health_check::make_checks_due;
use crate::listener_rules::{create_listener, create_load_balancer};
use crate::provisioning_loop::{
    account, action, call, emulator, first_text, health_block, members, p, REGION,
};

/// How long the one wait gives the checks to reach the verdict a row expects, and how long
/// between two looks. Generous on purpose — everything waited for happens in this process, so a
/// loaded host cannot decide the outcome, and the budget is only ever spent by a failure.
const BUDGET: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The health-check cadence every checked group below is created with, and the span one of its
/// checks may wait for an answer: a cadence inside the range the member admits ("the range is
/// 5-300") and longer than the wait, which is what a target group may carry ("health check
/// interval must be greater than the timeout"), and a wait many times what an in-process call
/// costs, so how fast this host runs does not decide whether an answer arrives in time.
const INTERVAL_SECONDS: &str = "10";
const TIMEOUT_SECONDS: &str = "5";

/// How many consecutive checks each verdict takes: the FEWEST either threshold member admits
/// ("the range is 2-10"), because the rule under test is which state a registration is reported
/// in and not how many checks a verdict takes.
const THRESHOLD: &str = "2";

/// The `TargetHealthStateEnum` values this suite reads back.
const INITIAL: &str = "initial";
const HEALTHY: &str = "healthy";
const UNAVAILABLE: &str = "unavailable";

/// The `TargetHealthReasonEnum` values an `initial` registration is reported under, in the order
/// the registration sequence reaches them: the registration being made, then the load balancer
/// still sending the checks its health is determined from.
const REGISTRATION_IN_PROGRESS: &str = "Elb.RegistrationInProgress";
const INITIAL_HEALTH_CHECKING: &str = "Elb.InitialHealthChecking";

/// The reason a registration of a group whose health checks are off is reported under:
/// "`Target.HealthCheckDisabled` - Health checks are disabled for the target group".
const HEALTH_CHECK_DISABLED: &str = "Target.HealthCheckDisabled";

/// One reading of a registration's health: the three members `TargetHealth` carries, each read
/// as empty where the answer carries none.
type Reading = (String, String, String);

/// What a row's checks can establish about its registration, which is what its target group and
/// its registration are built to produce.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checks {
    /// An invocation this emulator cannot carry: a `lambda` group whose registered function no
    /// mounted service of this engine serves a call for.
    FindNothing,
    /// Answered checks: a registration this emulator holds nothing contradicting.
    Pass,
    /// None at all: a group whose `HealthCheckEnabled` is off.
    AreOff,
}

/// One row: the target group it builds, what its checks can establish, and the reading its
/// registration must settle on.
struct Row {
    case: &'static str,
    group_name: &'static str,
    listener_port: i32,
    checks: Checks,
}

impl Row {
    /// The `(state, reason, description)` this row's registration must settle on.
    fn settles_on(&self) -> Reading {
        match self.checks {
            // Still being checked: no check has found it passing, and the load balancer is
            // still sending the checks its health is determined from.
            Checks::FindNothing => (
                INITIAL.to_string(),
                INITIAL_HEALTH_CHECKING.to_string(),
                "Health checks in progress".to_string(),
            ),
            // "If the target state is healthy, a reason code is not provided", and no
            // description comes with it either.
            Checks::Pass => (HEALTHY.to_string(), String::new(), String::new()),
            Checks::AreOff => (
                UNAVAILABLE.to_string(),
                HEALTH_CHECK_DISABLED.to_string(),
                "Health checks are not enabled for this target group".to_string(),
            ),
        }
    }
}

/// The rows, in the order they are created and routed.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a lambda target whose health-check invocation nothing carries",
            group_name: "ungraded-no-answer",
            listener_port: 80,
            checks: Checks::FindNothing,
        },
        Row {
            case: "an address this emulator holds nothing contradicting",
            group_name: "ungraded-reachable",
            listener_port: 81,
            checks: Checks::Pass,
        },
        Row {
            case: "a lambda target group whose health checks are off",
            group_name: "ungraded-checks-off",
            listener_port: 82,
            checks: Checks::AreOff,
        },
    ]
}

/// A function ARN of this account and Region. No Lambda is mounted on this suite's engine, so a
/// health check of a registration naming it is a call the in-process transport refuses — which
/// is a check that establishes nothing about the target.
fn function_arn() -> String {
    format!("arn:aws:lambda:{REGION}:{}:function:ungraded-health", account())
}

/// A network no VPC of the account's EC2 inventory is: nothing this emulator holds says
/// anything about an address in it, which is what the reachable row's checks are answered on.
const UNHELD_VPC: &str = "vpc-0123456789abcdef0";

/// The address that row registers, inside that network.
const UNHELD_ADDRESS: &str = "10.60.1.7";

/// Create the row's target group, register its target and route a listener of `lb_arn` to it —
/// each through this service's own API, the last of which is what starts the health checking.
/// Answers the group's ARN.
async fn routed_group(router: &MultiServiceRouter, row: &Row, lb_arn: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.group_name));
    match row.checks {
        // A `lambda` group names no network and no port; its checks are on, which is what a
        // caller must ask for ("if the target type is lambda, health checks are disabled by
        // default but can be enabled").
        Checks::FindNothing => {
            form.push(p("TargetType", "lambda"));
            form.push(p("HealthCheckEnabled", "true"));
            form.push(p("HealthCheckPath", "/hc"));
            form.push(p("Matcher.HttpCode", "200"));
            form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
            form.push(p("HealthCheckTimeoutSeconds", TIMEOUT_SECONDS));
            form.push(p("HealthyThresholdCount", THRESHOLD));
            form.push(p("UnhealthyThresholdCount", THRESHOLD));
        }
        Checks::Pass => {
            form.push(p("TargetType", "ip"));
            form.push(p("Protocol", "HTTP"));
            form.push(p("Port", "80"));
            form.push(p("VpcId", UNHELD_VPC));
            form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
            form.push(p("HealthCheckTimeoutSeconds", TIMEOUT_SECONDS));
            form.push(p("HealthyThresholdCount", THRESHOLD));
            form.push(p("UnhealthyThresholdCount", THRESHOLD));
        }
        // Checks OFF, which is the default for a `lambda` group: nothing checks its
        // registrations at all.
        Checks::AreOff => {
            form.push(p("TargetType", "lambda"));
            form.push(p("HealthCheckEnabled", "false"));
        }
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: CreateTargetGroup must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );
    let tg_arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body));

    let target = match row.checks {
        Checks::Pass => UNHELD_ADDRESS.to_string(),
        Checks::FindNothing | Checks::AreOff => function_arn(),
    };
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", &target));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: RegisterTargets must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );

    create_listener(router, lb_arn, &tg_arn, row.listener_port).await;
    tg_arn
}

/// What `DescribeTargetHealth` reports for the one registration of the target group `arn`.
async fn described(router: &MultiServiceRouter, case: &str, arn: &str) -> Reading {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DescribeTargetHealth must answer 200, got {} body {}",
        reply.status, reply.body
    );
    let descriptions = members(&reply.body, "TargetHealthDescriptions");
    assert_eq!(
        descriptions.len(),
        1,
        "case {case:?}: expected one description for the one registered target, got {}",
        reply.body
    );
    // The health's own element, because the override beside it carries members of the same
    // names ([`health_block`]).
    let health = health_block(&descriptions[0]);
    (
        first_text(&health, "State").unwrap_or_default(),
        first_text(&health, "Reason").unwrap_or_default(),
        first_text(&health, "Description").unwrap_or_default(),
    )
}

/// Look at every row's target group, over and over, until the row whose checks PASS reports the
/// health they found it — answering the whole HISTORY each row was reported in, in order, with
/// repeats collapsed.
///
/// The wait is for that CONDITION rather than for a span, and one loop serves every row because
/// their checks all run on the one clock: polling one row to its verdict would let another row's
/// history pass unobserved. The row whose checks pass is the condition because it is the only
/// one whose reported state changes at all — the others must hold the state they are in for as
/// long as this suite looks, and that row's verdict takes at least as long as a check.
async fn histories(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    rows: &[Row],
    arns: &[String],
) -> Vec<Vec<Reading>> {
    let deadline = Instant::now() + BUDGET;
    let mut seen: Vec<Vec<Reading>> = rows.iter().map(|_| Vec::new()).collect();
    loop {
        make_checks_due(engine);
        for ((row, arn), history) in rows.iter().zip(arns).zip(seen.iter_mut()) {
            let reading = described(router, row.case, arn).await;
            if history.last() != Some(&reading) {
                history.push(reading);
            }
        }
        let graded = rows
            .iter()
            .zip(&seen)
            .filter(|(row, _)| row.checks == Checks::Pass)
            .all(|(row, history)| history.last() == Some(&row.settles_on()));
        if graded || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: a registration is reported `healthy` only once a check has FOUND it passing — one whose
/// checks establish nothing stays `initial`, worded as one of the two reasons the model gives
/// that state, and a group whose checks are off reports its registrations `unavailable` — and no
/// reading after a decided one gives the state back to `initial`.
#[tokio::test]
async fn a_target_no_check_has_graded_is_reported_initial_and_never_healthy() {
    let (router, engine) = emulator();
    let lb_arn = create_load_balancer(&router, "ungraded-health-alb").await;

    // Every row's group is created, registered and routed before anything is looked at, so the
    // checks of all of them run on their own cadence over the same span.
    let rows = rows();
    let mut arns: Vec<String> = Vec::new();
    for row in &rows {
        arns.push(routed_group(&router, row, &lb_arn).await);
    }

    let seen = histories(&router, &engine, &rows, &arns).await;

    for (row, history) in rows.iter().zip(&seen) {
        let states: Vec<&str> = history.iter().map(|(state, _, _)| state.as_str()).collect();
        let settles_on = row.settles_on();

        // The health its checks decide, which is where the sequence ends.
        assert_eq!(
            history.last(),
            Some(&settles_on),
            "case {:?}: expected the registration to settle on {settles_on:?}, got the history \
             {history:?}",
            row.case
        );

        // `healthy` is never reported for a registration no check found passing, at any point
        // in its life.
        if row.checks != Checks::Pass {
            assert!(
                !states.contains(&HEALTHY),
                "case {:?}: a registration no check has found passing is never reported \
                 {HEALTHY}, got the states {states:?}",
                row.case
            );
        }

        // Nothing but `initial` is ever reported beside the row's own settled state, every
        // `initial` reading is worded as one of the two reasons the model gives the state, and
        // no reading after a decided one gives the state back.
        let mut decided = false;
        for (state, reason, description) in history {
            assert!(
                state == INITIAL || *state == settles_on.0,
                "case {:?}: expected only {INITIAL} and {} readings, got \
                 {state}/{reason}/{description:?} in {states:?}",
                row.case,
                settles_on.0
            );
            if state == INITIAL {
                assert!(
                    !decided,
                    "case {:?}: a decided registration is not reported {INITIAL} again, got \
                     {states:?}",
                    row.case
                );
                assert!(
                    reason == REGISTRATION_IN_PROGRESS || reason == INITIAL_HEALTH_CHECKING,
                    "case {:?}: expected an {INITIAL} reading worded \
                     {REGISTRATION_IN_PROGRESS} or {INITIAL_HEALTH_CHECKING}, got {reason:?} in \
                     {states:?}",
                    row.case
                );
            } else {
                decided = true;
            }
        }

        // The row whose checks pass went through `initial` on its way, wherever this suite's
        // sampling caught it: a registration being made is reported `initial`, and the health
        // its checks found it comes after. A history of ONE reading is a registration whose
        // whole registration span had passed before the first look, which says nothing either
        // way — the row above, whose checks never find anything, is what pins that no state
        // before a finding is `healthy`.
        if row.checks == Checks::Pass && history.len() > 1 {
            assert_eq!(
                states.first(),
                Some(&INITIAL),
                "case {:?}: a registration being made is reported {INITIAL} before the checks \
                 have found it anything, got {states:?}",
                row.case
            );
        }
    }
}
