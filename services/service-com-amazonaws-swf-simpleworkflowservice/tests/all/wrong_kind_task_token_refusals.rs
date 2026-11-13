// SPDX-License-Identifier: CC0-1.0

//! The wrong-kind-handle rule of [`super::cross_kind_task_token_refusals`], carried
//! across the dimension that suite holds fixed: WHICH domain and WHICH run the task the
//! supplied `taskToken` addresses lives in. End to end over the service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`),
//! through the same `MultiServiceRouter` the emulator mounts in production, and reusing
//! that suite's seeding, polling and message helpers rather than restating them.
//!
//! The rule: a call handed a `taskToken` it cannot act on answers the modelled
//! `UnknownResourceFault` reporting the `scheduledEventId` of the task THAT token
//! addresses, read out of the stored history of the token's own run. A task token carries
//! neither domain nor run on the wire, so the handle alone locates its task: two runs in
//! two domains, each holding a decision task and an activity task scheduled at DIFFERENT
//! event ids, are what make a refusal that reports another run's task — or the `EventId`
//! member's default — visible.
//!
//! Rows vary which domain the addressed task lives in, where in its own run's history
//! that task sits, which kind of task the handle is on, and which of the five
//! token-taking operations receives it.

use serde_json::{json, Value};

use super::cross_kind_task_token_refusals::{
    poll_activity_task, poll_decision_task, seed, unknown_activity, unknown_decision_task,
    ACTIVITY_SIDE, WORKFLOW_ID, WORKFLOW_TYPE,
};
use super::workflow_execution_lifecycle::{error_code, string_at, whole_history, Emulator};

/// The activity type [`seed`] registers in every domain, as its decisions name it.
const ACTIVITY_TYPE: (&str, &str) = ("cross-kind-activity", "1.0");

/// The `activityId` the one scheduled activity task of each run carries.
const ACTIVITY_ID: &str = "act-1";

/// One run of one domain, the two handles it handed out, and the scheduling event each of
/// those handles addresses.
struct Run {
    domain: &'static str,
    /// How many signals the run took before its decider scheduled the activity task,
    /// which is what pushes this run's event ids away from the other run's.
    signals: usize,
    run_id: String,
    decision_token: String,
    decision_scheduled: i64,
    activity_token: String,
    activity_scheduled: i64,
}

impl Run {
    /// Seed `domain`, start one run there and hand out one decision task and one activity
    /// task of it. `signals` signals before the scheduling decision give the run event
    /// ids of its own.
    async fn seeded(emulator: &Emulator, domain: &'static str, signals: usize) -> Self {
        seed(emulator, domain).await;
        let started = emulator
            .ok(
                domain,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": WORKFLOW_ID,
                    "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                }),
            )
            .await;
        let run_id = string_at(&started, "/runId");

        // Each signal appends its own event, so this run schedules its tasks at ids the
        // other run of this test does not share.
        for index in 0..signals {
            emulator
                .ok(
                    domain,
                    "SignalWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "workflowId": WORKFLOW_ID,
                        "runId": run_id,
                        "signalName": format!("nudge-{index}"),
                    }),
                )
                .await;
        }

        let (decision_token, decision_scheduled) =
            poll_decision_task(emulator, domain, domain).await;
        emulator
            .ok(
                domain,
                "RespondDecisionTaskCompleted",
                &json!({
                    "taskToken": decision_token,
                    "decisions": [{
                        "decisionType": "ScheduleActivityTask",
                        "scheduleActivityTaskDecisionAttributes": {
                            "activityId": ACTIVITY_ID,
                            "activityType": {
                                "name": ACTIVITY_TYPE.0,
                                "version": ACTIVITY_TYPE.1,
                            },
                        },
                    }],
                }),
            )
            .await;
        let (activity_token, activity_scheduled) =
            poll_activity_task(emulator, domain, domain, &run_id).await;

        Self {
            domain,
            signals,
            run_id,
            decision_token,
            decision_scheduled,
            activity_token,
            activity_scheduled,
        }
    }

    /// How this run is named in a case label.
    fn label(&self) -> String {
        format!("{} after {} signal(s)", self.domain, self.signals)
    }
}

#[tokio::test]
async fn a_task_fault_names_the_task_of_the_handle_s_own_domain_and_run() {
    let emulator = Emulator::new();
    let runs = [
        Run::seeded(&emulator, "cross-domain-a", 0).await,
        Run::seeded(&emulator, "cross-domain-b", 2).await,
    ];
    assert_ne!(
        (runs[0].decision_scheduled, runs[0].activity_scheduled),
        (runs[1].decision_scheduled, runs[1].activity_scheduled),
        "case \"two domains\": the runs must schedule their tasks at different event ids \
         for this rule to be visible, got {:?} and {:?}",
        (runs[0].decision_scheduled, runs[0].activity_scheduled),
        (runs[1].decision_scheduled, runs[1].activity_scheduled)
    );

    let mut before = Vec::new();
    for run in &runs {
        before.push(whole_history(&emulator, run.domain, WORKFLOW_ID, &run.run_id).await);
    }

    for run in &runs {
        // A DECISION handle offered to each activity-side call names the decision task of
        // ITS run, never the other run's and never the member's default.
        for operation in ACTIVITY_SIDE {
            refused_with(
                &emulator,
                &format!("{operation} on the decision handle of {}", run.label()),
                operation,
                &json!({ "taskToken": run.decision_token }),
                &unknown_activity(run.decision_scheduled),
            )
            .await;
        }
        // The mirror: an ACTIVITY handle offered to the decider's completion names the
        // activity task of its own run.
        refused_with(
            &emulator,
            &format!(
                "RespondDecisionTaskCompleted on the activity handle of {}",
                run.label()
            ),
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": run.activity_token, "decisions": [] }),
            &unknown_decision_task(run.activity_scheduled),
        )
        .await;
    }

    // No refusal recorded anything, and each run's own activity handle still closes that
    // run's own task: the handles were neither consumed nor confused with each other.
    for (run, history) in runs.iter().zip(&before) {
        let after = whole_history(&emulator, run.domain, WORKFLOW_ID, &run.run_id).await;
        assert_eq!(
            &after, history,
            "case {:?}: a refused call records nothing, got {after:?}",
            run.domain
        );
        let completed = emulator
            .ok(
                run.domain,
                "RespondActivityTaskCompleted",
                &json!({ "taskToken": run.activity_token, "result": "done" }),
            )
            .await;
        assert_eq!(
            completed,
            json!({}),
            "case {:?}: the run's own worker report is honoured, got {completed:?}",
            run.domain
        );
        let closed = whole_history(&emulator, run.domain, WORKFLOW_ID, &run.run_id).await;
        let completions: Vec<&Value> = closed
            .iter()
            .filter(|event| {
                event.pointer("/eventType").and_then(Value::as_str) == Some("ActivityTaskCompleted")
            })
            .collect();
        assert_eq!(
            completions.len(),
            1,
            "case {:?}: exactly the run's own activity task closed, got {closed:?}",
            run.domain
        );
        assert_eq!(
            completions[0]
                .pointer("/activityTaskCompletedEventAttributes/scheduledEventId")
                .and_then(Value::as_i64),
            Some(run.activity_scheduled),
            "case {:?}: the close event names the task this run's handle was on, got {:?}",
            run.domain,
            completions[0]
        );
    }
}

/// Call `operation` with `body` and require 400 `UnknownResourceFault` carrying
/// `expected_message`.
async fn refused_with(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
    expected_message: &str,
) {
    let (status, refused) = emulator.call(operation, body).await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            refused.get("message").and_then(Value::as_str),
        ),
        (400, "UnknownResourceFault", Some(expected_message)),
        "case {case:?}: expected 400 UnknownResourceFault naming {expected_message:?}, \
         got {status} with {refused:?}"
    );
}
