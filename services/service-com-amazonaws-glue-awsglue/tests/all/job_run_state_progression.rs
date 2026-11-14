// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATES a Glue job run passes through on its way to rest, read
//! back through `GetJobRun` and `GetJobRuns`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the scripts
//! the runs execute are staged through Amazon S3's own `PutObject`, exactly as in
//! [`crate::job_script_execution`].
//!
//! The rule under test: a run that has been started OCCUPIES each non-terminal state of its
//! lifecycle before the next one falls due — it is `STARTING` until the span the clock owes
//! it passes, `RUNNING` from then until one span after the one execution of its script has
//! answered, and only then in the state that execution came to. So a client that polls the
//! run for progress observes `RUNNING` before the terminal state, whatever its polling
//! interval and however quickly the script itself ran, and the reason a failed run reports is
//! the script's own.
//!
//! The recorded case — a job started with `StartJobRun` and then polled with `GetJobRun`
//! until it finishes, watching for `RUNNING` — is ONE row of the tables below; the rows
//! differ along the command kind, what the script does (exits cleanly, raises, is not named
//! at all), who began the run (a caller's start, a trigger's firing), how the client
//! observes it (polling only, or arriving long after each owed span fell due) and how many
//! attempts the job's retry budget buys.
//!
//! Every wait here is a poll for the CONDITION it waits on, with the owed span either held
//! open or made due THROUGH THE STORE rather than slept out, so nothing these tests observe
//! depends on how fast the host runs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    JOB_RUN_NO_SCRIPT_LOCATION, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_RUNNING,
    JOB_RUN_STATE_STARTING, JOB_RUN_STATE_SUCCEEDED, TERMINAL_JOB_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{
    already_past, far_future, get_run, hold_rest_open, hold_running, run_at_rest,
    set_due_instant, start_run, RUN_AT_REST_DEADLINE, RUN_POLL_INTERVAL,
};
use crate::job_script_execution::{
    create_bucket, emulator_with_s3, stage_script, ROLE_ARN, SCRIPTS,
};
use crate::wire::ok;

/// A script that exits cleanly, having computed something and written nothing.
const EXITS_CLEANLY: &str = r#"
rows = [row for row in range(10) if row % 3 == 0]
assert sum(rows) == 18, rows
"#;

/// A script that raises, so the run comes to rest in the modelled failure state carrying the
/// SCRIPT's own reason.
const RAISES: &str = r#"
raise RuntimeError("the input prefix held no partitions")
"#;

/// The text the raising script's own exception carries, which is what the failed run has to
/// report: the reason belongs to the script, not to this emulator.
const RAISED_REASON: &str = "the input prefix held no partitions";

/// Who began the run under test.
#[derive(Debug, Clone, Copy)]
enum BegunBy {
    /// A caller's own `StartJobRun`.
    AStart,
    /// The firing of an `ON_DEMAND` trigger whose one action names the job.
    ATriggerFiring,
}

/// What a row expects of the run's `ErrorMessage` once it has come to rest.
#[derive(Debug)]
enum Reason {
    /// None at all, which is what a run that failed at nothing reports.
    None,
    /// The message contains this — the script's own exception text, which the interpreter
    /// wraps in its own traceback.
    Contains(&'static str),
    /// The whole message, exactly.
    Exact(&'static str),
}

/// One row: a job whose script does something, who begins the run of it, and what that run
/// must come to rest in.
#[derive(Debug)]
struct Row {
    /// What makes the row recognisable in an assertion.
    case: &'static str,
    /// The job (and script object) name.
    name: &'static str,
    /// `JobCommand$Name`.
    command: &'static str,
    /// The script to stage, or `None` for a job whose `Command` names no `ScriptLocation`.
    script: Option<&'static str>,
    /// Who begins the run.
    begun_by: BegunBy,
    /// The `JobRunState` the run must come to rest in.
    state: &'static str,
    /// What its `ErrorMessage` must be.
    reason: Reason,
}

/// A `CreateJob` body for the row: the three `@required` members, the command kind it names
/// and the script location it points at, plus room for the one run each row begins.
fn job_body(row: &Row, location: Option<&str>) -> Value {
    let mut command = json!({ "Name": row.command });
    if let Some(location) = location {
        command["ScriptLocation"] = json!(location);
    }
    json!({
        "Name": row.name,
        "Role": ROLE_ARN,
        "Command": command,
        "ExecutionProperty": { "MaxConcurrentRuns": 2 },
    })
}

/// Create the row's job, staging its script where it names one, and answer the id of the one
/// run the row's beginner begins.
///
/// `hold` holds every span the run is owed open before anything else reads it, for a test
/// that then makes them due one at a time; a test that only POLLS passes `false`, so the run
/// advances on the clock alone.
async fn begin_the_rows_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
    hold: bool,
) -> String {
    let location = match row.script {
        Some(source) => {
            let key = format!("{SCRIPTS}{}.py", row.name);
            Some(stage_script(router, &key, source).await)
        }
        None => None,
    };
    ok(router, "CreateJob", &job_body(row, location.as_deref())).await;
    match row.begun_by {
        BegunBy::AStart => {
            let run_id = start_run(router, &json!({ "JobName": row.name })).await;
            if hold {
                set_due_instant(engine, row.name, far_future());
            }
            run_id
        }
        BegunBy::ATriggerFiring => {
            let trigger = format!("{}-trigger", row.name);
            ok(
                router,
                "CreateTrigger",
                &json!({
                    "Name": trigger,
                    "Type": "ON_DEMAND",
                    "Actions": [{ "JobName": row.name }],
                }),
            )
            .await;
            ok(router, "StartTrigger", &json!({ "Name": trigger })).await;
            if hold {
                set_due_instant(engine, row.name, far_future());
            }
            // A firing names no run, so the run it began is read back off the job.
            let run = one_run(router, row.case, row.name).await;
            run.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the firing's run must carry an Id, got {run}",
                        row.case
                    )
                })
                .to_string()
        }
    }
}

/// The one run `GetJobRuns` reports for `job`.
async fn one_run(router: &MultiServiceRouter, case: &str, job: &str) -> Value {
    let page = ok(router, "GetJobRuns", &json!({ "JobName": job })).await;
    let runs = page
        .get("JobRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        runs.len(),
        1,
        "case {case:?}: {job:?} must hold exactly one run, got {page}"
    );
    runs[0].clone()
}

/// The `JobRunState` a reported run is in.
fn state_of(run: &Value) -> &str {
    run.get("JobRunState")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Assert the run's `ErrorMessage` is what the row expects.
fn assert_reason(case: &str, row_reason: &Reason, run: &Value) {
    let reported = run.get("ErrorMessage").and_then(Value::as_str);
    match row_reason {
        Reason::None => assert!(
            reported.is_none(),
            "case {case:?}: a run that failed at nothing carries no ErrorMessage, got {run}"
        ),
        Reason::Contains(text) => assert!(
            reported.is_some_and(|message| message.contains(text)),
            "case {case:?}: the failed run must report the script's own reason {text:?}, got \
             {run}"
        ),
        Reason::Exact(text) => assert_eq!(
            reported,
            Some(*text),
            "case {case:?}: expected the ErrorMessage {text:?}, got {run}"
        ),
    }
}

/// The rows of the progression table.
fn rows() -> [Row; 5] {
    [
        Row {
            // The recorded case: a job whose script runs and succeeds, started by a caller
            // and polled for progress.
            case: "a glueetl script that exits cleanly, started by a caller",
            name: "progress-clean",
            command: "glueetl",
            script: Some(EXITS_CLEANLY),
            begun_by: BegunBy::AStart,
            state: JOB_RUN_STATE_SUCCEEDED,
            reason: Reason::None,
        },
        Row {
            case: "a pythonshell script that exits cleanly, started by a caller",
            name: "progress-shell",
            command: "pythonshell",
            script: Some(EXITS_CLEANLY),
            begun_by: BegunBy::AStart,
            state: JOB_RUN_STATE_SUCCEEDED,
            reason: Reason::None,
        },
        Row {
            case: "a script that raises, started by a caller",
            name: "progress-raises",
            command: "glueetl",
            script: Some(RAISES),
            begun_by: BegunBy::AStart,
            state: JOB_RUN_STATE_FAILED,
            reason: Reason::Contains(RAISED_REASON),
        },
        Row {
            case: "a job whose Command names no ScriptLocation",
            name: "progress-no-script",
            command: "glueetl",
            script: None,
            begun_by: BegunBy::AStart,
            state: JOB_RUN_STATE_FAILED,
            reason: Reason::Exact(JOB_RUN_NO_SCRIPT_LOCATION),
        },
        Row {
            case: "a clean script, begun by a trigger's firing",
            name: "progress-triggered",
            command: "glueetl",
            script: Some(EXITS_CLEANLY),
            begun_by: BegunBy::ATriggerFiring,
            state: JOB_RUN_STATE_SUCCEEDED,
            reason: Reason::None,
        },
    ]
}

/// A started run is still in flight while the spans the clock owes it are held open,
/// `RUNNING` once the next state it is owed falls due, and in the state its own execution
/// came to only after that.
///
/// The spans are held open and made due THROUGH THE STORE, so each state is observed exactly
/// where the lifecycle puts it and nothing depends on the host's speed. Making a span due
/// that fell due long ago is also the worst case for a client that polls slowly: it must
/// still observe the run running before the terminal state.
#[tokio::test]
async fn a_started_run_is_running_before_it_comes_to_rest() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for row in &rows() {
        let case = row.case;
        let run_id = begin_the_rows_run(&router, &engine, row, true).await;

        // While every owed span is held open — the beginner above holds them — the run is
        // still in flight: it is starting, or already running where its script took longer
        // than the first span, and nothing about it has finished.
        let in_flight = get_run(&router, row.name, &run_id).await;
        assert!(
            [JOB_RUN_STATE_STARTING, JOB_RUN_STATE_RUNNING].contains(&state_of(&in_flight)),
            "case {case:?}: a run whose owed spans are held open is still in flight, got \
             {in_flight}"
        );
        assert!(
            in_flight.get("CompletedOn").is_none(),
            "case {case:?}: a run that has not finished reports no CompletedOn, got {in_flight}"
        );
        assert_eq!(
            in_flight.get("ExecutionTime").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a run that has not finished consumed no whole second, got {in_flight}"
        );

        // Brought into the state it is in while its script runs, and held there: the run
        // must report RUNNING, and it is still not finished.
        hold_running(&engine, row.name);
        let running = get_run(&router, row.name, &run_id).await;
        assert_eq!(
            state_of(&running),
            JOB_RUN_STATE_RUNNING,
            "case {case:?}: the state the clock owes a started run is \
             {JOB_RUN_STATE_RUNNING}, got {running}"
        );
        assert!(
            running.get("CompletedOn").is_none(),
            "case {case:?}: a running run reports no CompletedOn, got {running}"
        );
        assert!(
            running.get("ErrorMessage").is_none(),
            "case {case:?}: a running run carries no ErrorMessage yet, got {running}"
        );

        // And only then does it come to rest in what its own execution came to.
        let settled = run_at_rest(&router, &engine, case, row.name, &run_id).await;
        assert_eq!(
            state_of(&settled),
            row.state,
            "case {case:?}: expected the run to come to rest in {:?}, got {settled}",
            row.state
        );
        assert!(
            settled.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {case:?}: a finished run reports when it finished, got {settled}"
        );
        assert_reason(case, &row.reason, &settled);
    }
}

/// The distinct states a client that ONLY polls `GetJobRun` observes, in the order it
/// observed them, until `stop_at` holds for the state it has just read.
///
/// Nothing is forced through the store from here: what moves the run is the span the clock
/// owes it and the execution its script was owed, so this is what the client of the recorded
/// case sees.
async fn polled_states(
    router: &MultiServiceRouter,
    case: &str,
    job: &str,
    run_id: &str,
    stop_at: impl Fn(&str) -> bool,
) -> Vec<String> {
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    let mut observed: Vec<String> = Vec::new();
    loop {
        let run = get_run(router, job, run_id).await;
        let state = state_of(&run).to_string();
        if observed.last() != Some(&state) {
            observed.push(state.clone());
        }
        if stop_at(&state) {
            return observed;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case:?}: run {run_id:?} of {job:?} must reach the state the poll waits \
             for, and it reports {run}"
        );
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
}

/// A client that only polls observes the run RUNNING before it observes the terminal state,
/// and observes no state outside the lifecycle.
///
/// This is the recorded client's own loop: start the run, then read it again and again until
/// it finishes. Every state the client sees is one the CLOCK put the run in and a read
/// materialised — the one thing fixed through the store is that the run may not come to REST
/// before the client has polled it running, because a poll loop on a loaded host can step
/// over a span, and what this test observes must not depend on how fast the host runs.
#[tokio::test]
async fn a_client_that_only_polls_observes_the_run_running() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for row in &rows() {
        let case = row.case;
        let run_id = begin_the_rows_run(&router, &engine, row, false).await;
        hold_rest_open(&engine, row.name);

        // Polled until it is running: whichever states the client saw on the way, it saw
        // them by reading the run.
        let mut observed =
            polled_states(&router, case, row.name, &run_id, |state| {
                state == JOB_RUN_STATE_RUNNING
            })
            .await;
        assert!(
            !observed
                .iter()
                .any(|state| TERMINAL_JOB_RUN_STATES.contains(&state.as_str())),
            "case {case:?}: the poller must reach {JOB_RUN_STATE_RUNNING} without the run \
             having come to rest on the way, got {observed:?}"
        );

        // And then, with the rest owed again, polled until it has come to rest.
        set_due_instant(&engine, row.name, already_past());
        let rest = polled_states(&router, case, row.name, &run_id, |state| {
            TERMINAL_JOB_RUN_STATES.contains(&state)
        })
        .await;
        for state in rest {
            if observed.last() != Some(&state) {
                observed.push(state);
            }
        }
        assert_eq!(
            observed.last().map(String::as_str),
            Some(row.state),
            "case {case:?}: the last state a poller observes is the one the run came to rest \
             in, got {observed:?}"
        );
        assert_eq!(
            observed
                .iter()
                .position(|state| state == JOB_RUN_STATE_RUNNING),
            Some(observed.len() - 2),
            "case {case:?}: {JOB_RUN_STATE_RUNNING} is the state just before the terminal \
             one, got {observed:?}"
        );
        // Nothing outside the lifecycle is ever observed, and `STARTING` — which a poller
        // that happened to arrive after the first owed span had passed may have missed — is
        // only ever the FIRST state.
        for (index, state) in observed.iter().enumerate() {
            assert!(
                [JOB_RUN_STATE_STARTING, JOB_RUN_STATE_RUNNING, row.state]
                    .contains(&state.as_str()),
                "case {case:?}: a poller observed the unexpected state {state:?} in \
                 {observed:?}"
            );
            assert!(
                state != JOB_RUN_STATE_STARTING || index == 0,
                "case {case:?}: {JOB_RUN_STATE_STARTING} is only ever the first state \
                 observed, got {observed:?}"
            );
        }
    }
}

/// The distinct states EVERY run of one job is observed in, by run id, until no run of that
/// job is still in flight.
///
/// Each turn brings the runs still in flight into the state they are in while their script
/// runs and reads them there, then makes everything the clock owes them due and reads again —
/// so the chain advances one attempt per turn and nothing depends on a poll landing inside a
/// span. The loop is still bounded by a deadline, and a run the service never finishes fails
/// the case instead of hanging.
async fn observed_states_of_every_run(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    job: &str,
) -> BTreeMap<String, Vec<String>> {
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    let mut observed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    loop {
        for (held, due) in [(true, false), (false, true)] {
            if held {
                hold_running(engine, job);
            }
            if due {
                set_due_instant(engine, job, already_past());
            }
            let page = ok(router, "GetJobRuns", &json!({ "JobName": job })).await;
            let runs = page
                .get("JobRuns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for run in &runs {
                let id = run
                    .get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let state = state_of(run).to_string();
                let states = observed.entry(id).or_default();
                if states.last() != Some(&state) {
                    states.push(state);
                }
            }
            if due
                && !runs.is_empty()
                && runs
                    .iter()
                    .all(|run| TERMINAL_JOB_RUN_STATES.contains(&state_of(run)))
            {
                return observed;
            }
            assert!(
                SystemTime::now() < deadline,
                "case {case:?}: every run of {job:?} must come to rest, and they report \
                 {runs:?}"
            );
        }
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
}

/// Every ATTEMPT of a run the job's retry budget buys passes through `RUNNING` too: the rule
/// is about a run of a job definition, so it holds for the attempt the service runs as much
/// as for the one a caller started.
///
/// The rows differ along the retry budget the job carries, and so along how many attempts one
/// start settles into.
#[tokio::test]
async fn every_attempt_of_a_retried_run_is_running_before_it_comes_to_rest() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    // (case, Job$MaxRetries, how many attempts one start must settle into)
    let cases: [(&str, i64, usize); 2] =
        [("no retry budget", 0, 1), ("two further attempts", 2, 3)];

    for (case, max_retries, attempts) in cases {
        let name = format!("progress-retries-{max_retries}");
        let key = format!("{SCRIPTS}{name}.py");
        let location = stage_script(&router, &key, RAISES).await;
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": name,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": location },
                "MaxRetries": max_retries,
                // Each attempt of the chain is a run of its own, and the attempt a failed
                // one buys begins while nothing else of the job is in flight.
                "ExecutionProperty": { "MaxConcurrentRuns": 2 },
            }),
        )
        .await;
        let _ = start_run(&router, &json!({ "JobName": name })).await;
        // The chain advances one attempt per turn, and every attempt still in flight is read
        // in the state it is in while its script runs.
        let observed = observed_states_of_every_run(&router, &engine, case, &name).await;

        assert_eq!(
            observed.len(),
            attempts,
            "case {case:?}: one start of a job with MaxRetries {max_retries} must settle into \
             {attempts} attempt(s), got {observed:?}"
        );
        for (run_id, states) in &observed {
            assert!(
                states.iter().any(|state| state == JOB_RUN_STATE_RUNNING),
                "case {case:?}: attempt {run_id:?} must pass through \
                 {JOB_RUN_STATE_RUNNING}, got {states:?}"
            );
            assert_eq!(
                states.last().map(String::as_str),
                Some(JOB_RUN_STATE_FAILED),
                "case {case:?}: every attempt of a raising script ends in \
                 {JOB_RUN_STATE_FAILED}, got {states:?}"
            );
            assert_eq!(
                states
                    .iter()
                    .position(|state| state == JOB_RUN_STATE_RUNNING),
                Some(states.len() - 2),
                "case {case:?}: attempt {run_id:?} is {JOB_RUN_STATE_RUNNING} just before it \
                 comes to rest, got {states:?}"
            );
        }
    }
}
