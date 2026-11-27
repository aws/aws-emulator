// SPDX-License-Identifier: CC0-1.0

//! The SEQUENCE of `TargetHealth$State` values `DescribeTargetHealth` serves for a `lambda`
//! registration, from the moment it is registered until its health checks have decided it.
//!
//! The rule under test: **`healthy` is a FINDING of the health checks, so a registration is
//! reported it only once a check has found it passing. Until the load balancer has determined
//! its health the state is `initial` — `Elb.RegistrationInProgress` while the registration is
//! being made and `Elb.InitialHealthChecking` while the checks have not decided — and a
//! registration whose checks fail is therefore never reported `healthy` at any point in its
//! life: it goes from `initial` to `unhealthy` with the reason its failing checks found. A
//! state the checks reached is not given back either: no reading after a `healthy` or an
//! `unhealthy` one reports `initial` again.**
//!
//! `TargetHealth$Reason` words the two `initial` reasons as "`Elb.RegistrationInProgress` - The
//! target is in the process of being registered with the load balancer" and
//! "`Elb.InitialHealthChecking` - The load balancer is still sending the target the minimum
//! number of health checks required to determine its health status", and
//! `HealthyThresholdCount` is "the number of consecutive health check successes required before
//! considering an unhealthy target healthy" — so a registration no check has found passing has
//! not earned `healthy`. The unhealthy side carries the condition those checks found:
//! "`Target.ResponseCodeMismatch` - The health checks did not return an expected HTTP code" for
//! an answer the group's `Matcher` rejects, and "`Target.FailedHealthChecks` - The load balancer
//! received an error while establishing a connection to the target or the target response was
//! malformed" for a run that returned no response at all.
//!
//! The rows differ along what the rule generalises over: what the invoked function ANSWERS the
//! check with — a status the group's codes accept, two statuses they reject, and a run that
//! raises, which carries no status at all — and the codes the group counts as success (one code
//! and a range). Every row's whole history is recorded, so a row is satisfied by the SEQUENCE of
//! states rather than by the one it settles on.
//!
//! Nothing here is observed from a record this suite invented: the function is a real deployment
//! package created through Lambda's OWN API and RUN by the Lambda emulator mounted on this
//! suite's engine, and every state is read back through `DescribeTargetHealth` over the wire.
//!
//! Nothing is timed by the wall clock either: the one wait is a look repeated until every row
//! has reached the verdict its checks decide, inside a budget generous enough that a loaded host
//! cannot decide the outcome, and no row asserts anything about how long a state took. The span
//! one check may wait for its answer is left as long as the cadence allows for the same reason —
//! how fast this host runs the function must not decide whether its answer arrives in time.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::create_load_balancer;
use crate::alb_lambda_target_published_address::lambda;
// The function a row's checks invoke, the package carrying it and the mounting it runs under are
// the sibling suite's: one function and one emulator for every suite whose rows turn on what a
// health check's invocation answers, so none of them drifts on what is mounted or answered.
use crate::lambda_health_check_verdict_reasons::{create_function, emulator, FUNCTION};
use crate::provisioning_loop::{action, call, first_text, health_block, members, p};

/// The cadence every checked group is created with, and the span one of its checks may wait for
/// an answer.
///
/// The cadence is what the states in between are observed over: the first check of a
/// registration is owed when its checking begins and every later one comes one
/// `HealthCheckIntervalSeconds` after the last, so a verdict of [`THRESHOLD`] consecutive checks
/// is one cadence away and every reading taken before it is a reading of a registration whose
/// health the load balancer has not determined. The WAIT is as long as the cadence admits, which
/// is many times what running this handler in this process costs: how fast the host runs it must
/// not decide whether the answer arrives in time.
const INTERVAL_SECONDS: &str = "10";
const TIMEOUT_SECONDS: &str = "10";

/// How many consecutive checks each verdict takes: the FEWEST either threshold member admits
/// ("the range is 2-10"), because the rule under test is the sequence of states a registration is
/// reported in and not how many checks a verdict takes.
const THRESHOLD: &str = "2";

/// How long the one wait gives every row to reach the verdict its checks decide, and how long
/// between two looks. Generous on purpose: everything waited for happens in this process, so a
/// loaded host does not decide the outcome, and the budget is only ever spent by a failure.
const BUDGET: Duration = Duration::from_secs(150);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The `TargetHealthStateEnum` values this suite reads back.
const INITIAL: &str = "initial";
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// The `TargetHealthReasonEnum` values an `initial` registration is reported under, as
/// `TargetHealth$Reason` words them: the registration being made, and the load balancer still
/// sending the checks its health is determined from.
const INITIAL_REASONS: [&str; 2] = ["Elb.RegistrationInProgress", "Elb.InitialHealthChecking"];

/// The `TargetHealthReasonEnum` values the two failure classes are reported under.
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";
const FAILED_HEALTH_CHECKS: &str = "Target.FailedHealthChecks";

/// One reading of a registration's health: the three members `TargetHealth` carries, each read
/// as empty where the answer carries none.
type Reading = (String, String, String);

/// One row: the destination its group's health check requests, the codes the group counts as
/// success, and the health its checks must reach.
struct Row {
    case: &'static str,
    group_name: &'static str,
    listener_port: &'static str,
    path: &'static str,
    matcher: &'static str,
    /// The `(state, reason, description)` the row's checks must settle on.
    settles_on: Reading,
}

/// The health a registration whose checks ACCEPT the answer is reported with: the state alone,
/// because "if the target state is healthy, a reason code is not provided" and no description
/// comes with it either.
fn healthy() -> Reading {
    (HEALTHY.to_string(), String::new(), String::new())
}

/// The health a registration that ANSWERED `status` outside its group's codes is reported with:
/// the model's reason for an answer those codes reject, described by the status the checks really
/// got back.
fn mismatched(status: u16) -> Reading {
    (
        UNHEALTHY.to_string(),
        RESPONSE_CODE_MISMATCH.to_string(),
        format!("Health checks failed with these codes: [{status}]"),
    )
}

/// The health a registration whose checks came back with no response at all is reported with:
/// there is no code to report a mismatch of.
fn failed_checks() -> Reading {
    (
        UNHEALTHY.to_string(),
        FAILED_HEALTH_CHECKS.to_string(),
        "Health checks failed".to_string(),
    )
}

/// The rows, spanning what the rule generalises over: what the function answers the check with,
/// and the shape of the codes that answer is judged against.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a function answering the one code the matcher names",
            group_name: "hc-seq-200",
            listener_port: "80",
            path: "/hc/200",
            matcher: "200",
            settles_on: healthy(),
        },
        Row {
            case: "a function answering a status inside the range the matcher names",
            group_name: "hc-seq-204",
            listener_port: "81",
            path: "/hc/204",
            matcher: "200-299",
            settles_on: healthy(),
        },
        Row {
            case: "a function answering 500 against the one code the matcher names",
            group_name: "hc-seq-500",
            listener_port: "82",
            path: "/hc/500",
            matcher: "200",
            settles_on: mismatched(500),
        },
        Row {
            case: "a function answering 503 against the range the matcher names",
            group_name: "hc-seq-503",
            listener_port: "83",
            path: "/hc/503",
            matcher: "200-299",
            settles_on: mismatched(503),
        },
        Row {
            case: "a handler that raises, so the run returned no response at all",
            group_name: "hc-seq-raise",
            listener_port: "84",
            path: "/hc/raise",
            matcher: "200",
            settles_on: failed_checks(),
        },
    ]
}

/// Create the row's `lambda` target group, grant the load balancer's invocation on it, register
/// the function and route a listener to it — each through the owning service's own API. Answers
/// the group's ARN.
async fn routed_group(
    router: &MultiServiceRouter,
    row: &Row,
    lb_arn: &str,
    function_arn: &str,
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.group_name));
    form.push(p("TargetType", "lambda"));
    form.push(p("HealthCheckEnabled", "true"));
    form.push(p("HealthCheckPath", row.path));
    form.push(p("Matcher.HttpCode", row.matcher));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
    form.push(p("HealthCheckTimeoutSeconds", TIMEOUT_SECONDS));
    form.push(p("HealthyThresholdCount", THRESHOLD));
    form.push(p("UnhealthyThresholdCount", THRESHOLD));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: CreateTargetGroup must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );
    let tg_arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body));

    // The grant that admits the invocation: the function's own resource-based policy, narrowed by
    // `aws:SourceArn` to this target group, written through Lambda's own `AddPermission`.
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        json!({
            "StatementId": format!("elbv2-hc-seq-{}", row.group_name),
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": tg_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: AddPermission must answer 201, got {status} {granted}",
        row.case
    );

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", function_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: RegisterTargets must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );

    // The listener that forwards to the group, which is what starts its health checking.
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", row.listener_port));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: CreateListener must answer 200, got {} body {}",
        row.case, reply.status, reply.body
    );
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
        "case {case:?}: expected one description for the one registered function, got {}",
        reply.body
    );
    let description = &descriptions[0];
    // The health's own element, because the override beside it carries members of the same
    // names ([`health_block`]).
    let health = health_block(description);
    (
        first_text(&health, "State").unwrap_or_default(),
        first_text(&health, "Reason").unwrap_or_default(),
        first_text(&health, "Description").unwrap_or_default(),
    )
}

/// Look at every row's target group, over and over, until each one reports the health its checks
/// decide — answering the whole HISTORY each was reported in, in order, with repeats collapsed.
///
/// The wait is for the CONDITION, never for a span: a row is done when its reading is the one its
/// checks reach, and the loop ends when every row is done or the budget is spent. One loop over
/// all the rows, because every row's checks run on the one clock at the same time: polling one
/// row to its verdict would let the next row's history pass unobserved.
async fn histories(
    router: &MultiServiceRouter,
    rows: &[Row],
    arns: &[String],
) -> BTreeMap<&'static str, Vec<Reading>> {
    let deadline = Instant::now() + BUDGET;
    let mut seen: BTreeMap<&'static str, Vec<Reading>> =
        rows.iter().map(|row| (row.case, Vec::new())).collect();
    loop {
        for (row, arn) in rows.iter().zip(arns) {
            let reading = described(router, row.case, arn).await;
            let history = seen.entry(row.case).or_default();
            if history.last() != Some(&reading) {
                history.push(reading);
            }
        }
        let settled = rows.iter().all(|row| {
            seen.get(row.case)
                .and_then(|history| history.last())
                .is_some_and(|reading| *reading == row.settles_on)
        });
        if settled || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: `healthy` is a finding of the health checks — a `lambda` registration is reported it
/// only once a check has found it passing, is `initial` (the registration being made, then the
/// checks still determining its health) until then, and therefore never reports `healthy` at any
/// point if its checks only ever fail; and no reading after a decided one reports `initial`
/// again.
#[tokio::test]
async fn a_lambda_target_is_reported_healthy_only_once_a_check_has_found_it_passing() {
    let (router, engine) = emulator();
    let (lb_arn, _dns_name) = create_load_balancer(&router, "lambda-hc-seq-alb").await;
    let function_arn = create_function(&router, &engine).await;

    // Every row's group is created, granted, registered and routed before anything is looked at,
    // so the checks of all of them run on their own cadence over the same span.
    let rows = rows();
    let mut arns: Vec<String> = Vec::new();
    for row in &rows {
        arns.push(routed_group(&router, row, &lb_arn, &function_arn).await);
    }

    let seen = histories(&router, &rows, &arns).await;

    for row in &rows {
        let history = seen
            .get(row.case)
            .unwrap_or_else(|| panic!("case {:?}: nothing was recorded", row.case));
        let states: Vec<&str> = history.iter().map(|(state, _, _)| state.as_str()).collect();

        // The health its checks decide, which is where the sequence ends.
        assert_eq!(
            history.last(),
            Some(&row.settles_on),
            "case {:?}: expected the registration to settle on {:?}, got the history {history:?}",
            row.case,
            row.settles_on
        );

        // Nothing but the row's own verdict is ever reported beside `initial`: a registration
        // whose checks fail is never `healthy`, and one whose checks pass is never `unhealthy`.
        let (settled_state, _, _) = &row.settles_on;
        for (state, reason, description) in history {
            assert!(
                state == INITIAL || state == settled_state,
                "case {:?}: expected only {INITIAL} and {settled_state} readings, got \
                 {state}/{reason}/{description:?} in {states:?}",
                row.case
            );
        }

        // Every `initial` reading is worded as one of the two the model gives the state, and no
        // reading after a decided one gives the state back.
        let mut decided = false;
        for (state, reason, _) in history {
            if state == INITIAL {
                assert!(
                    !decided,
                    "case {:?}: a decided registration is not reported {INITIAL} again, got \
                     {states:?}",
                    row.case
                );
                assert!(
                    INITIAL_REASONS.contains(&reason.as_str()),
                    "case {:?}: expected an {INITIAL} reading worded as one of \
                     {INITIAL_REASONS:?}, got {reason:?} in {states:?}",
                    row.case
                );
            } else {
                decided = true;
            }
        }
    }
}
