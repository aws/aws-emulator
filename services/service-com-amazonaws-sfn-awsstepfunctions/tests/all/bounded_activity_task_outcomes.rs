// SPDX-License-Identifier: CC0-1.0

//! End-to-end test for ONE activity whose queue serves SEVERAL executions with tasks
//! bounded differently: each waiting task is handed to its own poll, each token answers
//! only the execution it was minted for, and the counters of that one activity split by
//! the OUTCOME of each task while each execution's outcome stays with its own state
//! machine — driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions and CloudWatch mounted on ONE engine, so the
//! series are read back through CloudWatch's own `GetMetricStatistics` exactly as a
//! client charting them does.
//!
//! The rule under test: an activity's queue is shared by every `Task` state that names
//! it, and what closes one of its tasks is decided per TASK — the bound the scheduling
//! state declared, or the answer that task's own worker sent while it still held — so one
//! activity reports as many scheduled and started tasks as were queued and taken, with
//! each close counted in the series of that task's own outcome, and each execution's
//! outcome counted under the state machine it belongs to rather than following a sibling
//! execution's fate.
//!
//! What this adds over its two neighbours: `crate::activity_timeout_late_answer` drives
//! the late-answer hand-off with one activity per fate, and
//! `crate::states_metric_families` meters one activity whose workers all behave alike.
//! Neither has one activity's queue holding tasks that END DIFFERENTLY, which is what
//! separates per-task accounting from per-activity accounting.
//!
//! The rows differ along what the rule generalises over: the bound the scheduling state
//! declares, what that task's worker does inside it — reports a result, reports a failure,
//! or goes quiet — and so which series the task's close and its execution's outcome land
//! in.
//!
//! The harness (`Metered`, `activity_dimensions`, `machine_dimensions`) is the one
//! `states_metric_families` builds, the create/start/claim helpers are
//! `activity_timeout_late_answer`'s and the definition builder is `activity_tasks`';
//! `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::<module>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{asl::TIMEOUT_ERROR, observability};

use crate::activity_tasks::activity_definition;
use crate::activity_timeout_late_answer::{claimed, create_activity, start_execution};
use crate::states_metric_families::{activity_dimensions, machine_dimensions, Metered};

/// What the worker that reports a RESULT inside its bound answers with.
const IN_TIME_OUTPUT: &str = r#"{"handled":true}"#;

/// What the worker that reports a FAILURE inside its bound answers with.
const IN_TIME_ERROR: &str = "WorkerFailed";

/// What this row's worker does with the task it claimed, while the bound still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Worker {
    /// It reports the task's result.
    Succeeds,
    /// It reports the task failed.
    Fails,
    /// It goes quiet, so the bound the state declared is what closes the task.
    GoesQuiet,
}

/// One subject: a state machine whose `Task` state hands its work to the SHARED activity
/// under a bound of its own, and what its execution must report once its task has closed.
struct Row {
    case: &'static str,
    /// The bound that state machine's `Task` state declares, in seconds.
    bound_seconds: u32,
    /// What this row's worker does with the task it was handed.
    worker: Worker,
    /// The terminal status its execution reaches.
    status: &'static str,
    /// The `error` it reports, and `None` for one that reports none.
    error: Option<&'static str>,
    /// The `output` it reports, and `None` for one that carries no output member at all.
    output: Option<&'static str>,
    /// What its own state machine's succeeded, failed and timed-out execution series must
    /// hold.
    counted: [f64; 3],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a task of the shared queue left unanswered under a one-second bound",
            bound_seconds: 1,
            worker: Worker::GoesQuiet,
            status: "FAILED",
            error: Some(TIMEOUT_ERROR),
            output: None,
            counted: [0.0, 1.0, 0.0],
        },
        Row {
            case: "a task of the shared queue answered inside a generous bound",
            bound_seconds: 300,
            worker: Worker::Succeeds,
            status: "SUCCEEDED",
            error: None,
            output: Some(IN_TIME_OUTPUT),
            counted: [1.0, 0.0, 0.0],
        },
        Row {
            // The third outcome of the same queue: the worker's OWN failure, which the
            // execution reports as the worker worded it rather than as a timeout.
            case: "a task of the shared queue failed by its worker inside a generous bound",
            bound_seconds: 300,
            worker: Worker::Fails,
            status: "FAILED",
            error: Some(IN_TIME_ERROR),
            output: None,
            counted: [0.0, 1.0, 0.0],
        },
    ]
}

/// How many rows had their task closed by `worker`.
#[allow(clippy::cast_precision_loss)]
fn closed_by(rows: &[Row], worker: Worker) -> f64 {
    rows.iter().filter(|row| row.worker == worker).count() as f64
}

/// One activity's queue serves every state machine that names it: each task is handed out
/// once, closed on its own terms, and counted in the series of its own outcome — while
/// each execution's outcome is counted under its own state machine alone.
#[tokio::test]
async fn one_activitys_tasks_are_closed_and_counted_one_by_one() {
    let metered = Metered::new();
    let case = "one activity, one task per fate";

    // ONE activity, and one state machine per row, every one of them handing its work to
    // that same activity.
    let activity = create_activity(&metered, case, "shared-queue").await;
    let rows = rows();
    let mut machines: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let definition =
            activity_definition(&activity, json!({ "TimeoutSeconds": row.bound_seconds }));
        machines.push(
            metered
                .machine(case, &format!("shared-queue-{index}"), &definition)
                .await,
        );
    }

    // Every execution is started BEFORE any poll, so the activity's queue really holds
    // several waiting tasks at once. A queued task is bounded by nothing — an activity
    // task's own clock starts when a poll takes it — so none of them can close before it
    // is claimed.
    let mut executions: Vec<String> = Vec::new();
    for (index, machine) in machines.iter().enumerate() {
        let input = json!({ "row": index }).to_string();
        executions.push(start_execution(&metered, rows[index].case, machine, &input).await);
    }

    // Each task is handed out ONCE, and the token of each identifies the execution it was
    // minted for: the input it carries is that execution's own.
    let mut tokens: Vec<Option<String>> = vec![None; rows.len()];
    for _ in 0..rows.len() {
        let (token, delivered) = claimed(&metered, case, &activity).await;
        let row = delivered
            .get("row")
            .and_then(Value::as_u64)
            .and_then(|row| usize::try_from(row).ok())
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a task of this suite, got {delivered}")
            });
        assert!(
            row < rows.len() && tokens[row].is_none(),
            "case {case:?}: expected one task per row, got another one for row {row}"
        );
        assert!(
            !token.is_empty(),
            "case {case:?}: expected a non-empty taskToken for row {row}"
        );
        tokens[row] = Some(token);
    }

    // The workers answer — or stay quiet — each under its own row's bound.
    for (index, row) in rows.iter().enumerate() {
        let token = tokens[index]
            .as_deref()
            .unwrap_or_else(|| panic!("case {:?}: this row's task was handed out", row.case));
        let answer = match row.worker {
            Worker::Succeeds => Some((
                "SendTaskSuccess",
                json!({ "taskToken": token, "output": IN_TIME_OUTPUT }),
            )),
            Worker::Fails => Some((
                "SendTaskFailure",
                json!({ "taskToken": token, "error": IN_TIME_ERROR }),
            )),
            Worker::GoesQuiet => None,
        };
        if let Some((operation, request)) = answer {
            metered.ok(row.case, operation, &request).await;
        }
    }

    // Each execution reports the outcome of its OWN task, whatever its sibling's was, and
    // its state machine's series carry exactly that outcome.
    for (index, row) in rows.iter().enumerate() {
        let settled = metered.settled(row.case, &executions[index]).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some(row.status),
            "case {:?}: expected the execution to end {}, got {settled}",
            row.case,
            row.status
        );
        assert_eq!(
            settled.get("error").and_then(Value::as_str),
            row.error,
            "case {:?}: expected the error {:?}, got {settled}",
            row.case,
            row.error
        );
        assert_eq!(
            settled.get("output").and_then(Value::as_str),
            row.output,
            "case {:?}: expected the output {:?}, got {settled}",
            row.case,
            row.output
        );

        let of_machine = machine_dimensions(&machines[index]);
        for (metric, expected) in [
            (observability::EXECUTIONS_SUCCEEDED_METRIC, row.counted[0]),
            (observability::EXECUTIONS_FAILED_METRIC, row.counted[1]),
            (observability::EXECUTIONS_TIMED_OUT_METRIC, row.counted[2]),
        ] {
            let summed = metered.summed(row.case, metric, &of_machine).await;
            assert_eq!(
                summed, expected,
                "case {:?}: expected {metric} to count {expected} under this row's own state \
                 machine, got {summed}",
                row.case
            );
        }
    }

    // And the ONE activity reports every task it queued and handed out, with the closes
    // split across the series of what closed each — which is what per-task accounting
    // means: the shared queue does not report two timeouts because one task timed out,
    // nor two successes because one worker answered.
    let of_activity = activity_dimensions(&activity);
    #[allow(clippy::cast_precision_loss)]
    let queued = rows.len() as f64;
    for (metric, expected) in [
        (observability::ACTIVITIES_SCHEDULED_METRIC, queued),
        (observability::ACTIVITIES_STARTED_METRIC, queued),
        (
            observability::ACTIVITIES_SUCCEEDED_METRIC,
            closed_by(&rows, Worker::Succeeds),
        ),
        (
            observability::ACTIVITIES_FAILED_METRIC,
            closed_by(&rows, Worker::Fails),
        ),
        (
            observability::ACTIVITIES_TIMED_OUT_METRIC,
            closed_by(&rows, Worker::GoesQuiet),
        ),
    ] {
        let summed = metered.summed(case, metric, &of_activity).await;
        assert_eq!(
            summed, expected,
            "case {case:?}: expected {metric} to count {expected} over the shared queue, got \
             {summed}"
        );
    }
}
