// SPDX-License-Identifier: CC0-1.0

//! A GLUE WORKFLOW as the target of a rule: what a delivery to it does, end to end over
//! the wire through the real `MultiServiceRouter` with Glue mounted beside EventBridge — so
//! the workflow run a delivery starts is read back where a client reads it, through
//! `glue:GetWorkflowRuns`.
//!
//! The RULE asserted here: a rule target whose `Arn` is a Glue workflow is delivered to by
//! handing the matched event to that workflow (the `glue:notifyEvent` action the target's
//! `RoleArn` has to carry), where the workflow's EVENT trigger counts it against its
//! `EventBatchingCondition` and fires EXACTLY ONCE per completed batch — "batch condition
//! that must be met (specified number of events received or batch time window expired)
//! before EventBridge event trigger fires", whose `BatchSize` is "number of events that
//! must be received from Amazon EventBridge before EventBridge event trigger fires"
//! (`com.amazonaws.glue#EventBatchingCondition`, `@range 1..100`). So a workflow whose batch
//! has not filled reports NO run, the delivery that fills it leaves EXACTLY ONE, and that
//! run carries the workflow's `Name` and a `WorkflowRunId`. A workflow target is one AWS
//! invokes UNDER the target's role, so a role whose policies withhold `glue:notifyEvent`,
//! and one `events.amazonaws.com` cannot assume, deliver nothing.
//!
//! The rows differ along what the rule generalises over: the `BatchSize` the workflow's
//! trigger carries (each published bound, one in between, and no condition at all), how
//! many events are published and how they are SPLIT across `PutEvents` requests, how many
//! rules and how many workflows one delivery reaches, and why a role bars the delivery.
//! What fires a batch is the number of HANDOVERS the matched rules made, so a batch filled
//! by the entries of one request fires exactly as one filled an event at a time, and the
//! batch reopens after every firing. The `BatchWindow` half holds over the same ingress:
//! "window of time in seconds after which EventBridge event trigger fires. Window starts
//! when first event is received" (`@range 1..900`), so a batch the published event opened
//! and no further event filled fires once its window has run out.
//!
//! The batch ACCOUNTING on its own is asserted over its own table in the Glue crate's
//! `event_trigger_batch_firing` suite; this one is about the INGRESS: that every event the
//! rules matched reaches that accounting.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{InvocationOutcome, SERVICE_PRINCIPAL};

use crate::target_dispatch::{
    emulator, given_role, given_rule, invocations, publish_details, rpc_as, Emulator, AMBIENT_KEY,
    REGION,
};

/// The job every trigger below initiates, so a firing has an action to begin.
const JOB: &str = "workflow-target-job";

/// One Glue call over the wire (`AWSGlue`, awsJson1_1), answered with its status and body.
async fn glue(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc_as(
        emulator,
        AMBIENT_KEY,
        "glue",
        "AWSGlue",
        "1.1",
        operation,
        body,
    )
    .await
}

/// One Glue call that must succeed.
async fn glue_ok(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = glue(emulator, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: glue {operation} answered {status} {answer}"
    );
    answer
}

/// Register the job a trigger's action names.
async fn given_job(emulator: &Emulator, case: &str) {
    glue_ok(
        emulator,
        case,
        "CreateJob",
        &json!({
            "Name": JOB,
            "Role": format!(
                "arn:aws:iam::{}:role/glue-job-role",
                engine::emulator_account_id()
            ),
            "Command": { "Name": "pythonshell", "ScriptLocation": "s3://scripts/noop.py" },
        }),
    )
    .await;
}

/// A workflow called `name` with an EVENT trigger on it carrying `condition` (absent when
/// the row names none); answers the ARN a rule's target names it by.
async fn given_workflow(
    emulator: &Emulator,
    case: &str,
    name: &str,
    condition: Option<Value>,
) -> String {
    glue_ok(emulator, case, "CreateWorkflow", &json!({ "Name": name })).await;
    let mut request = json!({
        "Name": format!("{name}-evt"),
        "Type": "EVENT",
        "WorkflowName": name,
        "Actions": [{ "JobName": JOB }],
    });
    if let Some(condition) = condition {
        request["EventBatchingCondition"] = condition;
    }
    glue_ok(emulator, case, "CreateTrigger", &request).await;
    workflow_arn(name)
}

/// The ARN a rule's target names the workflow `name` by, in the Region every call here is
/// signed for.
fn workflow_arn(name: &str) -> String {
    engine::emulator_arn("glue", REGION, &format!("workflow/{name}"))
}

/// The runs `GetWorkflowRuns` reports for `workflow`, as a client reads them: a workflow no
/// run has been started of is refused with `EntityNotFoundException`, which is the same
/// ZERO.
async fn runs_of(emulator: &Emulator, case: &str, workflow: &str) -> Vec<Value> {
    let (status, body) = glue(emulator, "GetWorkflowRuns", &json!({ "Name": workflow })).await;
    if status == 400 {
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some("EntityNotFoundException"),
            "case {case:?}: a run history with nothing in it is the modelled miss, got {body}"
        );
        return Vec::new();
    }
    assert_eq!(
        status, 200,
        "case {case:?}: GetWorkflowRuns answered {status} {body}"
    );
    body["Runs"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetWorkflowRuns reports a Runs list, got {body}"))
}

/// Every reported run names the workflow it belongs to and carries a run id.
fn assert_runs_are_well_formed(case: &str, workflow: &str, runs: &[Value]) {
    for run in runs {
        assert_eq!(
            run.get("Name").and_then(Value::as_str),
            Some(workflow),
            "case {case:?}: a run reports the workflow it belongs to, got {run}"
        );
        assert!(
            run.get("WorkflowRunId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: a run carries a non-empty WorkflowRunId, got {run}"
        );
    }
}

/// Publish ONE matching event.
async fn publish_one(emulator: &Emulator, detail: &str) {
    publish_details(emulator, &[detail.to_string()]).await;
}

/// Publish `entries` matching events in ONE `PutEvents` request — the request number is
/// only what keeps each entry's detail distinct.
async fn publish_entries(emulator: &Emulator, request: usize, entries: usize) {
    let details: Vec<String> = (0..entries)
        .map(|entry| format!("{{\"orderId\":\"A-{request}-{entry}\"}}"))
        .collect();
    publish_details(emulator, &details).await;
}

/// A published event reaches the workflow's EVENT trigger, which starts one run per
/// completed batch — over a TABLE whose rows differ along the batch size the trigger
/// carries and how many events are published.
#[tokio::test]
async fn a_glue_workflow_target_starts_one_run_per_completed_batch() {
    // (case, the trigger's EventBatchingCondition, the runs expected after each of three
    //  published events)
    let rows: [(&str, Option<Value>, [usize; 3]); 4] = [
        (
            // The lower published bound of `BatchSize`: the unbatched arm, which starts a
            // run for every event it is handed.
            "batch size at the lower bound",
            Some(json!({ "BatchSize": 1 })),
            [1, 2, 3],
        ),
        (
            // A trigger with no condition batches nothing, so it fires per event as the
            // lower bound does.
            "no batching condition",
            None,
            [1, 2, 3],
        ),
        (
            // The recorded shape of the defect: nothing after deliveries 1 and 2, exactly
            // one run after delivery 3, well inside the window.
            "a batch of three inside its window",
            Some(json!({ "BatchSize": 3, "BatchWindow": 900 })),
            [0, 0, 1],
        ),
        (
            // The upper published bound: three events are nowhere near the batch, so the
            // workflow still has no run.
            "batch size at the upper bound",
            Some(json!({ "BatchSize": 100 })),
            [0, 0, 0],
        ),
    ];

    for (case, condition, expected) in rows {
        let emulator = emulator();
        given_job(&emulator, case).await;
        let workflow = "driven-by-events";
        let arn = given_workflow(&emulator, case, workflow, condition).await;
        let role = given_role(
            &emulator,
            "events-notify-glue",
            SERVICE_PRINCIPAL,
            Some("glue:notifyEvent"),
        )
        .await;
        given_rule(
            &emulator,
            "to-the-workflow",
            json!([{ "Id": "batched", "Arn": arn, "RoleArn": role }]),
        )
        .await;

        for (delivery, expected_runs) in expected.iter().enumerate() {
            publish_one(&emulator, &format!("{{\"orderId\":\"A-{delivery}\"}}")).await;
            let runs = runs_of(&emulator, case, workflow).await;
            assert_eq!(
                runs.len(),
                *expected_runs,
                "case {case:?}: after event {} expected {expected_runs} workflow run(s), got \
                 {} ({:?})",
                delivery + 1,
                runs.len(),
                invocations(&emulator)
            );
            assert_runs_are_well_formed(case, workflow, &runs);
        }

        // Every handover was one the workflow accepted: the delivery is what the trigger's
        // batch counts, so a batch that has not filled is still a delivery that was made.
        let recorded = invocations(&emulator);
        assert_eq!(
            recorded.len(),
            expected.len(),
            "case {case:?}: expected one handover per published event, got {recorded:?}"
        );
        for (_, _, outcome) in &recorded {
            assert_eq!(
                *outcome,
                InvocationOutcome::Invoked,
                "case {case:?}: the workflow accepted the event, got {recorded:?}"
            );
        }
    }
}

/// Two workflows on the SAME rule, role and deliveries: the batched arm (`BatchSize` 3)
/// reports no run until its third event, while its unbatched twin (`BatchSize` 1) already
/// reports one from the first — so a client can tell a suppressed batch from a dropped
/// delivery.
#[tokio::test]
async fn a_batched_workflow_and_its_unbatched_twin_see_the_same_deliveries() {
    let case = "a batched workflow beside its unbatched twin";
    let emulator = emulator();
    given_job(&emulator, case).await;
    let batched = given_workflow(
        &emulator,
        case,
        "batched-arm",
        Some(json!({ "BatchSize": 3, "BatchWindow": 900 })),
    )
    .await;
    let twin = given_workflow(&emulator, case, "twin-arm", Some(json!({ "BatchSize": 1 }))).await;
    let role = given_role(
        &emulator,
        "events-notify-glue",
        SERVICE_PRINCIPAL,
        Some("glue:notifyEvent"),
    )
    .await;
    given_rule(
        &emulator,
        "to-both-workflows",
        json!([
            { "Id": "batched", "Arn": batched, "RoleArn": role },
            { "Id": "twin", "Arn": twin, "RoleArn": role },
        ]),
    )
    .await;

    // (how many events have been published, the batched arm's runs, the twin's runs)
    let expected: [(usize, usize, usize); 3] = [(1, 0, 1), (2, 0, 2), (3, 1, 3)];
    for (published, batched_runs, twin_runs) in expected {
        publish_one(&emulator, &format!("{{\"orderId\":\"A-{published}\"}}")).await;
        let seen_batched = runs_of(&emulator, case, "batched-arm").await;
        let seen_twin = runs_of(&emulator, case, "twin-arm").await;
        assert_eq!(
            seen_batched.len(),
            batched_runs,
            "case {case:?}: after {published} event(s) the batched arm expected \
             {batched_runs} run(s), got {}",
            seen_batched.len()
        );
        assert_eq!(
            seen_twin.len(),
            twin_runs,
            "case {case:?}: after {published} event(s) the twin expected {twin_runs} run(s), \
             got {}",
            seen_twin.len()
        );
        assert_runs_are_well_formed(case, "batched-arm", &seen_batched);
        assert_runs_are_well_formed(case, "twin-arm", &seen_twin);
    }
}

/// What the role and the ARN decide: a workflow target is invoked UNDER the target's role,
/// so a role whose policies withhold `glue:notifyEvent` and one `events.amazonaws.com`
/// cannot assume deliver nothing, and a target naming a workflow this account does not hold
/// is a failed invocation. The grant row is the control: the same rule, the same event, and
/// a run appears.
#[tokio::test]
async fn a_workflow_target_is_delivered_to_under_the_targets_role() {
    // (case, the role's trusted service principal, the action its policy allows, whether
    //  the target names a workflow that exists, whether a run is expected)
    let rows: [(&str, &str, Option<&str>, bool, bool); 5] = [
        (
            "a role trusting EventBridge that allows the ingress",
            SERVICE_PRINCIPAL,
            Some("glue:notifyEvent"),
            true,
            true,
        ),
        (
            "a role whose policy names another action",
            SERVICE_PRINCIPAL,
            Some("glue:StartWorkflowRun"),
            true,
            false,
        ),
        (
            "a role whose policies state nothing",
            SERVICE_PRINCIPAL,
            None,
            true,
            false,
        ),
        (
            "a role EventBridge cannot assume",
            "glue.amazonaws.com",
            Some("glue:notifyEvent"),
            true,
            false,
        ),
        (
            "a target naming a workflow this account does not hold",
            SERVICE_PRINCIPAL,
            Some("glue:notifyEvent"),
            false,
            false,
        ),
    ];

    for (case, principal, allowed, workflow_exists, expect_a_run) in rows {
        let emulator = emulator();
        given_job(&emulator, case).await;
        let workflow = "role-governed";
        let arn = if workflow_exists {
            given_workflow(&emulator, case, workflow, Some(json!({ "BatchSize": 1 }))).await
        } else {
            workflow_arn("no-such-workflow")
        };
        let role = given_role(&emulator, "events-to-glue", principal, allowed).await;
        given_rule(
            &emulator,
            "to-the-workflow",
            json!([{ "Id": "batched", "Arn": arn, "RoleArn": role }]),
        )
        .await;

        publish_one(&emulator, r#"{"orderId":"A-1"}"#).await;

        let recorded = invocations(&emulator);
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: expected one handover, got {recorded:?}"
        );
        let delivered = matches!(recorded[0].2, InvocationOutcome::Invoked);
        assert_eq!(
            delivered, expect_a_run,
            "case {case:?}: expected the handover to be accepted: {expect_a_run}, got \
             {recorded:?}"
        );

        if workflow_exists {
            let runs = runs_of(&emulator, case, workflow).await;
            let expected = usize::from(expect_a_run);
            assert_eq!(
                runs.len(),
                expected,
                "case {case:?}: expected {expected} workflow run(s), got {} ({recorded:?})",
                runs.len()
            );
            assert_runs_are_well_formed(case, workflow, &runs);
        }
    }
}

/// One row of the table below: the case, the trigger's `EventBatchingCondition`, the entry
/// count of each `PutEvents` request, and the runs expected after each of those requests.
type BatchRow = (
    &'static str,
    Option<Value>,
    &'static [usize],
    &'static [usize],
);

/// What completes a batch is the number of HANDOVERS the matched rules made, not the number
/// of `PutEvents` requests they came in: a batch filled by the entries of ONE request fires
/// exactly once, a request carrying more entries than the batch holds fires once per
/// completed batch and leaves the rest in the open one, and the batch REOPENS after every
/// firing — over a table whose rows differ along the batch size and along how the events are
/// split across requests.
#[tokio::test]
async fn every_matched_handover_counts_against_the_batch_however_the_events_arrive() {
    // (case, the trigger's EventBatchingCondition, the entry count of each PutEvents
    //  request, the runs expected after each of those requests)
    let rows: [BatchRow; 5] = [
        (
            // One request whose entries fill the batch exactly: the batch is complete
            // inside that request, so it fires once.
            "one request whose entries fill the batch",
            Some(json!({ "BatchSize": 3, "BatchWindow": 900 })),
            &[3],
            &[1],
        ),
        (
            // The same batch filled by requests of DIFFERING size: what is counted is the
            // handovers, so the split does not change where the batch completes.
            "requests of differing size filling one batch",
            Some(json!({ "BatchSize": 3, "BatchWindow": 900 })),
            &[1, 2],
            &[0, 1],
        ),
        (
            // More entries at once than the batch holds: two batches complete inside the
            // request and the fifth event stays in the open one.
            "one request carrying more entries than the batch holds",
            Some(json!({ "BatchSize": 2 })),
            &[5],
            &[2],
        ),
        (
            // The batch reopens after each firing, so the count starts again: the fifth
            // and sixth events complete a third batch.
            "the batch reopening after each firing",
            Some(json!({ "BatchSize": 2 })),
            &[2, 2, 1, 1],
            &[1, 2, 2, 3],
        ),
        (
            // The lower published bound of `BatchSize`, several entries at once: every
            // entry completes its own batch.
            "the lower bound of the batch size, several entries at once",
            Some(json!({ "BatchSize": 1 })),
            &[3],
            &[3],
        ),
    ];

    for (case, condition, requests, expected) in rows {
        assert_eq!(
            requests.len(),
            expected.len(),
            "case {case:?}: the row states one expectation per request, got {} and {}",
            requests.len(),
            expected.len()
        );
        let emulator = emulator();
        given_job(&emulator, case).await;
        let workflow = "counted-per-handover";
        let arn = given_workflow(&emulator, case, workflow, condition).await;
        let role = given_role(
            &emulator,
            "events-notify-glue",
            SERVICE_PRINCIPAL,
            Some("glue:notifyEvent"),
        )
        .await;
        given_rule(
            &emulator,
            "to-the-workflow",
            json!([{ "Id": "batched", "Arn": arn, "RoleArn": role }]),
        )
        .await;

        for (request, (entries, expected_runs)) in requests.iter().zip(expected).enumerate() {
            publish_entries(&emulator, request, *entries).await;
            let runs = runs_of(&emulator, case, workflow).await;
            assert_eq!(
                runs.len(),
                *expected_runs,
                "case {case:?}: after request {} of {entries} entr(ies) expected \
                 {expected_runs} workflow run(s), got {} ({:?})",
                request + 1,
                runs.len(),
                invocations(&emulator)
            );
            assert_runs_are_well_formed(case, workflow, &runs);
        }

        // One handover per published entry, every one of them accepted: the batch counts
        // deliveries, so a request that fired nothing still delivered.
        let recorded = invocations(&emulator);
        let published: usize = requests.iter().sum();
        assert_eq!(
            recorded.len(),
            published,
            "case {case:?}: expected one handover per published entry ({published}), got \
             {recorded:?}"
        );
        for (_, _, outcome) in &recorded {
            assert_eq!(
                *outcome,
                InvocationOutcome::Invoked,
                "case {case:?}: the workflow accepted the event, got {recorded:?}"
            );
        }
    }
}

/// A workflow that is the target of SEVERAL rules is handed one event per matched rule, and
/// every one of those handovers counts against the same batch: two rules matching one
/// published event complete a batch of two.
#[tokio::test]
async fn a_workflow_targeted_by_several_rules_counts_every_rules_delivery() {
    let case = "two rules targeting one workflow";
    let emulator = emulator();
    given_job(&emulator, case).await;
    let workflow = "targeted-by-two-rules";
    let arn = given_workflow(
        &emulator,
        case,
        workflow,
        Some(json!({ "BatchSize": 2, "BatchWindow": 900 })),
    )
    .await;
    let role = given_role(
        &emulator,
        "events-notify-glue",
        SERVICE_PRINCIPAL,
        Some("glue:notifyEvent"),
    )
    .await;
    for rule in ["first-to-the-workflow", "second-to-the-workflow"] {
        given_rule(
            &emulator,
            rule,
            json!([{ "Id": "batched", "Arn": arn, "RoleArn": role }]),
        )
        .await;
    }

    publish_one(&emulator, r#"{"orderId":"A-1"}"#).await;

    // Both rules matched, so both handed the event over.
    let recorded = invocations(&emulator);
    let mut rules: Vec<String> = recorded.iter().map(|(rule, _, _)| rule.clone()).collect();
    rules.sort();
    assert_eq!(
        rules,
        vec![
            "first-to-the-workflow".to_string(),
            "second-to-the-workflow".to_string()
        ],
        "case {case:?}: expected one handover per matched rule, got {recorded:?}"
    );

    // The two handovers completed the batch of two, which fired exactly once.
    let runs = runs_of(&emulator, case, workflow).await;
    assert_eq!(
        runs.len(),
        1,
        "case {case:?}: two handovers complete a batch of two, got {} run(s) ({recorded:?})",
        runs.len()
    );
    assert_runs_are_well_formed(case, workflow, &runs);
}

/// How long a batch whose window has run out is given to be reported as fired before the
/// run that never appeared is a failed assertion, and how long a read waits before looking
/// again. The budget is far longer than the one-second window the trigger below carries, so
/// what the assertion depends on is the firing rather than on how fast the host runs.
const WINDOW_BUDGET: Duration = Duration::from_secs(20);
const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `BatchWindow` half of the condition over the same ingress: an event the rule
/// delivered OPENS the batch, and the batch fires with the events it holds once the window
/// has run out — "window of time in seconds after which EventBridge event trigger fires.
/// Window starts when first event is received" (`@range 1..900`).
///
/// The trigger's `BatchSize` is the UPPER published bound and one event is published, so
/// the count cannot be what fires this batch; the window is the LOWER published bound, one
/// second, which is what keeps the wait short. The runs are read again until the firing is
/// there, because it is owed by the clock rather than by a call.
#[tokio::test]
async fn a_batch_whose_window_runs_out_fires_from_a_published_event() {
    let case = "a one-second window the published event opens";
    let emulator = emulator();
    given_job(&emulator, case).await;
    let workflow = "closed-by-its-window";
    let arn = given_workflow(
        &emulator,
        case,
        workflow,
        Some(json!({ "BatchSize": 100, "BatchWindow": 1 })),
    )
    .await;
    let role = given_role(
        &emulator,
        "events-notify-glue",
        SERVICE_PRINCIPAL,
        Some("glue:notifyEvent"),
    )
    .await;
    given_rule(
        &emulator,
        "to-the-workflow",
        json!([{ "Id": "windowed", "Arn": arn, "RoleArn": role }]),
    )
    .await;

    publish_one(&emulator, r#"{"orderId":"A-1"}"#).await;
    let recorded = invocations(&emulator);
    assert_eq!(
        recorded.len(),
        1,
        "case {case:?}: expected one handover, got {recorded:?}"
    );

    let deadline = Instant::now() + WINDOW_BUDGET;
    let mut runs = runs_of(&emulator, case, workflow).await;
    while runs.is_empty() && Instant::now() < deadline {
        tokio::time::sleep(WINDOW_POLL_INTERVAL).await;
        runs = runs_of(&emulator, case, workflow).await;
    }
    assert_eq!(
        runs.len(),
        1,
        "case {case:?}: a window that ran out fires once with the one event it holds, got \
         {} run(s) ({recorded:?})",
        runs.len()
    );
    assert_runs_are_well_formed(case, workflow, &runs);
}
