// SPDX-License-Identifier: CC0-1.0

//! WHAT a schedule's `RetryPolicy` bounds: end to end over the wire through the real
//! `MultiServiceRouter`, with IAM, Lambda and CloudWatch mounted beside EventBridge
//! Scheduler on ONE engine — so each row is judged where a client observes it, in the
//! invocations Lambda holds for the function and in the counts this service publishes
//! about its own occurrences.
//!
//! RULE: `Target$RetryPolicy` carries "information about the retry policy settings,
//! including the maximum age of an event, and the maximum number of times EventBridge
//! Scheduler will try to deliver the event to a target", and its
//! `MaximumRetryAttempts` is "the maximum number of retry attempts to make before the
//! request fails" (`smithy_query(query="shape",
//! shape="com.amazonaws.scheduler#RetryPolicy")`: `MaximumRetryAttempts` `@range 0..185`,
//! `MaximumEventAgeInSeconds` `@range 60..86400`). So the policy bounds the DELIVERY of
//! an occurrence's event and nothing else: a delivery the target ACCEPTED ends the
//! occurrence, however many attempts the policy would have admitted, and a target that
//! accepted the event and whose own code then failed has been delivered to — the failure
//! is the target's, not the delivery's. One occurrence therefore makes exactly one call
//! to the target, and this service reports no failed target call for it.
//!
//! The rows span the class rather than one recorded case: they differ along
//! `MaximumRetryAttempts` — both published bounds of its range and two figures inside it
//! — and along whether the policy also states `MaximumEventAgeInSeconds`. Each row runs
//! in its own schedule GROUP, which is the dimension the counts are published per, so a
//! row's counts are its own.
//!
//! The function every row targets RAISES on each invocation, which is how its run fails
//! while the invocation itself was accepted — read where a client reads it, in the error
//! count Lambda publishes for the function. Every wait here looks again until the
//! condition holds, on a generous deadline, and a count expected to stay quiet is looked
//! at repeatedly across its own window. Nothing is asserted about elapsed time.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::metrics::{
    ERRORS_METRIC, FUNCTION_NAME_DIMENSION, METRIC_NAMESPACE as LAMBDA_METRIC_NAMESPACE,
};
use service_com_amazonaws_scheduler_awschronosservice::state::schedule_metrics::{
    INVOCATION_ATTEMPT_COUNT, INVOCATION_DROPPED_COUNT, TARGET_ERROR_COUNT,
};
use smithy_server_runtime::MultiServiceRouter;

// The readers of what a service publishes about its own work, which this crate's metrics
// suite owns (`tests/all/invocation_metrics.rs`).
use crate::invocation_metrics::{sum_in, sum_of};
// The emulator's Lambda setup, its one `CreateFunction` call, its front-door call and
// the reader of the invocations Lambda holds, which this crate's function-target suites
// share (`tests/all/per_schedule_target_input.rs`).
use crate::per_schedule_target_input::{
    given_function_with_code, invoked_payload_counts, lambda, FUNCTIONS_URI,
};
// The scheduler call, the execution-role builder and the instant helpers this crate's
// invocation suites share (`tests/all/schedule_invocation.rs`).
use crate::schedule_invocation::{
    at_expression_body, given_execution_role_granting, scheduler, seconds_from_now, POLL_INTERVAL,
};

/// The IAM action an invocation of a function target is authorized by, as
/// `Target$RoleArn`'s own documentation names the grant such a target needs.
const INVOKE_FUNCTION: &str = "lambda:InvokeFunction";

/// The function every row invokes, whose handler RAISES: the invocation is accepted and
/// the RUN of it fails, which is the failure a delivery bound must not act on.
const RAISING_HANDLER: &str =
    "def lambda_handler(event, context):\n    raise RuntimeError('the handler refused this \
     event')\n";

/// The entry of the deployment package the handler above is written as, and the
/// `Handler` naming it.
const HANDLER_ENTRY: &str = "lambda_function.py";
/// See [`HANDLER_ENTRY`].
const HANDLER_NAME: &str = "lambda_function.lambda_handler";

/// The `State` Lambda serves a function it has finished provisioning as, which is the one
/// an invocation of it is accepted in.
const FUNCTION_ACTIVE: &str = "Active";

/// How far ahead of the writes the one occurrence of every row falls.
///
/// It is read AFTER the groups exist and before the schedules are written, and is far
/// enough ahead that every row's schedule is written while the instant is still to come
/// however slow the host is — a one-time instant already behind the clock when a schedule
/// states it names no occurrence at all, so a tight margin would make the suite's subject
/// the host's speed.
const OCCURRENCE_IN: u64 = 15;

/// How long the suite waits for the invocations and counts it expects: generous rather
/// than tight, because a host running every suite at once is slow at everything.
const PATIENCE: Duration = Duration::from_secs(90);

/// How long the suite keeps looking once they hold, so a second delivery of one
/// occurrence has had its chance to appear.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// One row: the schedule group it runs in, and the retry policy its target states.
struct Row {
    case: &'static str,
    group: &'static str,
    /// `MaximumRetryAttempts`, inside the modelled range `0..185`.
    attempts: i64,
    /// `MaximumEventAgeInSeconds`, inside the modelled range `60..86400`, or `None` for a
    /// policy that states only the attempts.
    event_age: Option<i64>,
}

/// The rows differ along `MaximumRetryAttempts` — the floor of its range, two figures
/// inside it and its published ceiling — and along whether the policy states an event age
/// beside it.
const ROWS: &[Row] = &[
    Row {
        case: "a policy admitting no retry at all, the floor of the modelled range",
        group: "attempts-none",
        attempts: 0,
        event_age: None,
    },
    Row {
        case: "a policy admitting two retries, with the longest event age",
        group: "attempts-two",
        attempts: 2,
        event_age: Some(86_400),
    },
    Row {
        case: "a policy admitting ten retries",
        group: "attempts-ten",
        attempts: 10,
        event_age: None,
    },
    Row {
        case: "a policy admitting the most retries the range allows, with the shortest \
               event age",
        group: "attempts-most",
        attempts: 185,
        event_age: Some(60),
    },
];

/// Every row: the one occurrence makes exactly ONE call to the function, whose own run
/// reports a `FunctionError`, and this service publishes one attempt made with no failed
/// target call and no invocation given up on — whatever the row's `MaximumRetryAttempts`
/// states.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivery_the_target_accepted_is_not_retried_however_the_policy_reads() {
    let suite = "a function target whose handler raises";
    let (emulator, engine) = emulator_with_lambda_and_cloudwatch();
    let function = "raising-handler";
    let function_arn = given_raising_function(&emulator, suite, function).await;
    let role = given_execution_role_granting(
        &emulator,
        suite,
        "retry-policy-role",
        &[(INVOKE_FUNCTION, function_arn.as_str())],
    )
    .await;

    let occurrence = {
        // Every row's GROUP is written first, so the instant the rows share is read with
        // only the schedules themselves still to write.
        for row in ROWS {
            let case = row.case;
            let (status, created) = scheduler(
                &emulator,
                "POST",
                &format!("/schedule-groups/{}", row.group),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateScheduleGroup answered {status} {created}"
            );
        }
        seconds_from_now(OCCURRENCE_IN)
    };
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let mut policy = json!({ "MaximumRetryAttempts": row.attempts });
        if let Some(age) = row.event_age {
            policy["MaximumEventAgeInSeconds"] = json!(age);
        }
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/retry-{index}"),
            Some(json!({
                "GroupName": row.group,
                "ScheduleExpression": format!("at({})", at_expression_body(occurrence)),
                "ScheduleExpressionTimezone": "UTC",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "State": "ENABLED",
                "Target": {
                    "Arn": function_arn,
                    "RoleArn": role,
                    "Input": payload_of(row),
                    "RetryPolicy": policy,
                },
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );
    }

    // Every row's occurrence is waited for together, so one slow delivery does not spend
    // another row's patience.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let held = invocations_of(&engine, function);
        let pending: Vec<&str> = ROWS
            .iter()
            .filter(|row| !held.contains_key(&payload_of(row)))
            .map(|row| row.case)
            .collect();
        if pending.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected every row's occurrence to invoke the function; still waiting for \
             {pending:?}, Lambda holds {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    // The RUN of each accepted invocation FAILED, which the function's own service
    // publishes as an error of that function — one observation per run that reported a
    // function error. So what this suite watches is a target that failed on its own, and
    // never a delivery that did not happen.
    let runs = ROWS.len() as f64;
    let deadline = Instant::now() + PATIENCE;
    loop {
        let errored = function_errors(&emulator, function).await;
        if errored >= runs {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected the handler of every row's invocation to have failed: expected at \
             least {runs} of {ERRORS_METRIC} for the function, got {errored}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // One occurrence, one call to the target, and the counts this service publishes say
    // the same: one attempt made, no failed target call, nothing given up on. Both are
    // looked at repeatedly rather than once at the end of a sleep.
    let deadline = Instant::now() + QUIET_WINDOW;
    loop {
        let held = invocations_of(&engine, function);
        for row in ROWS {
            let case = row.case;
            let payload = payload_of(row);
            let delivered = held.get(&payload).copied().unwrap_or_default();
            assert_eq!(
                delivered, 1,
                "case {case:?}: expected the occurrence to invoke the function exactly once \
                 with {payload:?} however the policy reads, got {delivered} invocation(s): \
                 {held:?}"
            );
            for (metric, expected) in [
                (INVOCATION_ATTEMPT_COUNT, 1.0),
                (TARGET_ERROR_COUNT, 0.0),
                (INVOCATION_DROPPED_COUNT, 0.0),
            ] {
                let published = sum_of(&emulator, row.group, metric).await;
                assert!(
                    (published - expected).abs() < f64::EPSILON,
                    "case {case:?}: expected {metric} of group {:?} to hold {expected}, got \
                     {published}",
                    row.group
                );
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `Input` one row's schedule hands the function, which is how an invocation Lambda
/// holds is attributed to the row that made it.
fn payload_of(row: &Row) -> String {
    json!({ "marker": "retry-policy", "group": row.group }).to_string()
}

/// How many runs of `function` reported a function error, as the function's own service
/// publishes it and a client reads it.
async fn function_errors(router: &MultiServiceRouter, function: &str) -> f64 {
    sum_in(
        router,
        LAMBDA_METRIC_NAMESPACE,
        ERRORS_METRIC,
        FUNCTION_NAME_DIMENSION,
        function,
    )
    .await
}

/// The asynchronous invocations Lambda holds for `function`, counted by the payload each
/// one carries ([`invoked_payload_counts`]).
fn invocations_of(engine: &Arc<engine::InMemoryEngine>, function: &str) -> BTreeMap<String, usize> {
    invoked_payload_counts(engine, function)
}

/// EventBridge Scheduler, Lambda, IAM and CloudWatch on ONE engine — the shape
/// `core/router/src/lib.rs` mounts them in — with the engine itself, so the suite can read
/// the invocations Lambda holds beside the counts CloudWatch serves.
fn emulator_with_lambda_and_cloudwatch() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// A function of `name` whose handler RAISES, waited for until Lambda serves it as
/// `Active`, and the ARN a schedule names it by.
///
/// The package is this suite's own: the shared creator's carries no handler module,
/// because the suites it serves read only the payload an invocation carried, while this
/// one needs the RUN to happen and to fail.
async fn given_raising_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let arn =
        given_function_with_code(router, case, name, HANDLER_NAME, &deployment_package()).await;
    // A function Lambda has not finished provisioning refuses an invocation, which would
    // be a failed DELIVERY rather than the function-level failure under test — so the
    // schedule is written only once Lambda serves the function as ready.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (status, read) = lambda(router, "GET", &format!("{FUNCTIONS_URI}/{name}"), None).await;
        if status == 200 && read["Configuration"]["State"].as_str() == Some(FUNCTION_ACTIVE) {
            break arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Lambda never served the function as {FUNCTION_ACTIVE}, last \
             answer was {status} {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The deployment package the handler travels in, base64 as `FunctionCode$ZipFile`
/// carries one: a .zip holding [`HANDLER_ENTRY`], written in memory the way a caller
/// uploads one.
fn deployment_package() -> String {
    let mut archive = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut archive);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file(HANDLER_ENTRY, options)
            .expect("the archive's entry starts");
        writer
            .write_all(RAISING_HANDLER.as_bytes())
            .expect("the entry's bytes are written");
        writer.finish().expect("the archive is finished");
    }
    base64::engine::general_purpose::STANDARD.encode(archive.into_inner())
}

