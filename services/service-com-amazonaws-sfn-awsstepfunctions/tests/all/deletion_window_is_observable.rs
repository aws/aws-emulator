// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SPAN a started `DeleteStateMachine` is observable over, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! RULE: `DeleteStateMachine` "is an asynchronous operation. It sets the state machine's
//! status to DELETING and begins the deletion process", and for the EXPRESS workflow type
//! "the deletion happens eventually (usually in less than a minute)" — so the status that
//! delete sets, and the refusal every call that would start work on the machine answers
//! while it lasts, outlast a client's own poll interval: a client polling for a stable
//! answer reads the same in-progress refusal several times over before the record goes, and
//! reads the does-not-exist refusal from then on. A window shorter than one poll interval
//! would leave a client unable to tell "still deleting" from "gone", so it could not wait
//! for the right one.
//!
//! The rows differ along what the rule generalises over: WHICH call asks (the synchronous
//! start, the asynchronous start, and the read of the record itself, which reports the
//! status the two refusals are about), with a STANDARD machine as the control — the type the
//! service publishes no span for, whose deletion is held only to this emulator's floor and
//! whose own refusals are the same two.
//!
//! Nothing here is asserted against the clock: each loop reads until the ANSWER CHANGES,
//! pausing a fraction of the span the owning service publishes, and what is asserted is HOW
//! MANY reads the in-progress answer survived.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one `state_machine_lifecycle`
//! builds; `tests/all/main.rs` picks every suite of this target up, so it is reached through
//! `crate::state_machine_lifecycle`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EXPRESS_DELETE_SETTLE_DELAY;

use crate::state_machine_lifecycle::{string, Emulator, HELLO_DEFINITION, ROLE_ARN};

/// How long a loop waits for the deletion to go through before giving up: generous next to
/// the span the deletion takes, because what the loop waits for is the answer changing.
const DELETION_DEADLINE: Duration = Duration::from_secs(120);

/// How long one read pauses before looking again: a fraction of the span the owning service
/// publishes for an EXPRESS deletion, which is the interval a client polling for a stable
/// answer would use.
const CLIENT_POLL_PAUSE: Duration = match EXPRESS_DELETE_SETTLE_DELAY.checked_div(5) {
    Some(pause) => pause,
    None => Duration::from_secs(1),
};

/// How many consecutive reads of the same answer a client needs to call it stable, which is
/// what the published span has to outlast.
const STABLE_READS: usize = 3;

/// The refusal a call that would start work on a machine whose deletion is IN PROGRESS
/// answers, and the one it answers once the record has gone.
const DELETING: &str = "StateMachineDeleting";
const DOES_NOT_EXIST: &str = "StateMachineDoesNotExist";

/// The status `DescribeStateMachine` reports a machine whose deletion has begun in
/// (`StateMachineStatus`: ACTIVE | DELETING).
const STATUS_DELETING: &str = "DELETING";

/// Create a state machine of `machine_type` and answer its ARN.
async fn create_of_type(emulator: &Emulator, name: &str, machine_type: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": machine_type,
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// What one read of a row's call says about the deletion: the error code of a refusal, or
/// the `status` a read of the record itself reported.
async fn answer_of(emulator: &Emulator, operation: &str, machine: &str) -> String {
    let request = match operation {
        "DescribeStateMachine" => json!({ "stateMachineArn": machine }),
        _ => json!({ "stateMachineArn": machine, "input": "{}" }),
    };
    let (status, body) = emulator.call(operation, &request).await;
    if status == 200 {
        return body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    body.get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// One row: the call that asks about the deletion, the workflow type of the machine it is
/// addressed to, what the answer is while the deletion is in progress, and whether the
/// published span has to outlast a client's poll interval.
struct Row {
    case: &'static str,
    operation: &'static str,
    machine_type: &'static str,
    in_progress: &'static str,
    /// Whether the service publishes a span for this machine's deletion, which is what the
    /// stable-answer reads below are asserted of.
    published_span: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the synchronous start of an EXPRESS workflow",
            operation: "StartSyncExecution",
            machine_type: "EXPRESS",
            in_progress: DELETING,
            published_span: true,
        },
        Row {
            case: "the asynchronous start of an EXPRESS workflow",
            operation: "StartExecution",
            machine_type: "EXPRESS",
            in_progress: DELETING,
            published_span: true,
        },
        Row {
            case: "the read of the EXPRESS record the refusals are about",
            operation: "DescribeStateMachine",
            machine_type: "EXPRESS",
            in_progress: STATUS_DELETING,
            published_span: true,
        },
        Row {
            case: "the asynchronous start of a STANDARD workflow, whose span is unpublished",
            operation: "StartExecution",
            machine_type: "STANDARD",
            in_progress: DELETING,
            published_span: false,
        },
    ]
}

/// A client polling a deleted state machine reads the in-progress answer over and over
/// before the record goes, and the does-not-exist refusal from then on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answer_a_started_deletion_gives_outlasts_a_clients_poll_interval() {
    let emulator = Emulator::new();
    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            operation,
            machine_type,
            in_progress,
            published_span,
        } = row;
        let name = format!("deletion-window-{index}");
        let machine = create_of_type(&emulator, &name, machine_type).await;
        emulator
            .ok("DeleteStateMachine", &json!({ "stateMachineArn": &machine }))
            .await;

        // Read until the ANSWER CHANGES, pausing as a client polling for a stable answer
        // would: every answer before the change is the deletion in progress.
        let deadline = Instant::now() + DELETION_DEADLINE;
        let mut in_progress_reads = 0usize;
        let once_gone = loop {
            let answered = answer_of(&emulator, operation, &machine).await;
            if answered != in_progress {
                break answered;
            }
            in_progress_reads += 1;
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the deletion to go through, still answering \
                 {answered:?} after {in_progress_reads} reads"
            );
            ::tokio::time::sleep(CLIENT_POLL_PAUSE).await;
        };

        // The window the service publishes a span for outlasts a client's poll interval,
        // which is what lets that client settle on the in-progress answer at all.
        // The control's own span is unpublished, so how many reads it survives is not what
        // the rule is about: what it shows is that the floor still goes through.
        if published_span {
            assert!(
                in_progress_reads >= STABLE_READS,
                "case {case:?}: expected {operation} to answer {in_progress:?} on at least \
                 {STABLE_READS} reads a poll interval apart, got {in_progress_reads}"
            );
        }

        // And the deletion still GOES THROUGH: from that instant on every such call is
        // refused for an ARN that names nothing.
        assert_eq!(
            once_gone, DOES_NOT_EXIST,
            "case {case:?}: once the deletion is through the answer must say so, got \
             {once_gone:?}"
        );
        for read in 0..STABLE_READS {
            let answered = answer_of(&emulator, operation, &machine).await;
            assert_eq!(
                answered, DOES_NOT_EXIST,
                "case {case:?}: read {read} after the deletion must answer the same, got \
                 {answered:?}"
            );
        }
    }
}
