// SPDX-License-Identifier: CC0-1.0

//! WHICH value a `TaskTimedOut` refusal quotes, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`.
//!
//! The rule under test: a `SendTask*` call quoting a `taskToken` whose task is no longer
//! outstanding is refused 400 `TaskTimedOut` with the condition and then, quoted, the
//! ACTIVITY that handed that task out — read from the token, which names it, so a store
//! that no longer holds the task still answers with it — and for a task of NO activity (a
//! callback task, whose token names the execution that was waiting at it) the reason
//! itself, since there is no activity to name.
//!
//! The rows differ along what the rule generalises over: the ACTIVITY the token names, so
//! the quoted value is really that token's own (a one-character name, a hyphenated one, a
//! dotted and underscored one, and the longest the modelled `Name` takes); WHY the token
//! answers nothing (its task was answered, its execution was stopped under it, or no task
//! was ever scheduled under it); and WHICH of the three `SendTask*` operations arrives
//! under it.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`) is the one
//! `state_machine_lifecycle` builds, the activity helpers (`create_activity`,
//! `activity_definition`, `claim`, `MINTED_ID`) are `activity_callbacks`', and the refusal
//! assertion (`REPLIES`, `refused`) and the test-side spelling of the message
//! (`timed_out_message`, `NO_ACTIVITY_DETAIL`) are `task_timed_out_wording`'s;
//! `tests/all.rs` is this binary's crate root, so each is reached through `crate::<suite>`
//! rather than copied.

use serde_json::json;

use service_com_amazonaws_sfn_awsstepfunctions::state::task_token;

use crate::activity_callbacks::{activity_definition, claim, create_activity, MINTED_ID};
use crate::state_machine_lifecycle::{
    create_machine, start_execution, Emulator, HELLO_DEFINITION,
};
use crate::task_timed_out_wording::{refused, timed_out_message, NO_ACTIVITY_DETAIL, REPLIES};

/// The longest activity name this API takes: the modelled `Name` is `@length(1..80)`
/// (`smithy_query(query="shape",shape="Name")`), so 80 characters is the published upper
/// bound — the quoted ARN is at its longest here.
const LONGEST_NAME_CHARS: usize = 80;

/// Why the token under test answers nothing any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Closed {
    /// A worker already reported the task's result, which closed it.
    ByAnAnswer,
    /// The execution waiting at the task was stopped, so nobody answers it.
    WithItsExecution,
    /// A well-formed token of this activity that no task was ever scheduled under.
    NeverScheduled,
}

/// One row's resources: the token under test, the activity it names, and the OTHER
/// identifiers of that token's task — the state machine and the execution — which the
/// refusal must not carry.
struct Closure {
    token: String,
    activity: String,
    others: Vec<String>,
}

/// Bring `name`'s activity to the state `closed` describes and report what the row needs.
async fn token_of_a_closed_task(
    emulator: &Emulator,
    case: &str,
    name: &str,
    closed: Closed,
) -> Closure {
    let activity = create_activity(emulator, name).await;
    if closed == Closed::NeverScheduled {
        return Closure {
            token: task_token(MINTED_ID, &activity),
            activity,
            others: Vec::new(),
        };
    }
    let machine = create_machine(
        emulator,
        &format!("{name}-machine"),
        &activity_definition(&activity, json!({})),
    )
    .await;
    let execution = start_execution(emulator, &machine, "only", r#"{"job":"work"}"#).await;
    let (token, _) = claim(emulator, case, &activity).await;
    match closed {
        Closed::ByAnAnswer => {
            emulator
                .ok(
                    "SendTaskSuccess",
                    &json!({ "taskToken": token, "output": r#"{"done":true}"# }),
                )
                .await;
        }
        Closed::WithItsExecution => {
            emulator
                .ok("StopExecution", &json!({ "executionArn": execution }))
                .await;
        }
        Closed::NeverScheduled => unreachable!("answered above"),
    }
    Closure {
        token,
        activity,
        others: vec![machine, execution],
    }
}

/// Assert that every `SendTask*` call under `token` is refused 400 `TaskTimedOut` with
/// exactly `expected`, quoting none of `absent` — through the one refusal assertion
/// `task_timed_out_wording` exposes for this error class.
async fn every_reply_is_refused(
    emulator: &Emulator,
    case: &str,
    token: &str,
    expected: &str,
    absent: &[&str],
) {
    for reply in REPLIES {
        refused(emulator, case, *reply, token, expected, absent).await;
    }
}

/// A token whose ACTIVITY task is no longer outstanding is refused with that activity's own
/// ARN quoted after the condition — whatever the activity is named and whatever closed the
/// task.
#[tokio::test]
async fn a_closed_activity_token_is_refused_quoting_its_own_activity() {
    let emulator = Emulator::new();
    let longest = "l".repeat(LONGEST_NAME_CHARS);

    // The names span what the modelled `Name` admits: its documentation allows 0-9, A-Z,
    // a-z, `-` and `_` (and the ARN segment takes `.` as well), within 1..80 characters.
    let rows: Vec<(&str, &str, Closed)> = vec![
        ("a one-character activity name", "a", Closed::ByAnAnswer),
        (
            "a hyphenated, digit-bearing name whose task was answered",
            "worker-pool-7",
            Closed::ByAnAnswer,
        ),
        (
            "a dotted and underscored name whose execution was stopped",
            "team_a.billing.v2",
            Closed::WithItsExecution,
        ),
        (
            "a name at the modelled upper bound, with no task ever scheduled",
            longest.as_str(),
            Closed::NeverScheduled,
        ),
        (
            "a short name with no task ever scheduled under its token",
            "idle",
            Closed::NeverScheduled,
        ),
    ];

    for (case, name, closed) in rows {
        let Closure {
            token,
            activity,
            others,
        } = token_of_a_closed_task(&emulator, case, name, closed).await;
        // Spelled out here, not read from the service: the condition, then the activity
        // the token names, quoted.
        let expected = timed_out_message(&activity);
        assert!(
            activity.ends_with(&format!(":activity:{name}")),
            "case {case:?}: expected the activity ARN to name {name:?}, got {activity:?}"
        );
        // The activity is the ONE identifier this refusal carries: the state machine the
        // task was scheduled under and the execution waiting at it must not reach it.
        let absent: Vec<&str> = others.iter().map(String::as_str).collect();
        every_reply_is_refused(&emulator, case, &token, &expected, &absent).await;
    }
}

/// A CALLBACK token — one that names the EXECUTION that was waiting at its task rather
/// than an activity — has no activity to quote, so the same refusal states the reason
/// instead and names no resource at all.
#[tokio::test]
async fn a_callback_token_is_refused_stating_the_reason_and_naming_no_resource() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "callback-machine", HELLO_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"from":"the start"}"#).await;
    // Spelled out here, not read from the service: the condition, then the reason.
    let expected = timed_out_message(NO_ACTIVITY_DETAIL);
    // What the refusal must NOT carry: there is no activity to name, so neither the
    // execution the token names nor the state machine it ran may stand in for one.
    let absent = [execution.as_str(), machine.as_str()];

    let rows: Vec<(&str, String)> = vec![
        (
            "a callback token of an execution that holds no such task",
            task_token(MINTED_ID, &execution),
        ),
        (
            "a callback token naming an execution the store never held",
            task_token(MINTED_ID, &format!("{execution}-never-started")),
        ),
    ];

    for (case, token) in &rows {
        every_reply_is_refused(&emulator, case, token, &expected, &absent).await;
    }
}
