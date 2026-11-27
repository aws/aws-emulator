// SPDX-License-Identifier: CC0-1.0

//! WHICH definition a schedule's occurrences are dispatched from once `UpdateSchedule`
//! has answered: end to end over the wire through the real `MultiServiceRouter`, with
//! Amazon SQS, IAM and Lambda mounted beside EventBridge Scheduler on ONE engine — so
//! each row is judged where a client observes it, on the target the update names and on
//! the one the schedule named before it.
//!
//! RULE: `UpdateSchedule` states a schedule's definition WHOLESALE — `Target` and
//! `FlexibleTimeWindow` are required members of that write
//! (`smithy_query(query="operation", operation="UpdateSchedule")`), so the record it
//! writes replaces the one before it — and every occurrence is dispatched from the
//! record as it stands. Once the write has answered 200, no occurrence carries anything
//! of the previous definition: the invocation goes to the target that record names,
//! carrying the `Input` it names, and a definition that names no occurrence at all
//! invokes nothing.
//!
//! The rows span the class rather than one recorded case: they differ along WHAT the
//! update changes about the dispatch — the target's ARN (another resource of the same
//! kind), the target's KIND (a queue re-pointed to a function, which is invoked through
//! a different API entirely), and the `Input` handed to a target that does not change.
//! The `State` that stops the series is its own suite below, because the silence it asks
//! for has to outlast a whole period of the series it stopped.
//!
//! Every wait here looks again until the condition it waits on holds, on a generous
//! deadline, and a target expected to stay quiet is an invariant: it is looked at
//! repeatedly across its own window rather than once at the end of a sleep. Nothing is
//! asserted about elapsed time.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The emulator with Lambda beside the rest, the function creator and the reader of the
// invocation Lambda holds, which this crate's function-target suites share
// (`tests/all/per_schedule_target_input.rs`).
use crate::per_schedule_target_input::{emulator_with_lambda, given_function, invoked_payloads};
// The scheduler call, the queue setup, the execution-role builder and the queue reader
// this crate's invocation suites share (`tests/all/schedule_invocation.rs`).
use crate::schedule_invocation::{
    bodies, drain, given_execution_role_granting, given_queue, scheduler, POLL_INTERVAL,
};

/// The IAM action an invocation of a QUEUE target is authorized by, which the one
/// execution role of a row carries so that what a row observes is the dispatch rather
/// than a permission.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The IAM action an invocation of a FUNCTION target is authorized by. See
/// [`SEND_MESSAGE`].
const INVOKE_FUNCTION: &str = "lambda:InvokeFunction";

/// The series every row states: the shortest cadence a `rate(...)` expression can state,
/// whose first occurrence is the instant the series opens at — the write that armed it —
/// so a row observes the definition in force without waiting a period for it.
const EVERY_MINUTE: &str = "rate(1 minute)";

/// The period that expression states, which is how long a series the update STOPPED has
/// to be watched for before its next occurrence can be said not to have happened.
const PERIOD: Duration = Duration::from_secs(60);

/// The `Input` the schedule is CREATED with.
const INPUT_BEFORE: &str = r#"{"marker":"the definition before the update"}"#;

/// The `Input` the update states, for the row that changes it. See [`INPUT_BEFORE`].
const INPUT_AFTER: &str = r#"{"marker":"the definition after the update"}"#;

/// How long a row waits for a delivery it expects: generous rather than tight, because a
/// host running every suite at once is slow at everything.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long a row keeps watching the target the previous definition named, once the
/// updated definition has been observed. Nothing is asserted about how long anything
/// took: the window is only how long the absence is watched for.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// The `State` a schedule that invokes its target is in, and the one that invokes
/// nothing: the two values `ScheduleState` admits
/// (`smithy_query(query="shape", shape="com.amazonaws.scheduler#ScheduleState")`).
const ENABLED_STATE: &str = "ENABLED";
/// See [`ENABLED_STATE`].
const DISABLED_STATE: &str = "DISABLED";

/// What one row's `UpdateSchedule` changes about the schedule's dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// The target's ARN, re-pointed to another queue — another resource of the same kind,
    /// invoked through the same API.
    Arn,
    /// The target's KIND: a queue re-pointed to a function, whose invocation is a
    /// different API of a different service.
    Kind,
    /// The `Input` handed to a target the update leaves as it was.
    Input,
}

/// One row: what the update changes.
struct Row {
    case: &'static str,
    change: Change,
}

/// The rows whose update RE-POINTS the target, within one kind and across two kinds.
const RETARGETING_ROWS: &[Row] = &[
    Row {
        case: "the target re-pointed to another queue",
        change: Change::Arn,
    },
    Row {
        case: "the target re-pointed to a function, another kind of target entirely",
        change: Change::Kind,
    },
];

/// The rows whose update keeps the target and changes what the dispatch does with it.
const REDEFINING_ROWS: &[Row] = &[Row {
    case: "the same target handed a different Input",
    change: Change::Input,
}];

/// A whole `CreateSchedule`/`UpdateSchedule` body: both carry the same members, which is
/// what makes the update a statement of the whole definition.
fn definition(target_arn: &str, role_arn: &str, input: &str, state: &str) -> Value {
    json!({
        "ScheduleExpression": EVERY_MINUTE,
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "State": state,
        "Target": { "Arn": target_arn, "RoleArn": role_arn, "Input": input },
    })
}

/// An update that re-points the target is followed by the dispatch: the target the update
/// names is invoked, and the one it was re-pointed away from is not invoked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_re_points_the_target_stops_dispatching_to_the_previous_one() {
    the_updated_definition_is_what_is_dispatched(RETARGETING_ROWS).await;
}

/// An update that keeps the target is followed just as closely: the target is handed the
/// `Input` the updated record states and never the one before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_changes_the_input_is_what_dispatch_follows() {
    the_updated_definition_is_what_is_dispatched(REDEFINING_ROWS).await;
}

/// An update that moves `State` to `DISABLED` stops the series: the target the schedule
/// was invoking receives nothing more for longer than the period of that series, so the
/// occurrence the previous definition would have had has come and gone — and an update
/// that enables it again starts it, which is what proves the silence was the schedule's
/// state rather than a clock that had stopped or a target that had become unreachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_disables_the_schedule_stops_the_series_until_one_enables_it_again() {
    let case = "the schedule disabled, so its series names no occurrence";
    let (emulator, _engine) = emulator_with_lambda();
    let (url, queue_arn) = given_queue(&emulator, case, "disabled-target").await;
    let role = given_execution_role_granting(
        &emulator,
        case,
        "disabled-role",
        &[(SEND_MESSAGE, queue_arn.as_str())],
    )
    .await;

    let name = "disabled";
    let (status, created) = scheduler(
        &emulator,
        "POST",
        &format!("/schedules/{name}"),
        Some(definition(&queue_arn, &role, INPUT_BEFORE, ENABLED_STATE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );
    awaited_delivery(&emulator, case, &url, INPUT_BEFORE).await;
    drain(&emulator, &url).await;

    let (status, updated) = scheduler(
        &emulator,
        "PUT",
        &format!("/schedules/{name}"),
        Some(definition(&queue_arn, &role, INPUT_BEFORE, DISABLED_STATE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateSchedule answered {status} {updated}"
    );
    // Longer than one period of the series the update stopped: the occurrence the
    // previous definition had next falls inside this window, so an emulator that kept it
    // armed delivers inside it.
    let deadline = Instant::now() + PERIOD + QUIET_WINDOW;
    loop {
        let held = bodies(&emulator, &url).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected a disabled schedule to invoke nothing, got {held:?}"
        );
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Enabling it again arms the series afresh, which is what makes the silence above the
    // state's and nothing else's.
    let (status, enabled) = scheduler(
        &emulator,
        "PUT",
        &format!("/schedules/{name}"),
        Some(definition(&queue_arn, &role, INPUT_AFTER, ENABLED_STATE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateSchedule answered {status} {enabled}"
    );
    awaited_delivery(&emulator, case, &url, INPUT_AFTER).await;
}

/// Wait until the queue at `url` holds `body`, or fail naming what it holds instead.
async fn awaited_delivery(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    url: &str,
    body: &str,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let held = bodies(router, url).await;
        if held.iter().any(|held| held == body) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivery of {body:?}, got {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Drive one table: create the schedule, watch its series reach the target it was
/// created with, update it, and assert that what follows the update is the updated
/// record alone.
async fn the_updated_definition_is_what_is_dispatched(rows: &[Row]) {
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (emulator, engine) = emulator_with_lambda();
        let (previous_url, previous_arn) =
            given_queue(&emulator, case, &format!("previous-target-{index}")).await;
        let (updated_url, updated_arn) =
            given_queue(&emulator, case, &format!("updated-target-{index}")).await;
        let function = format!("updated-function-{index}");
        let function_arn = given_function(&emulator, case, &function).await;
        // ONE role, allowed every call any row's definition could make, so what a row
        // observes is which definition the dispatch followed and never a permission.
        let role = given_execution_role_granting(
            &emulator,
            case,
            &format!("retarget-role-{index}"),
            &[
                (SEND_MESSAGE, previous_arn.as_str()),
                (SEND_MESSAGE, updated_arn.as_str()),
                (INVOKE_FUNCTION, function_arn.as_str()),
            ],
        )
        .await;

        let name = format!("retargeted-{index}");
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(definition(&previous_arn, &role, INPUT_BEFORE, ENABLED_STATE)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );

        // The series IS being dispatched to the definition in force, which is what makes
        // the absence asserted after the update an absence of something that was
        // happening.
        awaited_delivery(&emulator, case, &previous_url, INPUT_BEFORE).await;
        // Everything the definition before the update delivered is taken off the queue,
        // so a later read reports only what the UPDATED definition dispatched.
        drain(&emulator, &previous_url).await;

        let (target_arn, input, state) = match row.change {
            Change::Arn => (updated_arn.as_str(), INPUT_BEFORE, ENABLED_STATE),
            Change::Kind => (function_arn.as_str(), INPUT_BEFORE, ENABLED_STATE),
            Change::Input => (previous_arn.as_str(), INPUT_AFTER, ENABLED_STATE),
        };
        let (status, updated) = scheduler(
            &emulator,
            "PUT",
            &format!("/schedules/{name}"),
            Some(definition(target_arn, &role, input, state)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSchedule answered {status} {updated}"
        );
        // The record a client READS is the updated one, so the dispatch below is judged
        // against the definition the service reports for itself.
        let (status, read) = scheduler(
            &emulator,
            "GET",
            &format!("/schedules/{name}?groupName=default"),
            None,
        )
        .await;
        assert_eq!(
            (
                status,
                read["Target"]["Arn"].as_str(),
                read["Target"]["Input"].as_str(),
                read["State"].as_str()
            ),
            (200, Some(target_arn), Some(input), Some(state)),
            "case {case:?}: expected GetSchedule to report the updated definition, got \
             {status} {read}"
        );

        // What the updated record states is dispatched, wherever that record points.
        let deadline = Instant::now() + PATIENCE;
        loop {
            let reached = match row.change {
                Change::Arn => bodies(&emulator, &updated_url)
                    .await
                    .iter()
                    .any(|body| body == INPUT_BEFORE),
                Change::Kind => invoked_payloads(&engine, &function).contains(INPUT_BEFORE),
                Change::Input => bodies(&emulator, &previous_url)
                    .await
                    .iter()
                    .any(|body| body == INPUT_AFTER),
            };
            if reached {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the updated definition was never dispatched"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // Nothing of the PREVIOUS definition is dispatched again: the queue the schedule
        // was re-pointed away from holds nothing at all, a target the update kept holds
        // only what the updated record hands it, and a function the schedule never named
        // holds no invocation.
        let expected: BTreeSet<String> = match row.change {
            Change::Input => [INPUT_AFTER.to_string()].into_iter().collect(),
            _ => BTreeSet::new(),
        };
        let deadline = Instant::now() + QUIET_WINDOW;
        loop {
            let held: BTreeSet<String> =
                bodies(&emulator, &previous_url).await.into_iter().collect();
            assert_eq!(
                held, expected,
                "case {case:?}: expected the target of the previous definition to hold \
                 {expected:?}, got {held:?}"
            );
            if row.change != Change::Kind {
                let invoked = invoked_payloads(&engine, &function);
                assert!(
                    invoked.is_empty(),
                    "case {case:?}: expected the function no definition of this schedule \
                     named to hold no invocation, got {invoked:?}"
                );
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}
