// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue job-run slice: `StartJobRun` and the `GetJobRun` that
//! reports the run it started.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `StartJobRun` on a stored job definition records a run of it and
//! answers 200 with the id it minted for that run; the run carries the job's own
//! configuration with the members the request overrode applied on top, and `GetJobRun`
//! reports it under that id — `STARTING` until the state the clock owes it falls due,
//! `RUNNING` while the one execution of its script is owed and performed, and the state
//! that execution came to afterwards. The jobs here point their `ScriptLocation` at an
//! object no Amazon S3 serves (Glue is mounted on its own), so that state is the modelled
//! failure state carrying the reason the script could not be read, and the run never
//! reports a result nothing computed.
//!
//! The recorded cross-service case — a `pythonshell` job started by name with no
//! overrides at all — is ONE row of the tables below; the rows differ along which
//! members the start overrides, the command kind, the capacity form, the concurrency the
//! job allows, the arguments sent, and whether the start is a first attempt or a retry.
//!
//! The due instants the lifecycle turns on are read and moved THROUGH THE STORE rather
//! than slept for, and every wait is a poll for the CONDITION it waits on, so what these
//! tests observe does not depend on how fast the host runs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, JobRunEntry, DEFAULT_GLUE_VERSION, FIRST_JOB_RUN_ATTEMPT,
    JOB_RUN_ID_PREFIX, JOB_RUN_SCRIPT_UNREADABLE_LEAD, JOB_RUN_SETTLES_AFTER,
    JOB_RUN_STATE_FAILED, JOB_RUN_STATE_RUNNING, JOB_RUN_STATE_STARTING, MAX_RUNS_PER_JOB,
    MAX_TIMEOUT_MINUTES, PYTHON_SHELL_JOB_LOG_GROUP, TERMINAL_JOB_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator_on, error_code, error_message, ok};

/// The role ARN the recorded cross-service workflow passes to `CreateJob`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The lower and upper bound of the window a transition this emulator owes falls due in.
/// [`JOB_RUN_SETTLES_AFTER`] must sit inside it, and so must every run's stored due
/// instant.
///
/// A span a client has to be able to OBSERVE by polling cannot be sub-second, and a span a
/// test suite has to wait out cannot be long: these are the bounds that leave room for both,
/// and they are the window this project's own guidance gives a transition the emulator
/// chooses the span of.
pub(crate) const OWED_TRANSITION_WINDOW: (Duration, Duration) =
    (Duration::from_secs(2), Duration::from_secs(5));

/// An emulator plus the engine it runs on, so a test can read and move the due instants
/// the run lifecycle turns on instead of waiting for them.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// A `CreateJob` body: the three `@required` members, plus whatever `extra` adds.
pub(crate) fn job_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Create `name` and start one run of it, answering the id `StartJobRun` minted.
pub(crate) async fn create_and_start(
    router: &MultiServiceRouter,
    name: &str,
    job: Value,
) -> String {
    ok(router, "CreateJob", &job_body(name, job)).await;
    start_run(router, &json!({ "JobName": name })).await
}

/// Start a run and answer the `JobRunId` it reports.
pub(crate) async fn start_run(router: &MultiServiceRouter, body: &Value) -> String {
    let started = ok(router, "StartJobRun", body).await;
    started
        .get("JobRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {started}"))
        .to_string()
}

/// The `JobRun` structure `GetJobRun` reports for one run.
pub(crate) async fn get_run(router: &MultiServiceRouter, job_name: &str, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetJobRun",
        &json!({ "JobName": job_name, "RunId": run_id }),
    )
    .await;
    body.get("JobRun")
        .cloned()
        .unwrap_or_else(|| panic!("GetJobRun must report a JobRun, got {body}"))
}

/// The runs one job holds, as the store holds them.
pub(crate) fn stored_runs(
    engine: &Arc<engine::InMemoryEngine>,
    job_name: &str,
) -> Vec<JobRunEntry> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store
        .job(crate::wire::REGION, job_name)
        .map(|job| job.runs.clone())
        .unwrap_or_else(|| panic!("the store must hold job {job_name:?}"))
}

/// Move EVERY due instant an unsettled run of `job_name` still owes to `at`, so a test can
/// hold the transient window open or make what the clock owes due without sleeping.
///
/// A run owes up to TWO states at once — the state it is in while its script runs, and the
/// state its own execution came to — so a helper that makes "what is owed" due has to move
/// both, or a run would stop half way.
pub(crate) fn set_due_instant(
    engine: &Arc<engine::InMemoryEngine>,
    job_name: &str,
    at: SystemTime,
) {
    set_due_instants(engine, job_name, at, at);
}

/// The same, moving the NEXT state a run is owed and the state it comes to REST in to
/// different instants.
fn set_due_instants(
    engine: &Arc<engine::InMemoryEngine>,
    job_name: &str,
    next: SystemTime,
    rest: SystemTime,
) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let job = store
        .job_mut(crate::wire::REGION, job_name)
        .unwrap_or_else(|| panic!("the store must hold job {job_name:?}"));
    for run in &mut job.runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(next);
        }
        if run.rests_at.is_some() {
            run.rests_at = Some(rest);
        }
    }
}

/// Hold the state every unfinished run of `job_name` comes to REST in open, leaving the state
/// the clock owes it NEXT where it was.
///
/// What this fixes is only that a run may not come to rest yet: the transition into the state
/// it is in while its script runs still falls due on the clock, so a test can poll for it.
/// Which instant carries the rest depends on how far the run has got, which is why it is
/// decided per run here.
pub(crate) fn hold_rest_open(engine: &Arc<engine::InMemoryEngine>, job_name: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let job = store
        .job_mut(crate::wire::REGION, job_name)
        .unwrap_or_else(|| panic!("the store must hold job {job_name:?}"));
    for run in &mut job.runs {
        if run.rests_at.is_some() {
            run.rests_at = Some(far_future());
        } else if run.state == JOB_RUN_STATE_RUNNING && run.settles_at.is_some() {
            run.settles_at = Some(far_future());
        }
    }
}

/// Bring every unfinished run of `job_name` into the state it is in WHILE ITS SCRIPT RUNS,
/// and hold it there, so a test can read it in that state without waiting for a clock.
///
/// Which instant that takes depends on how far the run has got, which is why it is decided
/// per run here rather than by the caller: a run the clock still owes that state has it made
/// due and the state it comes to REST in held open; a run whose script outlasted the first
/// span and that is therefore already there has only its rest held open.
pub(crate) fn hold_running(engine: &Arc<engine::InMemoryEngine>, job_name: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let job = store
        .job_mut(crate::wire::REGION, job_name)
        .unwrap_or_else(|| panic!("the store must hold job {job_name:?}"));
    for run in &mut job.runs {
        if run.rests_at.is_some() {
            run.settles_at = Some(already_past());
            run.rests_at = Some(far_future());
        } else if run.settles_at.is_some() {
            // Either the state it is in while its script runs is the one still owed, or its
            // script has answered and the rest is: the first is made due, the second held.
            run.settles_at = Some(if run.settles_to == JOB_RUN_STATE_RUNNING {
                already_past()
            } else {
                far_future()
            });
        }
    }
}

/// An instant far enough out that no owed transition falls due during a test.
pub(crate) fn far_future() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// An instant already past, so the next call settles what the clock owed.
pub(crate) fn already_past() -> SystemTime {
    SystemTime::now() - Duration::from_secs(1)
}

/// How long a poll of a job's runs waits for them to come to rest.
///
/// Deliberately generous, and nothing is asserted about how long anything took: what the
/// poll waits for is the EXECUTION the service owes a run, performed by whichever call
/// finds it owed and bounded by the ceiling this emulator gives one call, plus the span
/// the clock then owes the run itself. A window far longer than both is what separates a
/// run that has not finished YET from one that never finishes, on a host of any speed.
pub(crate) const RUN_AT_REST_DEADLINE: Duration = Duration::from_secs(120);

/// How long one turn of that poll waits before reading again — the gap between two reads
/// of a client that polls, not a span anything is expected to finish inside.
pub(crate) const RUN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Whether a reported run has come to rest, by the states a run never leaves again.
pub(crate) fn at_rest(run: &Value) -> bool {
    let state = run
        .get("JobRunState")
        .and_then(Value::as_str)
        .unwrap_or_default();
    TERMINAL_JOB_RUN_STATES.contains(&state)
}

/// Read the run `run_id` of `job_name` back until it has come to rest, and answer the
/// `JobRun` it then reports.
///
/// The state a run comes to rest in is owed to it by its own script's EXECUTION — which
/// whichever call finds that execution owed performs, the start or a later read — and then
/// by the clock, one owed span later. So the run is polled for that CONDITION rather than
/// assumed to have reached it by the first read, which is what a client polling the run
/// does and what keeps the outcome independent of what the host was doing meanwhile. The
/// owed span is made due through the store on every turn rather than slept out, and a run
/// still not at rest when the window closes fails the case instead of hanging.
pub(crate) async fn run_at_rest(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    job_name: &str,
    run_id: &str,
) -> Value {
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    loop {
        set_due_instant(engine, job_name, already_past());
        let run = get_run(router, job_name, run_id).await;
        if at_rest(&run) {
            return run;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case:?}: run {run_id:?} of {job_name:?} must come to rest, and it reports \
             {run}"
        );
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
}

/// Read the runs of `job_name` back until every one of them has come to rest, and answer
/// them as `GetJobRuns` reports them.
///
/// A chain of attempts advances one attempt per owed span — the retry a failed attempt
/// bought is recorded, and its script executed, by the call that finds that attempt
/// finished — so the chain has come to rest exactly when no run of the job is still in
/// flight. Polled for that condition, as [`run_at_rest`] polls for one run's.
pub(crate) async fn runs_at_rest(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    job_name: &str,
) -> Vec<Value> {
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    loop {
        set_due_instant(engine, job_name, already_past());
        let page = ok(router, "GetJobRuns", &json!({ "JobName": job_name })).await;
        // The chain is read in ONE page: a job of these suites holds far fewer runs than a
        // page does, so a token on the answer would mean runs the caller never saw.
        assert!(
            page.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: every run of {job_name:?} must fit one page, got {page}"
        );
        let listed = page
            .get("JobRuns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: GetJobRuns must report JobRuns, got {page}")
            });
        if !listed.is_empty() && listed.iter().all(at_rest) {
            return listed;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {case:?}: every run of {job_name:?} must come to rest, and they report \
             {listed:?}"
        );
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
}

/// Assert that `run` failed on the one thing a run of these suites' jobs fails on: its
/// script could not be read.
///
/// A job of these suites points its `ScriptLocation` at an object no Amazon S3 serves —
/// Glue is mounted on its own here — so the run's script is never read and the run comes to
/// rest in the modelled failure state naming the script it could not download. What a run
/// whose script this emulator DOES execute reports is pinned by the suite that stages a
/// script in Amazon S3 (`crate::job_script_execution`).
pub(crate) fn assert_script_was_unreadable(case: &str, run: &Value) {
    let message = run
        .get("ErrorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.starts_with(JOB_RUN_SCRIPT_UNREADABLE_LEAD),
        "case {case:?}: a run whose script could not be read must say so, got {run}"
    );
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// What `StartJobRun` records is what `GetJobRun` reports: the job's configuration with
/// the request's overrides applied, in a non-terminal state while the owed transition is
/// held open and in the modelled failure state once it is due.
///
/// Each row is a job definition plus a start of it, and the `JobRun` members that start
/// must produce. The rows differ along the command kind, which members the start
/// overrides (none at all, the timeout, the worker configuration, an explicit capacity,
/// the queuing flag, the session policy), and the capacity form.
#[tokio::test]
async fn a_started_run_is_read_back_through_get_job_run() {
    let (router, engine) = emulator_with_engine();

    // (case, job name, CreateJob members beyond the required three, StartJobRun body,
    //  the JobRun members the pair must produce)
    let cases: Vec<(&str, &str, Value, Value, Value)> = vec![
        (
            // The recorded cross-service case: a Python shell job started by name with
            // no overrides, so the run reports the job's own configuration.
            "no overrides at all",
            "sim2real-bookmark-job",
            json!({
                "Command": {
                    "Name": "pythonshell",
                    "PythonVersion": "3.9",
                    "ScriptLocation": "s3://sim2real/scripts/job.py",
                },
                "MaxCapacity": 0.0625,
                "Timeout": 5,
            }),
            json!({ "JobName": "sim2real-bookmark-job" }),
            json!({
                "JobName": "sim2real-bookmark-job",
                "JobMode": "SCRIPT",
                "GlueVersion": DEFAULT_GLUE_VERSION,
                "Timeout": 5,
                "MaxCapacity": 0.0625,
                // A fraction of a DPU is no whole DPU.
                "AllocatedCapacity": 0,
                "Attempt": FIRST_JOB_RUN_ATTEMPT,
                "Arguments": {},
                // Nothing triggered this run and it retries nothing, so no predicate run
                // precedes it — reported as the empty list.
                "TriggerName": Value::Null,
                "PreviousRunId": Value::Null,
                "PredecessorRuns": [],
                "ProfileName": Value::Null,
                "DPUSeconds": Value::Null,
                "StateDetail": Value::Null,
                // A Python shell job's runs log under their own group, and this job carries
                // no security configuration for the group to be encrypted with.
                "LogGroupName": PYTHON_SHELL_JOB_LOG_GROUP,
                // "If false or not populated, the job runs will not be considered for
                // queueing": neither the job nor the start named the flag.
                "JobRunQueuingEnabled": false,
            }),
        ),
        (
            // "This value overrides the timeout value set in the parent job."
            "the timeout overridden",
            "timeout-override",
            json!({ "Timeout": 60, "GlueVersion": "4.0" }),
            json!({ "JobName": "timeout-override", "Timeout": 1 }),
            json!({ "Timeout": 1, "GlueVersion": "4.0" }),
        ),
        (
            // The run's worker configuration replaces the job's, and the DPU figure is
            // derived from the pair the run ends up with: 2 DPU per G.2X worker.
            "the worker configuration overridden",
            "worker-override",
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 2, "GlueVersion": "4.0" }),
            json!({
                "JobName": "worker-override",
                "WorkerType": "G.2X",
                "NumberOfWorkers": 3,
            }),
            json!({
                "WorkerType": "G.2X",
                "NumberOfWorkers": 3,
                "MaxCapacity": 6.0,
                "AllocatedCapacity": 6,
            }),
        ),
        (
            // Only the worker COUNT is overridden, so the job's worker type still says
            // what each worker is worth: 1 DPU per G.1X worker.
            "only the worker count overridden",
            "worker-count-override",
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 2, "GlueVersion": "4.0" }),
            json!({ "JobName": "worker-count-override", "NumberOfWorkers": 5 }),
            json!({
                "WorkerType": "G.1X",
                "NumberOfWorkers": 5,
                "MaxCapacity": 5.0,
                "AllocatedCapacity": 5,
            }),
        ),
        (
            "an explicit capacity overriding the job's",
            "capacity-override",
            json!({ "MaxCapacity": 2.0, "GlueVersion": "1.0" }),
            json!({ "JobName": "capacity-override", "MaxCapacity": 10.0 }),
            json!({ "MaxCapacity": 10.0, "AllocatedCapacity": 10 }),
        ),
        (
            // "This parameter is deprecated. Use MaxCapacity instead" — the two name one
            // quantity, so a run that names the deprecated one reports both.
            "the deprecated allocated capacity",
            "allocated-capacity-override",
            json!({ "GlueVersion": "1.0" }),
            json!({ "JobName": "allocated-capacity-override", "AllocatedCapacity": 7 }),
            json!({ "MaxCapacity": 7.0, "AllocatedCapacity": 7 }),
        ),
        (
            // "If this field does not match the value set in the job run, then the value
            // from the job run field will be used."
            "the queuing flag overridden",
            "queuing-override",
            json!({ "JobRunQueuingEnabled": false }),
            json!({ "JobName": "queuing-override", "JobRunQueuingEnabled": true }),
            json!({ "JobRunQueuingEnabled": true }),
        ),
        (
            "every member a start may carry",
            "fully-specified-run",
            json!({
                "JobMode": "VISUAL",
                "GlueVersion": "4.0",
                "MaintenanceWindow": "Mon:10",
                "SecurityConfiguration": "job-security",
                "NotificationProperty": { "NotifyDelayAfter": 10 },
            }),
            json!({
                "JobName": "fully-specified-run",
                "Timeout": 120,
                "WorkerType": "G.4X",
                "NumberOfWorkers": 2,
                "SecurityConfiguration": "run-security",
                "NotificationProperty": { "NotifyDelayAfter": 3 },
                "ExecutionClass": "FLEX",
                "ExecutionRoleSessionPolicy": "{\"Version\":\"2012-10-17\"}",
                "Arguments": { "--scale": "3" },
            }),
            json!({
                "JobMode": "VISUAL",
                "GlueVersion": "4.0",
                "MaintenanceWindow": "Mon:10",
                "Timeout": 120,
                "WorkerType": "G.4X",
                "NumberOfWorkers": 2,
                "MaxCapacity": 8.0,
                "AllocatedCapacity": 8,
                "SecurityConfiguration": "run-security",
                "NotificationProperty": { "NotifyDelayAfter": 3 },
                "ExecutionClass": "FLEX",
                "ExecutionRoleSessionPolicy": "{\"Version\":\"2012-10-17\"}",
                "Arguments": { "--scale": "3" },
            }),
        ),
    ];

    for (case, name, job, start, expected) in cases {
        ok(&router, "CreateJob", &job_body(name, job)).await;
        let run_id = start_run(&router, &start).await;
        assert!(
            run_id.starts_with(JOB_RUN_ID_PREFIX)
                && run_id.len() == JOB_RUN_ID_PREFIX.len() + 64
                && run_id[JOB_RUN_ID_PREFIX.len()..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "case {case:?}: the run id must be {JOB_RUN_ID_PREFIX} and 64 lowercase hex \
             characters, got {run_id:?}"
        );

        // Hold the owed transition open, so the run is read in its transient state.
        set_due_instant(&engine, name, far_future());
        let run = get_run(&router, name, &run_id).await;
        assert_eq!(
            run.get("Id").and_then(Value::as_str),
            Some(run_id.as_str()),
            "case {case:?}: the run is reported under the id the start minted, got {run}"
        );
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_STARTING),
            "case {case:?}: a run whose owed transition is not due yet is still \
             {JOB_RUN_STATE_STARTING}, got {run}"
        );
        assert!(
            run.get("CompletedOn").is_none(),
            "case {case:?}: a run that has not finished has no CompletedOn, got {run}"
        );
        assert!(
            run.get("ErrorMessage").is_none(),
            "case {case:?}: a run that has not failed carries no ErrorMessage, got {run}"
        );
        assert_eq!(
            run.get("ExecutionTime").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a run that has not finished consumed no whole second, got {run}"
        );
        assert_echoed(case, &expected, &run);

        // A read does not change what the next caller sees.
        let again = get_run(&router, name, &run_id).await;
        assert_eq!(
            again, run,
            "case {case:?}: reading a run must not move it; first {run}, then {again}"
        );

        // Once the owed transition is due the run is terminal, with the reason it did
        // not run, and everything the start recorded is still reported unchanged.
        set_due_instant(&engine, name, already_past());
        let settled = get_run(&router, name, &run_id).await;
        assert_eq!(
            settled.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_FAILED),
            "case {case:?}: the state the clock owes is {JOB_RUN_STATE_FAILED}, got {settled}"
        );
        assert_script_was_unreadable(case, &settled);
        assert!(
            settled.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {case:?}: a finished run reports when it finished, got {settled}"
        );
        assert_echoed(case, &expected, &settled);

        // And settling is itself idempotent: the instants are the DUE instants, not the
        // instants somebody read them at.
        let settled_again = get_run(&router, name, &run_id).await;
        assert_eq!(
            settled_again, settled,
            "case {case:?}: a settled run must read the same twice; first {settled}, then \
             {settled_again}"
        );
    }
}

/// The transition out of the non-terminal state is one the CLOCK owes: it falls due a
/// non-zero span after the start, whether or not anybody calls.
///
/// The span is asserted from the stored due instant, and then actually waited out once,
/// so what moves the run is the passage of time and not the act of reading it.
#[tokio::test]
async fn the_terminal_state_is_owed_by_the_clock_a_span_after_the_start() {
    let (router, engine) = emulator_with_engine();
    let (lower, upper) = OWED_TRANSITION_WINDOW;
    assert!(
        JOB_RUN_SETTLES_AFTER >= lower && JOB_RUN_SETTLES_AFTER <= upper,
        "the owed transition's span must sit in {lower:?}..={upper:?}, got \
         {JOB_RUN_SETTLES_AFTER:?}"
    );

    // The rows differ along the command kind, because the span is a property of the run
    // lifecycle and not of what is being run.
    for command in ["glueetl", "pythonshell", "gluestreaming", "glueray"] {
        let name = format!("owed-{command}");
        let run_id = create_and_start(
            &router,
            &name,
            json!({ "Command": { "Name": command, "ScriptLocation": "s3://etl/j.py" } }),
        )
        .await;

        let runs = stored_runs(&engine, &name);
        let run = runs
            .iter()
            .find(|run| run.id == run_id)
            .unwrap_or_else(|| panic!("case {command:?}: the store must hold run {run_id:?}"));
        let due = run
            .settles_at
            .unwrap_or_else(|| panic!("case {command:?}: a fresh run must owe a transition"));
        let span = due
            .duration_since(run.started_on)
            .unwrap_or_else(|_| panic!("case {command:?}: the due instant is after the start"));
        assert!(
            span >= lower && span <= upper,
            "case {command:?}: the owed transition must be due {lower:?}..={upper:?} after the \
             start, got {span:?}"
        );
    }

    // Nobody moved a due instant and nobody forced anything: the run leaves the state the
    // start put it in because the CLOCK passed its due instant, so a reader only observes
    // what was already owed. The next state owed a started run is the one a run is in while
    // its script runs, and the run comes to rest after that.
    let name = "owed-by-the-clock";
    let run_id = create_and_start(&router, name, json!({})).await;
    let deadline = SystemTime::now() + RUN_AT_REST_DEADLINE;
    let moved = loop {
        let run = get_run(&router, name, &run_id).await;
        if run.get("JobRunState").and_then(Value::as_str) != Some(JOB_RUN_STATE_STARTING) {
            break run;
        }
        assert!(
            SystemTime::now() < deadline,
            "the transition the clock owes must fall due with nobody forcing it, and the run \
             reports {run}"
        );
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    };
    assert_eq!(
        moved.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_RUNNING),
        "the state the clock owes a started run is {JOB_RUN_STATE_RUNNING}, got {moved}"
    );
    let settled = run_at_rest(&router, &engine, "owed by the clock", name, &run_id).await;
    assert_eq!(
        settled.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_FAILED),
        "the run then comes to rest in {JOB_RUN_STATE_FAILED}, got {settled}"
    );
    assert!(
        TERMINAL_JOB_RUN_STATES.contains(&JOB_RUN_STATE_FAILED),
        "the state the clock owes must be one a run never leaves, got \
         {TERMINAL_JOB_RUN_STATES:?}"
    );
}

/// A job admits at most `ExecutionProperty$MaxConcurrentRuns` runs in flight: the start
/// at the limit succeeds, the one past it is refused with the modelled concurrency error,
/// and a run finishing frees its slot — the limit bounds what is in flight rather than
/// how many runs were ever started.
///
/// The rows differ along the concurrency the job allows, including the documented default
/// a job that names no `ExecutionProperty` gets.
#[tokio::test]
async fn a_start_past_the_jobs_concurrency_limit_is_refused() {
    let (router, engine) = emulator_with_engine();

    // (case, CreateJob members, the number of runs the job admits at once)
    let cases: Vec<(&str, Value, usize)> = vec![
        ("the documented default of one", json!({}), 1),
        (
            "an explicit single run",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 1 } }),
            1,
        ),
        (
            "two concurrent runs",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 2 } }),
            2,
        ),
        (
            "five concurrent runs",
            json!({ "ExecutionProperty": { "MaxConcurrentRuns": 5 } }),
            5,
        ),
    ];

    for (case, job, allowed) in cases {
        let name = format!("concurrency-{allowed}-{}", case.len());
        ok(&router, "CreateJob", &job_body(&name, job)).await;

        // Every start holds its run open, so all `allowed` of them are in flight at once.
        for index in 0..allowed {
            start_run(&router, &json!({ "JobName": name })).await;
            set_due_instant(&engine, &name, far_future());
            assert_eq!(
                stored_runs(&engine, &name).len(),
                index + 1,
                "case {case:?}: start {} of {allowed} must be recorded",
                index + 1
            );
        }

        let (status, answer) = call(&router, "StartJobRun", &json!({ "JobName": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: run {} of {allowed} must be refused, got {status} with {answer}",
            allowed + 1
        );
        assert!(
            error_code(&answer).contains("ConcurrentRunsExceededException"),
            "case {case:?}: expected ConcurrentRunsExceededException, got {answer}"
        );
        assert_eq!(
            stored_runs(&engine, &name).len(),
            allowed,
            "case {case:?}: the refused start must record no run"
        );

        // Finishing the runs in flight frees every slot again.
        set_due_instant(&engine, &name, already_past());
        start_run(&router, &json!({ "JobName": name })).await;
        assert_eq!(
            stored_runs(&engine, &name).len(),
            allowed + 1,
            "case {case:?}: a start after the runs in flight finished must be recorded"
        );
    }
}

/// A start whose `Arguments` the run must carry: the job's `DefaultArguments` with the
/// run's own applied over them, and the job's `NonOverridableArguments` applied last —
/// "arguments for this job that are not overridden when providing job arguments in a job
/// run".
///
/// The rows differ along which of the three argument sets are present and whether they
/// collide, and the count of arguments sent.
#[tokio::test]
async fn run_arguments_overlay_the_job_defaults() {
    let (router, engine) = emulator_with_engine();

    // (case, DefaultArguments, NonOverridableArguments, the run's Arguments, expected)
    let cases: Vec<(&str, Value, Value, Value, Value)> = vec![
        (
            "no arguments anywhere",
            Value::Null,
            Value::Null,
            Value::Null,
            json!({}),
        ),
        (
            "the job's defaults alone",
            json!({ "--job-bookmark-option": "job-bookmark-enable" }),
            Value::Null,
            Value::Null,
            json!({ "--job-bookmark-option": "job-bookmark-enable" }),
        ),
        (
            "the run's arguments alone",
            Value::Null,
            Value::Null,
            json!({ "--input": "s3://lake/in/" }),
            json!({ "--input": "s3://lake/in/" }),
        ),
        (
            "the run's arguments replacing a default",
            json!({ "--input": "s3://lake/in/", "--enable-metrics": "true" }),
            Value::Null,
            json!({ "--input": "s3://lake/other/" }),
            json!({ "--input": "s3://lake/other/", "--enable-metrics": "true" }),
        ),
        (
            "a non-overridable argument the run tried to replace",
            json!({ "--input": "s3://lake/in/" }),
            json!({ "--locked": "yes" }),
            json!({ "--locked": "no", "--input": "s3://lake/other/" }),
            json!({ "--locked": "yes", "--input": "s3://lake/other/" }),
        ),
        (
            "many arguments at once",
            json!({ "--a": "1", "--b": "2", "--c": "3" }),
            json!({ "--c": "locked" }),
            json!({ "--b": "overridden", "--d": "4", "--e": "5" }),
            json!({
                "--a": "1",
                "--b": "overridden",
                "--c": "locked",
                "--d": "4",
                "--e": "5",
            }),
        ),
    ];

    for (case, defaults, locked, arguments, expected) in cases {
        let name = format!("arguments-{}", case.len());
        let mut job = json!({});
        if !defaults.is_null() {
            job["DefaultArguments"] = defaults;
        }
        if !locked.is_null() {
            job["NonOverridableArguments"] = locked;
        }
        ok(&router, "CreateJob", &job_body(&name, job)).await;

        let mut start = json!({ "JobName": name });
        if !arguments.is_null() {
            start["Arguments"] = arguments;
        }
        let run_id = start_run(&router, &start).await;
        set_due_instant(&engine, &name, far_future());

        let run = get_run(&router, &name, &run_id).await;
        let reported: BTreeMap<String, String> = run
            .get("Arguments")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: a run reports its arguments, got {run}"))
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let wanted: BTreeMap<String, String> = expected
            .as_object()
            .expect("every row's expectation is a map")
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the run's arguments to be {wanted:?}, got {reported:?}"
        );
    }
}

/// A start naming a previous run retries it: the new run names the run it retries and is
/// the next attempt of it, while a first start of the same job is attempt zero and names
/// no previous run.
#[tokio::test]
async fn a_retry_names_the_run_it_retries_and_bumps_the_attempt() {
    let (router, engine) = emulator_with_engine();
    let name = "retried-job";
    let first = create_and_start(&router, name, json!({})).await;

    let mut previous = first.clone();
    // Each row is one further retry, so the attempt number is asserted over a sequence
    // rather than at a single point.
    for attempt in 1..=3i64 {
        // A retry is a new run, so the one it retries must have finished first.
        set_due_instant(&engine, name, already_past());
        let retry = start_run(
            &router,
            &json!({ "JobName": name, "JobRunId": previous.clone() }),
        )
        .await;
        set_due_instant(&engine, name, far_future());

        assert_ne!(
            retry, previous,
            "case attempt {attempt}: a retry is a new run with its own id"
        );
        let run = get_run(&router, name, &retry).await;
        assert_eq!(
            run.get("PreviousRunId").and_then(Value::as_str),
            Some(previous.as_str()),
            "case attempt {attempt}: the retry names the run it retries, got {run}"
        );
        assert_eq!(
            run.get("Attempt").and_then(Value::as_i64),
            Some(attempt),
            "case attempt {attempt}: the retry is the next attempt, got {run}"
        );

        // The run it retries is untouched by the retry.
        let retried = get_run(&router, name, &previous).await;
        assert_eq!(
            retried.get("Id").and_then(Value::as_str),
            Some(previous.as_str()),
            "case attempt {attempt}: the retried run is still reported, got {retried}"
        );
        previous = retry;
    }

    let fresh = get_run(&router, name, &first).await;
    assert_eq!(
        fresh.get("Attempt").and_then(Value::as_i64),
        Some(i64::from(FIRST_JOB_RUN_ATTEMPT)),
        "a first start is attempt {FIRST_JOB_RUN_ATTEMPT}, got {fresh}"
    );
    assert!(
        fresh.get("PreviousRunId").is_none(),
        "a first start names no previous run, got {fresh}"
    );
}

/// Every run of every job gets its own id, and the id is what the start answered.
#[tokio::test]
async fn every_start_mints_its_own_run_id() {
    let (router, engine) = emulator_with_engine();
    let mut minted: Vec<String> = Vec::new();

    // Several runs of one job, and runs of other jobs, so the ids are compared both
    // within a job and across jobs.
    for job in ["ids-a", "ids-b"] {
        ok(
            &router,
            "CreateJob",
            &job_body(job, json!({ "ExecutionProperty": { "MaxConcurrentRuns": 10 } })),
        )
        .await;
        for _ in 0..5 {
            let run_id = start_run(&router, &json!({ "JobName": job })).await;
            set_due_instant(&engine, job, far_future());
            let run = get_run(&router, job, &run_id).await;
            assert_eq!(
                run.get("Id").and_then(Value::as_str),
                Some(run_id.as_str()),
                "case {job:?}: the run is reported under the id the start answered, got {run}"
            );
            minted.push(run_id);
        }
    }

    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every start must mint its own id, got {minted:?}"
    );
}

/// A start or a read naming a job or a run this account does not hold is answered with
/// the modelled miss — never an empty success, which a caller would read as a run with no
/// members.
#[tokio::test]
async fn an_unknown_job_or_run_is_not_found() {
    let (router, engine) = emulator_with_engine();
    let run_id = create_and_start(&router, "known", json!({})).await;
    set_due_instant(&engine, "known", far_future());

    // The rows differ along how the name misses: never created, a different casing (the
    // model documents no case fold for a job name) and a prefix of a stored name.
    for job_name in ["absent", "KNOWN", "know"] {
        let (status, answer) = call(&router, "StartJobRun", &json!({ "JobName": job_name })).await;
        assert_eq!(
            status, 400,
            "case {job_name:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {job_name:?}: expected EntityNotFoundException, got {answer}"
        );

        let (status, answer) = call(
            &router,
            "GetJobRun",
            &json!({ "JobName": job_name, "RunId": run_id.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {job_name:?}: a run of an unknown job is not found, got {status} {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {job_name:?}: expected EntityNotFoundException, got {answer}"
        );
    }

    // And a run id the job does not hold, including one that belongs to another job.
    ok(&router, "CreateJob", &job_body("other", json!({}))).await;
    let other_run = start_run(&router, &json!({ "JobName": "other" })).await;
    for (case, run) in [
        ("never minted", format!("{JOB_RUN_ID_PREFIX}{}", "0".repeat(64))),
        ("a run of another job", other_run),
        ("the id without its prefix", run_id[JOB_RUN_ID_PREFIX.len()..].to_string()),
    ] {
        let (status, answer) = call(
            &router,
            "GetJobRun",
            &json!({ "JobName": "known", "RunId": run }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {answer}"
        );
    }

    // The run the account does hold is still reported.
    let run = get_run(&router, "known", &run_id).await;
    assert_eq!(
        run.get("Id").and_then(Value::as_str),
        Some(run_id.as_str()),
        "the misses must not have touched the stored run, got {run}"
    );
}

/// A start `StartJobRun` cannot serve is refused with the error the model names for it,
/// and records no run. The rows differ along which part of the request is at fault: the
/// timeout bound, the two ways of naming capacity, the execution class the job's command
/// and version do not admit, and a retry of a run that does not exist.
#[tokio::test]
async fn an_unservable_start_is_refused_with_the_modeled_error() {
    let (router, engine) = emulator_with_engine();
    ok(
        &router,
        "CreateJob",
        &job_body(
            "refusals",
            json!({
                "Command": { "Name": "pythonshell", "ScriptLocation": "s3://etl/x.py" },
                "GlueVersion": "4.0",
            }),
        ),
    )
    .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            // "Jobs must have timeout values less than 7 days or 10080 minutes."
            "a timeout past the documented maximum",
            json!({ "JobName": "refusals", "Timeout": MAX_TIMEOUT_MINUTES + 1 }),
            "InvalidInputException",
        ),
        (
            // "Do not set MaxCapacity if using WorkerType and NumberOfWorkers."
            "a capacity named twice",
            json!({
                "JobName": "refusals",
                "MaxCapacity": 10.0,
                "WorkerType": "G.1X",
                "NumberOfWorkers": 10,
            }),
            "InvalidInputException",
        ),
        (
            "a capacity named twice, by worker count alone",
            json!({ "JobName": "refusals", "MaxCapacity": 10.0, "NumberOfWorkers": 2 }),
            "InvalidInputException",
        ),
        (
            // "Only jobs with Glue version 3.0 and above and command type glueetl will
            // be allowed to set ExecutionClass to FLEX": this job is a pythonshell one.
            "a flexible execution class the job's command does not admit",
            json!({ "JobName": "refusals", "ExecutionClass": "FLEX" }),
            "InvalidInputException",
        ),
        (
            // "The ID of a previous JobRun to retry."
            "a retry of a run that does not exist",
            json!({
                "JobName": "refusals",
                "JobRunId": format!("{JOB_RUN_ID_PREFIX}{}", "f".repeat(64)),
            }),
            "EntityNotFoundException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, answer) = call(&router, "StartJobRun", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains(expected),
            "case {case:?}: expected {expected}, got {answer}"
        );
        assert!(
            !error_message(&answer).is_empty(),
            "case {case:?}: the refusal must say what is wrong, got {answer}"
        );
        assert!(
            stored_runs(&engine, "refusals").is_empty(),
            "case {case:?}: a refused start must record no run"
        );
    }

    // A timeout one below the modelled minimum is refused at the request boundary,
    // before the handler runs, so only the refusal itself is asserted here.
    let (status, answer) = call(
        &router,
        "StartJobRun",
        &json!({ "JobName": "refusals", "Timeout": 0 }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case timeout 0: expected 400, got {status} with {answer}"
    );
    assert!(
        answer.get("__type").is_some(),
        "case timeout 0: the refusal must name a modeled error, got {answer}"
    );

    // Each documented bound of the timeout, and the job's own capacity form, is served.
    for timeout in [1, 60, MAX_TIMEOUT_MINUTES] {
        let run_id = start_run(
            &router,
            &json!({ "JobName": "refusals", "Timeout": timeout }),
        )
        .await;
        set_due_instant(&engine, "refusals", far_future());
        let run = get_run(&router, "refusals", &run_id).await;
        assert_eq!(
            run.get("Timeout").and_then(Value::as_i64),
            Some(i64::from(timeout)),
            "case timeout {timeout}: a timeout within the documented bounds round-trips, got {run}"
        );
        set_due_instant(&engine, "refusals", already_past());
    }
}

/// One job keeps a bounded number of runs: past the window the OLDEST record is dropped,
/// so a long-lived process holds a bounded number of runs however many are started.
#[tokio::test]
async fn a_job_keeps_a_bounded_number_of_runs() {
    let (router, engine) = emulator_with_engine();
    let name = "bounded-runs";
    ok(&router, "CreateJob", &job_body(name, json!({}))).await;

    // Reaching the window over the wire takes one request per run, which is what a
    // client would do; each run is settled so the next start is admitted.
    let mut first_ids: Vec<String> = Vec::new();
    for index in 0..MAX_RUNS_PER_JOB + 5 {
        let run_id = start_run(&router, &json!({ "JobName": name })).await;
        if index < 5 {
            first_ids.push(run_id);
        }
        set_due_instant(&engine, name, already_past());
    }

    let held = stored_runs(&engine, name);
    assert_eq!(
        held.len(),
        MAX_RUNS_PER_JOB,
        "case {} starts: a job holds at most {MAX_RUNS_PER_JOB} runs, got {}",
        MAX_RUNS_PER_JOB + 5,
        held.len()
    );
    for dropped in &first_ids {
        let (status, answer) = call(
            &router,
            "GetJobRun",
            &json!({ "JobName": name, "RunId": dropped }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {dropped:?}: the oldest runs are dropped first, got {status} {answer}"
        );
    }
    // The newest run is still there.
    let newest = held
        .last()
        .unwrap_or_else(|| panic!("the job must still hold its newest run"));
    let run = get_run(&router, name, &newest.id).await;
    assert_eq!(
        run.get("Id").and_then(Value::as_str),
        Some(newest.id.as_str()),
        "the newest run must still be reported, got {run}"
    );
}
