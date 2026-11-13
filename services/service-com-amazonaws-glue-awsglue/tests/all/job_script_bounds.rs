// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT STOPS a Glue job run's script, and so for what the run reports.
//!
//! The rule under test: the state a job run comes to rest in is decided by what its script's
//! execution DID — the work it performed and the waits it asked for — and never by how fast
//! the host that ran it happened to be. So a script that runs to its end is reported on what
//! it came to however much work it did, a script that WAITS past the span one run of a script
//! is given is the one reported in the state `Job$Timeout` documents for a run terminated for
//! taking too long, and a script that computes without end is answered by the step budget
//! this emulator gives a script, which is a count of its own work.
//!
//! Every case goes over the wire through the shared [`crate::wire`] harness, with the script
//! staged through Amazon S3's own `PutObject` and the run read back through `GetJobRun`, so
//! what is asserted is HTTP behaviour over state the data plane really holds. Nothing here
//! sleeps or reads this host's clock: the due instant a run's terminal state falls at is
//! moved through the store ([`crate::job_runs::set_due_instant`]), and the waits the scripts
//! below ask for are spans of their own run's time.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    job_run_script_bound_reached, job_run_script_ceiling_reached, JOB_RUN_STATE_FAILED,
    JOB_RUN_STATE_SUCCEEDED, JOB_RUN_STATE_TIMEOUT, JOB_SCRIPT_RUN_CEILING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_script_execution::{
    create_bucket, emulator_with_s3, run_to_rest, stage_script, ROLE_ARN, SCRIPTS,
};
use crate::wire::ok;

/// A script that exits at once, having done nothing at all: the control, which must succeed
/// whatever the host is doing.
const EXITS_AT_ONCE: &str = r#"
rows = []
"#;

/// A script that asks to wait far past the span one run of a script is given, which is what a
/// job script that never finishes does.
const WAITS_PAST_THE_SPAN: &str = r#"
import time

time.sleep(600)
print("this script was not stopped")
"#;

/// A script whose MANY SMALL waits add up past that span, so what stops it is the total of
/// the waits it asked for rather than any one of them.
const WAITS_IN_SMALL_SPANS: &str = r#"
import time

for asked in range(20):
    time.sleep(1)
print("this script was not stopped")
"#;

/// A script that waits a span well inside the one its run is given and then exits cleanly.
const WAITS_INSIDE_THE_SPAN: &str = r#"
import time

time.sleep(1)
rows = [row * row for row in range(16)]
assert sum(rows) == 1240, rows
"#;

/// A script that computes without end, which is answered by the step budget rather than by
/// any clock.
const COMPUTES_WITHOUT_END: &str = r#"
total = 0
while True:
    total += 1
"#;

/// A script that computes a great DEAL — hundreds of thousands of traced events — and then
/// exits cleanly, so a script that is far slower than a trivial one is still reported on what
/// it came to.
const COMPUTES_A_GREAT_DEAL: &str = r#"
total = 0
for row in range(200000):
    total += row
assert total == 19999900000, total
"#;

/// What a row expects of the run's `ErrorMessage`: the whole message, or none at all.
#[derive(Debug)]
enum Reason {
    /// The run carries none, which is what a run that failed at nothing reports.
    None,
    /// The whole message, exactly.
    Exact(String),
}

/// Create the job for one case, pointing at `location`, and answer nothing: the span its
/// script's run is given is this emulator's own ceiling, because the shortest `Timeout` the
/// model admits is longer than it.
async fn create_job(router: &MultiServiceRouter, name: &str, location: &str) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": {
                "Name": "pythonshell",
                "PythonVersion": "3.9",
                "ScriptLocation": location,
            },
            "MaxCapacity": 0.0625,
            "Timeout": 5,
        }),
    )
    .await;
}

/// One row per class of what a script does with the span and the budget its run is given.
#[tokio::test]
async fn what_stops_a_script_is_its_own_work_and_its_own_waits() {
    // (case, the script, the state the run must come to rest in, its ErrorMessage)
    let rows: [(&str, &str, &str, Reason); 6] = [
        (
            "a script that exits at once",
            EXITS_AT_ONCE,
            JOB_RUN_STATE_SUCCEEDED,
            Reason::None,
        ),
        (
            "a script that computes a great deal and then exits cleanly",
            COMPUTES_A_GREAT_DEAL,
            JOB_RUN_STATE_SUCCEEDED,
            Reason::None,
        ),
        (
            "a script that waits a span well inside the one its run is given",
            WAITS_INSIDE_THE_SPAN,
            JOB_RUN_STATE_SUCCEEDED,
            Reason::None,
        ),
        (
            "a script that asks to wait far past the span its run is given",
            WAITS_PAST_THE_SPAN,
            JOB_RUN_STATE_TIMEOUT,
            Reason::Exact(job_run_script_ceiling_reached()),
        ),
        (
            "a script whose many small waits add up past that span",
            WAITS_IN_SMALL_SPANS,
            JOB_RUN_STATE_TIMEOUT,
            Reason::Exact(job_run_script_ceiling_reached()),
        ),
        (
            "a script that computes without end",
            COMPUTES_WITHOUT_END,
            JOB_RUN_STATE_FAILED,
            Reason::Exact(job_run_script_bound_reached(
                "the step budget this emulator gives a job script",
            )),
        ),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, script, state, reason)) in rows.into_iter().enumerate() {
        let name = format!("script-bounds-{index}");
        let location = stage_script(&router, &format!("{SCRIPTS}{name}.py"), script).await;
        create_job(&router, &name, &location).await;

        let run = run_to_rest(&router, &engine, &name, &json!({})).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected the run to come to rest in {state}, got {run}"
        );
        let actual = run.get("ErrorMessage").and_then(Value::as_str);
        match &reason {
            Reason::None => assert!(
                actual.is_none(),
                "case {case:?}: a run that failed at nothing carries no ErrorMessage, got {run}"
            ),
            Reason::Exact(expected) => assert_eq!(
                actual,
                Some(expected.as_str()),
                "case {case:?}: expected the reason {expected:?}, got {run}"
            ),
        }
        assert!(
            run.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {case:?}: a run that has come to rest reports when it finished, got {run}"
        );
    }
}

/// The span one run of a script is given is spent by the waits that script asks for, so the
/// SAME script comes to rest in one state or the other according to how much of it it asks
/// for — one row either side of the span, read off the ceiling itself rather than written out.
#[tokio::test]
async fn a_script_is_timed_out_by_the_waits_it_asks_for_and_nothing_else() {
    let span = JOB_SCRIPT_RUN_CEILING.as_secs();
    // (case, the seconds the script asks to wait for, the state it must come to rest in)
    let rows: [(&str, u64, &str); 4] = [
        ("no wait at all", 0, JOB_RUN_STATE_SUCCEEDED),
        ("a wait of half the span", span / 2, JOB_RUN_STATE_SUCCEEDED),
        (
            "a wait of one second past the span",
            span + 1,
            JOB_RUN_STATE_TIMEOUT,
        ),
        (
            "a wait of many times the span",
            span * 100,
            JOB_RUN_STATE_TIMEOUT,
        ),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, asked, state)) in rows.into_iter().enumerate() {
        let name = format!("script-waits-{index}");
        let script = format!("import time\n\ntime.sleep({asked})\nrows = [1]\n");
        let location = stage_script(&router, &format!("{SCRIPTS}{name}.py"), &script).await;
        create_job(&router, &name, &location).await;

        let run = run_to_rest(&router, &engine, &name, &json!({})).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(state),
            "case {case:?}: a script asking to wait {asked} second(s) of the {span}-second \
             span its run is given must come to rest in {state}, got {run}"
        );
    }
}
