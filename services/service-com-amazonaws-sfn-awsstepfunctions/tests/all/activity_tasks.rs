// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ACTIVITY TASKS: what a `Task` state whose `Resource` is an
//! activity ARN schedules, and what `GetActivityTask` hands a polling worker, driven
//! over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. a `Task` state whose `Resource` is an activity ARN SCHEDULES one task under
//!    that activity, carrying the state's effective input (`InputPath`, then the
//!    `Parameters` payload template), and the execution waits at that state: it stays
//!    `RUNNING` with no outcome until the task is answered or the clock closes it;
//! 2. `GetActivityTask` hands the longest-waiting unclaimed task of the activity it
//!    names to the poll, ONCE — the `taskToken` that identifies it and that stored
//!    input — and answers an empty token when the activity has no task to hand out,
//!    after holding the poll open;
//! 3. a poll is judged in two steps, as every other activity read is: an
//!    `activityArn` that is not an activity ARN of this service in this endpoint's
//!    region is `InvalidArn` (400) before any lookup, and an ARN the store holds no
//!    activity under is `ActivityDoesNotExist` (400);
//! 4. a task the scheduling state BOUNDED is closed by the clock when that bound runs
//!    out — both `TimeoutSeconds` and `HeartbeatSeconds` running from the instant a
//!    worker took it — and the execution waiting at it then reports the language's own
//!    `States.Timeout`;
//! 5. a `Task` state that names any other integration schedules nothing: it is the
//!    call to another service this emulator does not make, so the execution ends
//!    `FAILED` with `States.Runtime` and no worker is handed anything;
//! 6. the scheduling state's own `Catch` answers for the task either way it can fail —
//!    a bound of its own closing it, and a failure a worker reported — so the FIRST
//!    catcher whose `ErrorEquals` names the error takes the execution on at its `Next`
//!    with the `{Error, Cause}` error output placed through that catcher's `ResultPath`,
//!    and a closure of the HEARTBEAT window answers to both names the language states
//!    for it.
//!
//! The harness (`Emulator`, `string`, `create_machine`, `start_execution`,
//! `describe_execution`) is the one `state_machine_lifecycle` builds; `tests/all.rs`
//! is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::{DATA_LIMIT_ERROR, HEARTBEAT_TIMEOUT_ERROR, MAX_PAYLOAD_BYTES, RUNTIME_ERROR, TIMEOUT_ERROR},
    ACTIVITY_POLL_HOLD,
};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator, ACCOUNT,
    PAST_THE_EXECUTION_DELAY, REGION,
};

/// The longest a `taskToken` may be: the modelled `TaskToken` length (1..2048).
const MAX_TASK_TOKEN_CHARS: usize = 2048;

/// Long enough that a one-second bound the scheduling state declared has run out.
const PAST_A_ONE_SECOND_BOUND: Duration = Duration::from_millis(1_300);

/// How long a poll waits for a status the clock owes before giving up — generous next
/// to the bounds the rows below declare, because what a poll waits for is the
/// CONDITION and never this span.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);

/// How long one poll pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(25);

/// Describe `execution_arn` until it reports `status`, and answer what it then says.
///
/// The wait is for the STATUS and not for a span: the bound the clock has to run out
/// of, and the settle the outcome is owed after it, take as long as the host takes.
async fn reported_as(
    emulator: &Emulator,
    case: &str,
    execution_arn: &str,
    status: &str,
) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) == Some(status) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {execution_arn} to report {status}, got {described:?}"
        );
        ::tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The ARN an activity of `name` carries in the region every request here signs for.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so the suite
/// that drives a RETRIED activity task reads these helpers rather than carrying a
/// second copy of them.
pub(crate) fn activity_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:{name}")
}

/// Create an activity that must succeed, and return its ARN.
pub(crate) async fn create_activity(emulator: &Emulator, name: &str) -> String {
    let created = emulator.ok("CreateActivity", &json!({ "name": name })).await;
    string(&created, "activityArn")
}

/// A one-state definition whose `Task` state polls `activity_arn`, plus whatever
/// other members `extra` declares for that state.
pub(crate) fn activity_definition(activity_arn: &str, extra: Value) -> String {
    let Value::Object(extra) = extra else {
        panic!("expected an object of state members, got {extra:?}")
    };
    let mut state = json!({ "Type": "Task", "Resource": activity_arn, "End": true });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in extra {
        members.insert(member, value);
    }
    json!({ "StartAt": "Work", "States": { "Work": state } }).to_string()
}

/// Poll `activity_arn` once, requiring a 200, and return the body.
pub(crate) async fn poll(emulator: &Emulator, activity_arn: &str, worker: Option<&str>) -> Value {
    let mut request = json!({ "activityArn": activity_arn });
    if let Some(worker) = worker {
        request
            .as_object_mut()
            .expect("the request is an object")
            .insert("workerName".to_string(), json!(worker));
    }
    emulator.ok("GetActivityTask", &request).await
}

/// The `(taskToken, input)` a poll that must have been handed a task answered.
pub(crate) async fn claimed(
    emulator: &Emulator,
    case: &str,
    activity_arn: &str,
    worker: Option<&str>,
) -> (String, Value) {
    let answer = poll(emulator, activity_arn, worker).await;
    let token = answer
        .get("taskToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskToken, got {answer:?}"))
        .to_string();
    assert!(
        !token.is_empty() && token.chars().count() <= MAX_TASK_TOKEN_CHARS,
        "case {case:?}: expected a taskToken within the modelled 1..{MAX_TASK_TOKEN_CHARS}, got \
         {token:?}"
    );
    let input = answer
        .get("input")
        .and_then(Value::as_str)
        .map(|text| {
            serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("case {case:?}: expected JSON input, got {error}"))
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected an input, got {answer:?}"));
    (token, input)
}

/// Whether a poll's answer carries no task at all, which is what "the poll returns a
/// taskToken with a null string" is on the wire.
fn is_empty_answer(answer: &Value) -> bool {
    answer.get("taskToken").is_none_or(Value::is_null)
        && answer.get("input").is_none_or(Value::is_null)
}

/// How many tasks the store holds for `activity_arn`, and how many of those a poll
/// could be handed right now.
///
/// Read from the STORE rather than by polling, because a poll with nothing to hand
/// out is HELD open for [`ACTIVITY_POLL_HOLD`] — the one rule asserted on the wire,
/// by the one test below that waits a hold out. Every other assertion here is about
/// what the store holds, so it reads the store, the way `sync_execution` does for a
/// record no operation of this API reports.
pub(crate) fn queued_tasks(emulator: &Emulator, activity_arn: &str) -> (usize, usize) {
    let state = emulator.store();
    let store = state.lock();
    let now = SystemTime::now();
    let mine = store
        .activity_tasks
        .values()
        .filter(|task| task.activity_arn.as_deref() == Some(activity_arn));
    let mut held = 0;
    let mut claimable = 0;
    for task in mine {
        held += 1;
        if task.is_queued() && task.available_at <= now {
            claimable += 1;
        }
    }
    (held, claimable)
}

// ---------------------------------------------------------------------------
// What the scheduling state queues, and what the poll is handed
// ---------------------------------------------------------------------------

/// One task is scheduled per execution, carrying the state's EFFECTIVE input, and the
/// poll is handed exactly that — over a table whose rows differ along the input's
/// shape and size and along the data-flow members the state applies to it.
#[tokio::test]
async fn get_activity_task_hands_out_the_input_the_state_scheduled() {
    let emulator = Emulator::new();

    // Each row: the execution input, the extra members of the `Task` state, and the
    // document the worker must be handed.
    let blob = "x".repeat(4_096);
    let blob_input = format!(r#"{{"blob":"{blob}"}}"#);
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "the empty object a start with no input runs on",
            "{}",
            json!({}),
            json!({}),
        ),
        (
            "an object the state passes through untouched",
            r#"{"order":"A-1","items":2}"#,
            json!({}),
            json!({ "order": "A-1", "items": 2 }),
        ),
        (
            "an array input, which is a JSON document like any other",
            r#"[1,2,3]"#,
            json!({}),
            json!([1, 2, 3]),
        ),
        (
            "a nested document",
            r#"{"a":{"b":{"c":[true,null,"x"]}}}"#,
            json!({}),
            json!({ "a": { "b": { "c": [true, null, "x"] } } }),
        ),
        (
            "characters outside ASCII, which the payload carries verbatim",
            r#"{"who":"wörker-日本"}"#,
            json!({}),
            json!({ "who": "wörker-日本" }),
        ),
        (
            "a larger payload than one request's worth of members",
            &blob_input,
            json!({}),
            json!({ "blob": blob }),
        ),
        (
            "InputPath, which selects the part of the input the task gets",
            r#"{"keep":{"id":7},"drop":"everything else"}"#,
            json!({ "InputPath": "$.keep" }),
            json!({ "id": 7 }),
        ),
        (
            "InputPath null, which discards the input: 'the effective input is {}'",
            r#"{"ignored":true}"#,
            json!({ "InputPath": null }),
            json!({}),
        ),
        (
            "Parameters, which BUILDS what the worker is handed",
            r#"{"order":"A-1"}"#,
            json!({ "Parameters": { "id.$": "$.order", "fixed": 1 } }),
            json!({ "id": "A-1", "fixed": 1 }),
        ),
        (
            "Parameters reading the context object, which is this execution's own",
            r#"{"order":"A-1"}"#,
            json!({ "Parameters": { "state.$": "$$.State.Name" } }),
            json!({ "state": "Work" }),
        ),
        (
            "InputPath and Parameters together, in that order",
            r#"{"keep":{"order":"A-2"},"drop":1}"#,
            json!({ "InputPath": "$.keep", "Parameters": { "id.$": "$.order" } }),
            json!({ "id": "A-2" }),
        ),
        (
            "the bounds the state may declare, which do not change what is scheduled",
            r#"{"order":"A-3"}"#,
            json!({ "TimeoutSeconds": 300, "HeartbeatSeconds": 60 }),
            json!({ "order": "A-3" }),
        ),
    ];

    let mut tokens: Vec<String> = Vec::with_capacity(cases.len());
    for (index, (case, input, extra, expected)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("worker-{index}")).await;
        let definition = activity_definition(&activity, extra);
        let machine = create_machine(&emulator, &format!("machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", input).await;

        let (token, scheduled) = claimed(&emulator, case, &activity, Some("w-1")).await;
        assert_eq!(
            scheduled, expected,
            "case {case:?}: expected the task input {expected}, got {scheduled}"
        );
        assert!(
            !tokens.contains(&token),
            "case {case:?}: expected a token no other task carries, got {token:?}"
        );
        tokens.push(token);

        // The execution WAITS at the state: nothing the clock owed turned it
        // terminal, because what happens next is the worker's reply.
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the execution to wait at its activity task, got {described:?}"
        );
        assert!(
            described.get("output").is_none_or(Value::is_null)
                && described.get("stopDate").is_none_or(Value::is_null),
            "case {case:?}: an execution waiting at a task has no outcome, got {described:?}"
        );
    }
}

/// A task is handed out ONCE, and the tasks of one activity are handed out in the
/// order they were scheduled, whichever execution scheduled them.
#[tokio::test]
async fn each_task_is_handed_to_one_worker_in_schedule_order() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "shared").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "shared-machine", &definition).await;

    // Two executions of the SAME activity: two tasks, queued in start order.
    let first = start_execution(&emulator, &machine, "first", r#"{"n":1}"#).await;
    let second = start_execution(&emulator, &machine, "second", r#"{"n":2}"#).await;
    assert_ne!(
        first, second,
        "the two executions must be different records, got {first} twice"
    );

    let (first_token, first_input) = claimed(&emulator, "the first poll", &activity, Some("w-1")).await;
    let (second_token, second_input) =
        claimed(&emulator, "the second poll", &activity, Some("w-2")).await;
    assert_eq!(
        (first_input, second_input),
        (json!({ "n": 1 }), json!({ "n": 2 })),
        "expected the tasks in the order they were scheduled"
    );
    assert_ne!(
        first_token, second_token,
        "expected two tokens, got {first_token} twice"
    );

    // Both tasks are out with workers, so there is nothing left to hand out.
    let (held, claimable) = queued_tasks(&emulator, &activity);
    assert_eq!(
        (held, claimable),
        (2, 0),
        "expected both tasks to be out with a worker, got {held} held and {claimable} claimable"
    );
}

/// A poll of an activity that has nothing scheduled answers an empty token, and a
/// task scheduled afterwards is handed to the next poll.
///
/// This is the one test that waits a whole [`ACTIVITY_POLL_HOLD`] out, which is why
/// every other assertion about "nothing to hand out" reads the store instead.
#[tokio::test]
async fn a_poll_with_nothing_scheduled_answers_an_empty_token() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "idle").await;

    let began = Instant::now();
    let answer = poll(&emulator, &activity, None).await;
    let held = began.elapsed();
    assert!(
        is_empty_answer(&answer),
        "expected an empty answer from an activity with no task, got {answer:?}"
    );
    assert!(
        held >= ACTIVITY_POLL_HOLD,
        "expected the poll to be held for {ACTIVITY_POLL_HOLD:?}, it answered after {held:?}"
    );

    // The same activity, once an execution has scheduled a task for it.
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "idle-machine", &definition).await;
    start_execution(&emulator, &machine, "only", r#"{"ready":true}"#).await;
    let (_, scheduled) = claimed(&emulator, "after a task is scheduled", &activity, None).await;
    assert_eq!(
        scheduled,
        json!({ "ready": true }),
        "expected the task the execution scheduled, got {scheduled}"
    );
}

/// A task behind a `Wait` the definition declares is not available to a worker before
/// the clock reaches it: the state that schedules it runs when that span has run out, so
/// until then the store holds no task for the activity at all and the execution is
/// running inside the wait.
#[tokio::test]
async fn a_task_behind_a_wait_state_is_not_available_yet() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "delayed").await;
    // One `Wait` of 30 seconds in front of the task: far longer than a poll is held,
    // so no poll in this test can be handed it.
    let definition = json!({
        "StartAt": "Hold",
        "States": {
            "Hold": { "Type": "Wait", "Seconds": 30, "Next": "Work" },
            "Work": { "Type": "Task", "Resource": activity, "End": true },
        },
    })
    .to_string();
    let machine = create_machine(&emulator, "delayed-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;

    let (held, claimable) = queued_tasks(&emulator, &activity);
    assert_eq!(
        (held, claimable),
        (0, 0),
        "expected no task to be scheduled while the wait in front of it is still running, \
         got {held} held and {claimable} claimable"
    );
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "expected the execution to still be running, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// The two refusals: the ARN, then the resource
// ---------------------------------------------------------------------------

/// An `activityArn` that is not an activity ARN of this service in this endpoint's
/// region is `InvalidArn` (400) — before any lookup — and a well-formed one the store
/// holds no activity under is `ActivityDoesNotExist` (400).
#[tokio::test]
async fn a_poll_is_refused_for_an_arn_and_then_for_the_resource() {
    let emulator = Emulator::new();
    let machine = create_machine(
        &emulator,
        "machine",
        r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","End":true}}}"#,
    )
    .await;
    create_activity(&emulator, "present").await;
    create_activity(&emulator, "doomed").await;

    // The rows differ along WHICH component of the ARN is wrong.
    let invalid: Vec<(&str, String)> = vec![
        ("text that is not an ARN at all", "present".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:activity:present"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:activity:present"),
        ),
        ("a state machine ARN", machine.clone()),
        (
            "an activity ARN with no name",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:"),
        ),
        (
            "an activity ARN carrying a qualifier",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:present:1"),
        ),
        (
            "an ARN with no partition",
            format!("arn::states:{REGION}:{ACCOUNT}:activity:present"),
        ),
    ];
    for (case, arn) in invalid {
        let (status, code) = emulator
            .err("GetActivityTask", &json!({ "activityArn": arn }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {case:?}: expected a 400 InvalidArn for {arn}, got {status} {code}"
        );
    }

    emulator
        .ok(
            "DeleteActivity",
            &json!({ "activityArn": activity_arn("doomed") }),
        )
        .await;
    let elsewhere = Emulator::in_region_on(&emulator, "eu-west-1");
    create_activity(&elsewhere, "abroad").await;

    let absent: Vec<(&str, String)> = vec![
        ("a name never created", activity_arn("ghost")),
        ("an activity that was deleted", activity_arn("doomed")),
        (
            "a name created only against another region's endpoint",
            activity_arn("abroad"),
        ),
        (
            "a name that only a state machine carries",
            activity_arn("machine"),
        ),
    ];
    for (case, arn) in absent {
        let (status, code) = emulator
            .err("GetActivityTask", &json!({ "activityArn": arn }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ActivityDoesNotExist"),
            "case {case:?}: expected a 400 ActivityDoesNotExist for {arn}, got {status} {code}"
        );
    }

    // The refusals created nothing: the activity that is there holds no task.
    let (held, _) = queued_tasks(&emulator, &activity_arn("present"));
    assert_eq!(
        held, 0,
        "expected the surviving activity to hold no task, got {held}"
    );
}

/// An activity DELETED while its task is outstanding orphans that task: the worker's
/// poll is refused for the activity that is gone, and the execution is still waiting.
#[tokio::test]
async fn deleting_an_activity_orphans_its_outstanding_task() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "mid-flight").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "mid-flight-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"n":1}"#).await;
    let (token, _) = claimed(&emulator, "before the delete", &activity, Some("w-1")).await;
    assert!(!token.is_empty(), "expected a token, got {token:?}");

    emulator
        .ok("DeleteActivity", &json!({ "activityArn": activity }))
        .await;
    let (status, code) = emulator
        .err("GetActivityTask", &json!({ "activityArn": activity }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ActivityDoesNotExist"),
        "expected a poll of the deleted activity to be refused, got {status} {code}"
    );
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "expected the execution to still be waiting at its task, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// What the clock owes a task
// ---------------------------------------------------------------------------

/// A task the scheduling state BOUNDED is closed when that bound runs out, and the
/// execution waiting at it fails with the language's own `States.Timeout`: both
/// bounds run from the instant a worker took the task, because an activity task's
/// clock starts at the claim ("for Activities, the count begins when GetActivityTask
/// receives a token and ActivityStarted is logged in the execution event history").
#[tokio::test]
async fn a_bound_the_state_declared_closes_the_task_and_fails_the_execution() {
    let emulator = Emulator::new();

    // Each row: the bounds the state declares, and whether a worker claims the task
    // before the bound is waited out. Every row claims: a task no worker holds has
    // started nothing, so neither bound has anything to run against
    // (`an_unclaimed_task_is_never_timed_out` is that rule's own test).
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "TimeoutSeconds, with the task out with a worker",
            json!({ "TimeoutSeconds": 1 }),
            true,
        ),
        (
            "HeartbeatSeconds, which runs from the claim",
            json!({ "HeartbeatSeconds": 1 }),
            true,
        ),
        (
            "both bounds, the heartbeat window being the shorter",
            json!({ "TimeoutSeconds": 300, "HeartbeatSeconds": 1 }),
            true,
        ),
    ];

    for (index, (case, bounds, claim)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("bounded-{index}")).await;
        let definition = activity_definition(&activity, bounds);
        let machine = create_machine(&emulator, &format!("bounded-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;
        if claim {
            claimed(&emulator, case, &activity, Some("w-1")).await;
        }

        let described = reported_as(&emulator, case, &execution, "FAILED").await;
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(TIMEOUT_ERROR),
            "case {case:?}: expected the error {TIMEOUT_ERROR}, got {described:?}"
        );
        assert!(
            described.get("cause").is_none_or(Value::is_null),
            "case {case:?}: expected no cause beside the error name, got {described:?}"
        );
        // The task went with it: there is nothing left for a worker to be handed.
        let (held, _) = queued_tasks(&emulator, &activity);
        assert_eq!(
            held, 0,
            "case {case:?}: expected the closed task to be gone, got {held} still held"
        );
    }
}

/// A task bounded only by a heartbeat window stays claimable while nobody holds it:
/// the window opens at the CLAIM, so an unclaimed task misses no heartbeat.
#[tokio::test]
async fn an_unclaimed_task_misses_no_heartbeat() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "heartbeat-only").await;
    let definition = activity_definition(&activity, json!({ "HeartbeatSeconds": 1 }));
    let machine = create_machine(&emulator, "heartbeat-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"late":true}"#).await;

    std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "expected an unclaimed task to leave its execution running, got {described:?}"
    );
    let (_, scheduled) = claimed(&emulator, "after the window would have closed", &activity, None).await;
    assert_eq!(
        scheduled,
        json!({ "late": true }),
        "expected the task to still be claimable, got {scheduled}"
    );
}

/// An execution that was STOPPED is waiting for nothing, so its outstanding task is
/// not handed to any worker.
#[tokio::test]
async fn stopping_an_execution_withdraws_its_task() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "stopped").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "stopped-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;

    emulator
        .ok(
            "StopExecution",
            &json!({ "executionArn": execution, "error": "ByHand", "cause": "the test" }),
        )
        .await;
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("ABORTED"),
        "expected the stopped execution to report ABORTED, got {described:?}"
    );
    let (held, _) = queued_tasks(&emulator, &activity);
    assert_eq!(
        held, 0,
        "expected a stopped execution's task to be withdrawn, got {held} still held"
    );
}

// ---------------------------------------------------------------------------
// What is NOT an activity task
// ---------------------------------------------------------------------------

/// A `Task` state that names any other integration schedules nothing: it is a call to
/// another service rather than work a worker claims, so the execution ends `FAILED`
/// and no poll of any activity is handed anything.
///
/// Most rows are not run at all, which is `States.Runtime`. A `Resource` that is a
/// Lambda FUNCTION's ARN IS a call — the task invokes that function — so this suite,
/// which mounts this service alone, fails it on the INVOCATION it cannot make, under
/// an error of Lambda's own: either way no activity task is scheduled, which is what
/// this asserts.
#[tokio::test]
async fn a_task_state_that_is_not_an_activity_schedules_nothing() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "untouched").await;

    // Each row is a `Resource` that is not an activity ARN of this service, or an
    // activity task somewhere the walk cannot wait at, beside the error name expected
    // of it — the whole name for a row nothing was run for, and the service half of it
    // for the row whose failure is the invoked service's.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a Lambda function ARN",
            activity_definition(
                &format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:f"),
                json!({}),
            ),
            "Lambda.",
        ),
        (
            "an optimised service integration",
            activity_definition("arn:aws:states:::lambda:invoke", json!({})),
            RUNTIME_ERROR,
        ),
        (
            "a state machine ARN, which is not an activity",
            activity_definition(
                &format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:other"),
                json!({}),
            ),
            RUNTIME_ERROR,
        ),
        (
            "an activity task inside a Map iteration, which has nowhere to wait",
            json!({
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "Items": [1, 2],
                        "ItemProcessor": {
                            "StartAt": "Work",
                            "States": {
                                "Work": { "Type": "Task", "Resource": activity, "End": true },
                            },
                        },
                        "End": true,
                    },
                },
            })
            .to_string(),
            RUNTIME_ERROR,
        ),
    ];

    let mut started = Vec::new();
    for (index, (case, definition, expected)) in cases.into_iter().enumerate() {
        let machine = create_machine(&emulator, &format!("other-{index}"), &definition).await;
        started.push((
            case,
            expected,
            start_execution(&emulator, &machine, "only", "{}").await,
        ));
    }
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    for (case, expected, execution) in started {
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        let reported = described
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reported.starts_with(expected),
            "case {case:?}: expected the error {expected}, got {reported:?} in {described:?}"
        );
    }

    let (held, _) = queued_tasks(&emulator, &activity);
    assert_eq!(
        held, 0,
        "expected no task queued for any activity, got {held}"
    );
}

/// A task whose input outgrew what one state may produce is NOT scheduled: the
/// execution ends with the language's own data-limit error instead, so no worker is
/// handed a document the service could not carry.
#[tokio::test]
async fn a_task_input_past_what_a_state_may_carry_is_not_scheduled() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "outsized").await;
    // Two copies of a document that is itself inside the limit: the template builds
    // what the worker would be handed, so that is what is measured.
    let definition = activity_definition(
        &activity,
        json!({ "Parameters": { "one.$": "$.blob", "two.$": "$.blob" } }),
    );
    let machine = create_machine(&emulator, "outsized-machine", &definition).await;
    let input = json!({ "blob": "x".repeat(MAX_PAYLOAD_BYTES * 2 / 3) }).to_string();
    let execution = start_execution(&emulator, &machine, "only", &input).await;

    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "expected the execution to have failed, got {described:?}"
    );
    assert_eq!(
        described.get("error").and_then(Value::as_str),
        Some(DATA_LIMIT_ERROR),
        "expected the error {DATA_LIMIT_ERROR}, got {described:?}"
    );
    let (held, _) = queued_tasks(&emulator, &activity);
    assert_eq!(
        held, 0,
        "expected nothing queued for a task that could not be scheduled, got {held}"
    );
}

/// The worker-poll limit is a bound on OPEN polls, not on polls ever made: a poll
/// that has answered gives its place back, so a worker polling in a loop is never
/// refused.
#[tokio::test]
async fn a_poll_that_answered_gives_its_place_back() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "serial").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "serial-machine", &definition).await;

    // More polls in a row than one, each of which held its place while it ran: a
    // leak would refuse the later ones with ActivityWorkerLimitExceeded. The guard
    // leaves room for more than one worker ([`MAX_ACTIVITY_POLLERS`]), so a refusal
    // here could only be a place that was not given back.
    for index in 0..3 {
        start_execution(&emulator, &machine, &format!("run-{index}"), &format!(r#"{{"n":{index}}}"#))
            .await;
        let (_, scheduled) = claimed(
            &emulator,
            "one of a worker's repeated polls",
            &activity,
            Some("looping-worker"),
        )
        .await;
        assert_eq!(
            scheduled,
            json!({ "n": index }),
            "case {index}: expected the task that run scheduled, got {scheduled}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the scheduling state's own `Catch` does with the task
// ---------------------------------------------------------------------------

/// Where the catchers below place the error output, and where the state after them
/// reads it from: a `ResultPath` into the state's own input.
const ERROR_INFO_PATH: &str = "$.error-info";

/// A definition whose `Task` state polls `activity_arn` under `bounds` with `catch` as
/// its `Catch`, followed by the `Pass` states those catchers name.
///
/// Each reporting state puts out what the catcher placed at [`ERROR_INFO_PATH`], so the
/// execution's own output IS the error output the catcher wrote — the one way to read
/// that document back through this API.
fn caught_definition(activity_arn: &str, bounds: &Value, catch: Value, reports: &[&str]) -> String {
    let mut task = json!({
        "Type": "Task",
        "Resource": activity_arn,
        "Catch": catch,
        "End": true,
    });
    let members = task.as_object_mut().expect("the state is an object");
    for (member, value) in bounds.as_object().expect("the bounds are an object") {
        members.insert(member.clone(), value.clone());
    }
    let mut states = json!({ "HandOff": task });
    let declared = states.as_object_mut().expect("the states are an object");
    for report in reports {
        declared.insert(
            (*report).to_string(),
            json!({ "Type": "Pass", "OutputPath": ERROR_INFO_PATH, "End": true }),
        );
    }
    json!({ "StartAt": "HandOff", "States": states }).to_string()
}

/// The document an execution that ended `SUCCEEDED` put out.
fn output_of(described: &Value, case: &str) -> Value {
    let output = described
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
    serde_json::from_str(output)
        .unwrap_or_else(|error| panic!("case {case:?}: expected JSON output, got {error} for {output}"))
}

/// A bound the scheduling state declared closing the task is the `Task` state's own
/// failure, so its `Catch` answers for it: the FIRST catcher that names the error takes
/// the execution on at its `Next` with the error output, and with none the failure is
/// the execution's.
///
/// The rows differ along which bound closes the task and along the name the catcher
/// takes it over by. The names are the Amazon States Language's own predefined error
/// names (its "Error names" documentation): `States.Timeout` — "a Task state either ran
/// longer than the TimeoutSeconds value, or failed to send a heartbeat for a period
/// longer than the HeartbeatSeconds value" — `States.HeartbeatTimeout`, the `States.ALL`
/// wildcard, and one name the closure is NOT ("States.TaskFailed [...] matches any known
/// error name except States.Timeout").
#[tokio::test]
async fn a_catcher_takes_over_the_bound_that_closed_the_task() {
    let emulator = Emulator::new();

    // Each row: the bounds the `Task` state declares, the errors its one catcher names,
    // and the output the execution ends with — `None` where no catcher takes the closure
    // over, which leaves it the execution's own failure.
    let cases: Vec<(&str, Value, Value, Option<Value>)> = vec![
        (
            "TimeoutSeconds, caught by the error's own name",
            json!({ "TimeoutSeconds": 1 }),
            json!([TIMEOUT_ERROR]),
            Some(json!({ "Error": TIMEOUT_ERROR, "Cause": "" })),
        ),
        (
            "TimeoutSeconds, caught by the wildcard",
            json!({ "TimeoutSeconds": 1 }),
            json!(["States.ALL"]),
            Some(json!({ "Error": TIMEOUT_ERROR, "Cause": "" })),
        ),
        (
            "HeartbeatSeconds, caught by the heartbeat's own name",
            json!({ "HeartbeatSeconds": 1, "TimeoutSeconds": 120 }),
            json!([HEARTBEAT_TIMEOUT_ERROR]),
            Some(json!({ "Error": TIMEOUT_ERROR, "Cause": "" })),
        ),
        (
            "HeartbeatSeconds, caught by the timeout name the language states for it too",
            json!({ "HeartbeatSeconds": 1, "TimeoutSeconds": 120 }),
            json!([TIMEOUT_ERROR]),
            Some(json!({ "Error": TIMEOUT_ERROR, "Cause": "" })),
        ),
        (
            "a catcher of another error entirely, which takes nothing over",
            json!({ "TimeoutSeconds": 1 }),
            json!(["States.TaskFailed"]),
            None,
        ),
    ];

    for (index, (case, bounds, errors, expected)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("caught-{index}")).await;
        let definition = caught_definition(
            &activity,
            &bounds,
            json!([{ "ErrorEquals": errors, "ResultPath": ERROR_INFO_PATH, "Next": "Report" }]),
            &["Report"],
        );
        let machine = create_machine(&emulator, &format!("caught-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;

        // A state that declares a `Catch` still SCHEDULES its activity's task: the poll
        // is handed it, and the worker then answers nothing at all, so the bound runs
        // out with the task out with that worker.
        let (token, _) = claimed(&emulator, case, &activity, Some("w-1")).await;
        assert!(!token.is_empty(), "case {case:?}: expected a token, got {token:?}");

        match expected {
            Some(error_output) => {
                let described = reported_as(&emulator, case, &execution, "SUCCEEDED").await;
                assert_eq!(
                    output_of(&described, case),
                    error_output,
                    "case {case:?}: expected the catcher's error output {error_output}, got \
                     {described:?}"
                );
            }
            None => {
                let described = reported_as(&emulator, case, &execution, "FAILED").await;
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(TIMEOUT_ERROR),
                    "case {case:?}: expected the error {TIMEOUT_ERROR}, got {described:?}"
                );
            }
        }
    }
}

/// A closure both catchers name is taken over by the one the `Catch` array lists FIRST,
/// whichever of the two names the error is reported by: the catchers are matched in the
/// order the definition declares them, not by how specifically they name the error.
#[tokio::test]
async fn the_first_catcher_that_names_the_closure_takes_it_over() {
    let emulator = Emulator::new();

    // Both orders of the same two catchers, over a task whose HEARTBEAT window is the
    // bound that closes it — the closure both names answer to.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "the heartbeat's own name first",
            json!([HEARTBEAT_TIMEOUT_ERROR, TIMEOUT_ERROR]),
            "HeartbeatCaught",
        ),
        (
            "the timeout name first",
            json!([TIMEOUT_ERROR, HEARTBEAT_TIMEOUT_ERROR]),
            "TimeoutCaught",
        ),
    ];

    for (index, (case, order, winner)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("ordered-{index}")).await;
        let catch: Vec<Value> = order
            .as_array()
            .expect("the order is an array")
            .iter()
            .map(|error| {
                json!({
                    "ErrorEquals": [error],
                    "ResultPath": ERROR_INFO_PATH,
                    "Next": match error.as_str() {
                        Some(HEARTBEAT_TIMEOUT_ERROR) => "HeartbeatCaught",
                        _ => "TimeoutCaught",
                    },
                })
            })
            .collect();
        let definition = caught_definition(
            &activity,
            &json!({ "HeartbeatSeconds": 1, "TimeoutSeconds": 120 }),
            Value::Array(catch),
            &["HeartbeatCaught", "TimeoutCaught"],
        );
        let machine =
            create_machine(&emulator, &format!("ordered-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;
        claimed(&emulator, case, &activity, Some("w-1")).await;

        let described = reported_as(&emulator, case, &execution, "SUCCEEDED").await;
        let history = emulator
            .ok("GetExecutionHistory", &json!({ "executionArn": execution }))
            .await;
        let entered: Vec<String> = history
            .get("events")
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .filter(|event| {
                        event.get("type").and_then(Value::as_str) == Some("PassStateEntered")
                    })
                    .filter_map(|event| {
                        event
                            .get("stateEnteredEventDetails")
                            .and_then(|details| details.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            entered,
            vec![winner.to_string()],
            "case {case:?}: expected only {winner} to have run, got {entered:?} with \
             {described:?}"
        );
    }
}

/// A failure a WORKER reported is the `Task` state's own in exactly the same way, so the
/// same `Catch` answers for it — carrying the `error` and the `cause` that worker
/// reported into the error output.
#[tokio::test]
async fn a_catcher_takes_over_a_failure_the_worker_reported() {
    let emulator = Emulator::new();

    // Each row: the errors the catcher names, and what the worker reports. The names are
    // the language's own ("States.TaskFailed [...] acts as a wildcard that matches any
    // known error name except States.Timeout").
    let cases: Vec<(&str, Value, Option<&str>, Option<&str>)> = vec![
        (
            "the error the worker named",
            json!(["OutOfCapacity"]),
            Some("OutOfCapacity"),
            Some("the pool is empty"),
        ),
        (
            "the task-failed wildcard",
            json!(["States.TaskFailed"]),
            Some("SomethingElse"),
            None,
        ),
        (
            "a worker that named no error at all, under the wildcard",
            json!(["States.ALL"]),
            None,
            None,
        ),
    ];

    for (index, (case, errors, error, cause)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("reported-{index}")).await;
        let definition = caught_definition(
            &activity,
            &json!({}),
            json!([{ "ErrorEquals": errors, "ResultPath": ERROR_INFO_PATH, "Next": "Report" }]),
            &["Report"],
        );
        let machine =
            create_machine(&emulator, &format!("reported-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;
        let (token, _) = claimed(&emulator, case, &activity, Some("w-1")).await;

        let mut reply = json!({ "taskToken": token });
        let members = reply.as_object_mut().expect("the reply is an object");
        if let Some(error) = error {
            members.insert("error".to_string(), json!(error));
        }
        if let Some(cause) = cause {
            members.insert("cause".to_string(), json!(cause));
        }
        emulator.ok("SendTaskFailure", &reply).await;

        let described = reported_as(&emulator, case, &execution, "SUCCEEDED").await;
        let mut expected = json!({ "Cause": cause.unwrap_or_default() });
        if let Some(error) = error {
            expected
                .as_object_mut()
                .expect("the document is an object")
                .insert("Error".to_string(), json!(error));
        }
        assert_eq!(
            output_of(&described, case),
            expected,
            "case {case:?}: expected the catcher's error output {expected}, got {described:?}"
        );
    }
}
