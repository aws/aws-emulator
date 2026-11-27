// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the run-a-job (`.sync`) integration pattern inside an EXPRESS
//! workflow, driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: the run-a-job pattern is served for the STANDARD workflow type
//! alone, and WHERE an EXPRESS definition asking for it is refused follows the INTEGRATION
//! the `Resource` names. RECORDED behaviour: for this service's OWN nested execution the
//! document is no defect — the EXPRESS state machine is created exactly as a STANDARD one is
//! — and it is the EXECUTION that fails AT that state, with the service's own refusal of the
//! pattern naming the state it failed at and having started no job; for another Amazon Web
//! Services service's integration the CREATE itself is refused, carrying that same sentence.
//! The same definition run as a STANDARD workflow starts its job and parks at it, which is
//! the other side of the same rule.
//!
//! The rows differ along what the rule generalises over: the revision of the pattern the
//! `Resource` asks for (`.sync` and `.sync:2`), the INTEGRATION it asks it of (a nested
//! execution, whose calls this service serves itself, and the integrations of services whose
//! own API is not even mounted here), and the name of the offending state.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` builds, the EXPRESS create is `sync_execution`'s and the
//! history reader is `execution_history`'s; `tests/all/main.rs` picks this file up, so
//! each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    EXPRESS_RUN_A_JOB_UNSUPPORTED, RUNTIME_ERROR,
};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{create_machine, string, Emulator, ROLE_ARN};
use crate::sync_execution::create_express;

/// One row of the table: the case, the state name, and the `Resource` that state's
/// run-a-job `Task` names.
type PatternRow = (&'static str, &'static str, &'static str);

/// A definition whose one `Task` state, named `state`, asks for the run-a-job pattern
/// through `resource`.
fn parent_definition(state: &str, resource: &str, child_arn: &str) -> String {
    json!({
        "StartAt": state,
        "States": {
            state: {
                "Type": "Task",
                "Resource": resource,
                "Parameters": { "StateMachineArn": child_arn, "Input": {} },
                "End": true,
            }
        }
    })
    .to_string()
}

/// A child definition that finishes at once, for the STANDARD control to really run.
const CHILD: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// How many executions `ListExecutions` reports for `machine_arn`.
async fn started_count(emulator: &Emulator, machine_arn: &str) -> usize {
    let listed = emulator
        .ok(
            "ListExecutions",
            &json!({ "stateMachineArn": machine_arn, "maxResults": 10 }),
        )
        .await;
    listed
        .get("executions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// An EXPRESS state machine asking for the run-a-job pattern of this service's OWN nested
/// execution is CREATED, and each of its executions fails at the state that asked, starting
/// nothing.
#[tokio::test]
async fn an_express_execution_fails_at_a_nested_run_a_job_task_the_create_accepted() {
    let rows: [PatternRow; 3] = [
        (
            "a nested execution through the pattern's first revision",
            "CallChild",
            "arn:aws:states:::states:startExecution.sync",
        ),
        (
            "the same integration through its second revision",
            "CallChild",
            "arn:aws:states:::states:startExecution.sync:2",
        ),
        (
            "a state of another name, which the refusal must locate instead",
            "RunTheChildWorkflow",
            "arn:aws:states:::states:startExecution.sync",
        ),
    ];

    let emulator = Emulator::new();
    let child_arn = create_machine(&emulator, "express-sync-child", CHILD).await;
    for (index, (case, state, resource)) in rows.into_iter().enumerate() {
        let definition = parent_definition(state, resource, &child_arn);
        let machine = create_express(&emulator, &format!("express-sync-{index}"), &definition).await;
        let answered = emulator
            .ok(
                "StartSyncExecution",
                &json!({ "stateMachineArn": machine, "name": "run" }),
            )
            .await;
        assert_eq!(
            (
                answered.get("status").and_then(Value::as_str),
                answered.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: expected the run to fail with {RUNTIME_ERROR}, got {answered:?}"
        );
        let cause = string(&answered, "cause");
        assert!(
            cause.contains(EXPRESS_RUN_A_JOB_UNSUPPORTED),
            "case {case:?}: expected the cause to state {EXPRESS_RUN_A_JOB_UNSUPPORTED:?}, got \
             {cause:?}"
        );
        assert!(
            cause.contains(&format!("'{state}'")),
            "case {case:?}: expected the cause to name the state that asked, got {cause:?}"
        );
    }

    // Nothing was started for the child: the refusal is decided before the integration's
    // call would be made, so no execution of the child exists for any of the rows.
    assert_eq!(
        started_count(&emulator, &child_arn).await,
        0,
        "case \"the child a refused task would have started\": expected no execution of it"
    );
}

/// The other half of the same rule: an EXPRESS create naming the run-a-job pattern of
/// ANOTHER Amazon Web Services service's integration is refused where the document is
/// submitted, with the service's one sentence for the unsupported pattern located at the
/// member that asked for it, and no state machine is left behind. The rows differ along the
/// integration the pattern is asked of, the revision of the pattern and the name of the
/// offending state — none of which bears on the refusal.
#[tokio::test]
async fn an_express_create_is_refused_a_run_a_job_task_of_another_service() {
    let rows: [PatternRow; 4] = [
        (
            "a container task through the pattern's first revision",
            "RunTask",
            "arn:aws:states:::ecs:runTask.sync",
        ),
        (
            "the same integration through its second revision",
            "RunTask",
            "arn:aws:states:::ecs:runTask.sync:2",
        ),
        (
            "a query of a service not mounted beside this one",
            "RunQuery",
            "arn:aws:states:::athena:startQueryExecution.sync",
        ),
        (
            "a batch job, under a state of another name",
            "SubmitTheJob",
            "arn:aws:states:::batch:submitJob.sync",
        ),
    ];

    let emulator = Emulator::new();
    let child_arn = create_machine(&emulator, "refused-sync-child", CHILD).await;
    for (index, (case, state, resource)) in rows.into_iter().enumerate() {
        let definition = parent_definition(state, resource, &child_arn);
        let name = format!("express-refused-sync-{index}");
        let (status, body) = emulator
            .call(
                "CreateStateMachine",
                &json!({
                    "name": &name,
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                    "type": "EXPRESS",
                }),
            )
            .await;
        assert_eq!(
            (status, body.get("__type").and_then(Value::as_str)),
            (400, Some("InvalidDefinition")),
            "case {case:?}: expected the create to be refused, got {status} with {body:?}"
        );
        let message = string(&body, "message");
        assert!(
            message.contains(EXPRESS_RUN_A_JOB_UNSUPPORTED)
                && message.contains(&format!("/States/{state}/Resource")),
            "case {case:?}: expected the refusal to state \
             {EXPRESS_RUN_A_JOB_UNSUPPORTED:?} at the member that asked for the pattern, got \
             {message:?}"
        );

        // Nothing was created: the listing names no machine of that name.
        let listed = emulator
            .ok("ListStateMachines", &json!({ "maxResults": 100 }))
            .await;
        let names: Vec<String> = listed
            .get("stateMachines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|machine| machine.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect();
        assert!(
            !names.contains(&name),
            "case {case:?}: a refused create must leave no state machine, got {names:?}"
        );

        // The SAME definition is created as a STANDARD workflow, so the refusal is the
        // workflow type's and not the definition's.
        let standard = create_machine(
            &emulator,
            &format!("standard-sync-{index}"),
            &definition,
        )
        .await;
        assert!(
            standard.contains(&format!("standard-sync-{index}")),
            "case {case:?}: expected the same definition to be created as a STANDARD \
             workflow, got {standard:?}"
        );
    }
}

/// The SAME definition run as a STANDARD workflow starts its job and parks at it: the
/// refusal above is the workflow type's and not the definition's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standard_execution_of_the_same_definition_starts_its_job() {
    let emulator = Emulator::new();
    let child_arn = create_machine(&emulator, "standard-sync-child", CHILD).await;
    let definition = parent_definition(
        "CallChild",
        "arn:aws:states:::states:startExecution.sync",
        &child_arn,
    );
    let parent = create_machine(&emulator, "standard-sync-parent", &definition).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": parent, "name": "parked" }),
        )
        .await;
    let execution_arn = string(&started, "executionArn");
    let case = "a STANDARD execution of the same definition";

    let described = emulator
        .ok(
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "case {case:?}: expected the parent to be parked at its job, got {described:?}"
    );
    assert_eq!(
        started_count(&emulator, &child_arn).await,
        1,
        "case {case:?}: expected the job to have been started as a child execution"
    );
    let submitted = history(&emulator, &execution_arn, json!({}))
        .await
        .into_iter()
        .any(|event| event.get("type").and_then(Value::as_str) == Some("TaskSubmitted"));
    assert!(
        submitted,
        "case {case:?}: expected the submission of the job in the parent's history"
    );
}
