// SPDX-License-Identifier: CC0-1.0

//! End-to-end and seam tests for WHICH BOUNDS decide what a job run reports, and therefore
//! for what a run whose script really finished reports.
//!
//! The rule under test, in one sentence: a run's terminal state is what its own execution
//! came to, so every bound that decides it is a count of the RUN's own doings — the traced
//! events it delivers and the waits it asks for — while the spans of the HOST's own time on
//! the path bound what the emulator spends and report no outcome of the script at all.
//!
//! What goes wrong when it does not: a bound read against this host's own clock runs out while
//! the interpreter is still being stood up, or while the script is still working, on a busy
//! host — and the run is then reported in the state `Job$Timeout` documents for a script that
//! ran past nothing. That is an outcome no execution produced, and whether it happens is
//! decided by how loaded the host is rather than by the script.
//!
//! The first test pins those bounds against each other over the `Timeout` values a job may
//! carry; the second drives the rule end to end, where a script whose MODULES are imported
//! before it runs — work the engine does in the phase it bounds on its own span — still
//! reports what the script itself came to.

use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_glue_awsglue::state::{
    job_script_budget, JOB_RUN_STATE_SUCCEEDED, JOB_SCRIPT_CALL_CEILING, JOB_SCRIPT_CLOCK_TICK,
    JOB_SCRIPT_HOST_CEILING, JOB_SCRIPT_HOST_WAIT, JOB_SCRIPT_RUN_CEILING, MAX_JOB_SCRIPT_STEPS,
    MAX_TIMEOUT_MINUTES,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::INIT_PHASE_LIMIT;
use serde_json::json;

use crate::job_script_execution::{
    create_bucket, emulator_with_s3, run_to_rest, stage_script, ROLE_ARN, SCRIPTS,
};
use crate::wire::ok;

/// The `Timeout` values the bounds are pinned over: the shortest the model admits
/// (`Timeout`'s `@range(min: 1)`), one in between, the longest it documents
/// ([`MAX_TIMEOUT_MINUTES`], the seven days `Job$Timeout` admits), and one outside that range
/// altogether, which bounds nothing and so leaves the emulator's own ceiling in charge.
const TIMEOUT_ROWS: &[(&str, i32)] = &[
    ("the shortest Timeout the model admits", 1),
    ("an hour", 60),
    ("the longest Timeout the model documents", MAX_TIMEOUT_MINUTES),
    ("a Timeout outside the range the model allows", 0),
];

#[test]
fn what_decides_a_runs_outcome_is_the_runs_own_doings_and_not_the_hosts_clock() {
    for (case, minutes) in TIMEOUT_ROWS {
        // The span ONE run of a script is given, which is a span of the run's own time.
        let span = job_script_budget(*minutes).wall_clock;
        assert!(
            span > Duration::ZERO && span <= JOB_SCRIPT_RUN_CEILING,
            "case {case:?}: no stored Timeout may set the span one run of a script is given — \
             it is this emulator's own ceiling ({JOB_SCRIPT_RUN_CEILING:?}) at most — and it \
             is {span:?}"
        );
        // Every traced event the script delivers moves the run's own clock by one tick, so
        // the whole step budget has to spend less of that clock than the span: that is what
        // leaves a script that computes without end to be answered by its step budget — a
        // count of its own work — and leaves the span itself for the waits a script asks for.
        let spent_by_the_step_budget = JOB_SCRIPT_CLOCK_TICK
            .saturating_mul(u32::try_from(MAX_JOB_SCRIPT_STEPS).unwrap_or(u32::MAX));
        assert!(
            spent_by_the_step_budget < span,
            "case {case:?}: the whole step budget ({MAX_JOB_SCRIPT_STEPS} events at \
             {JOB_SCRIPT_CLOCK_TICK:?} each = {spent_by_the_step_budget:?}) must spend less of \
             the run's own clock than the {span:?} span it is given"
        );
        // The HOST's own time one run may take, which the engine stops the run on: it covers
        // the whole span the ENGINE may take in the run's own time — the phase it stands the
        // execution environment up in, which it bounds on its own span, and then the script's
        // own — so a script that finished is never stopped before it has answered, however
        // much slower this host is than the span suggests.
        let engine_span = INIT_PHASE_LIMIT.saturating_add(span);
        assert!(
            JOB_SCRIPT_HOST_CEILING > engine_span,
            "case {case:?}: the host time one run may take ({JOB_SCRIPT_HOST_CEILING:?}) must \
             cover the engine's own span for the budget ({engine_span:?} = \
             {INIT_PHASE_LIMIT:?} standing the environment up plus {span:?} of script) before \
             a run is stopped on it"
        );
        // The call waits for that ceiling AND for the engine's own stop to land, so a thread
        // the call gives up on is one the engine has already stopped.
        assert!(
            JOB_SCRIPT_HOST_WAIT > JOB_SCRIPT_HOST_CEILING,
            "case {case:?}: the call's wait ({JOB_SCRIPT_HOST_WAIT:?}) must outlast the host \
             time one run may take ({JOB_SCRIPT_HOST_CEILING:?}), so the engine's own stop \
             lands before the call stops waiting"
        );
        // And the whole of what one request can be held for stays a small multiple of that
        // span, so a call is answered rather than held open for the life of the process. The
        // multiple is this test's own bound on how generous those spans may become, not the
        // source's.
        let longest_a_request_may_be_held = JOB_SCRIPT_CALL_CEILING.saturating_add(JOB_SCRIPT_HOST_WAIT);
        let bound_on_holding_a_request = engine_span.saturating_mul(10);
        assert!(
            longest_a_request_may_be_held <= bound_on_holding_a_request,
            "case {case:?}: the longest one request can be held \
             ({longest_a_request_may_be_held:?} = {JOB_SCRIPT_CALL_CEILING:?} of starting \
             scripts plus {JOB_SCRIPT_HOST_WAIT:?} waiting for the last) must stay inside ten \
             times the engine's own span ({bound_on_holding_a_request:?})"
        );
    }
}

/// A script whose modules are imported before a line of it runs — the phase the engine
/// stands the environment up in — and which then exits cleanly, having computed something.
///
/// The imports are standard-library modules the engine serves from its own frozen library,
/// so what they cost is the HOST's work of building the environment rather than the script's.
const IMPORTS_THEN_EXITS_CLEANLY: &str = r#"
import base64
import datetime
import decimal
import hashlib
import json
import re
import uuid

digest = hashlib.sha256(b"partition").hexdigest()
encoded = base64.b64encode(digest.encode("utf-8")).decode("ascii")
parsed = json.loads(json.dumps({"rows": [decimal.Decimal(1).as_tuple().exponent]}))
matched = re.findall(r"[0-9a-f]+", digest)
assert parsed["rows"] == [0], parsed
assert matched, digest
assert isinstance(uuid.uuid4().hex, str)
assert datetime.datetime(2026, 1, 1).year == 2026
"#;

#[tokio::test]
async fn a_run_whose_script_finished_is_not_reported_as_terminated_for_taking_too_long() {
    let case = "a script whose modules are imported before it runs";
    let name = "script-imports-then-exits";
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}{name}.py"),
        IMPORTS_THEN_EXITS_CLEANLY,
    )
    .await;
    ok(
        &router,
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
            // The shortest Timeout the model admits, so the span the script itself is given
            // is this emulator's own ceiling and nothing longer.
            "Timeout": 1,
        }),
    )
    .await;

    let run = run_to_rest(&router, &engine, name, &json!({})).await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: the script exited cleanly, so the run must come to rest in \
         {JOB_RUN_STATE_SUCCEEDED} rather than in a state no execution produced, got {run}"
    );
    assert!(
        run.get("ErrorMessage").is_none(),
        "case {case:?}: a run whose script failed at nothing carries no ErrorMessage, got {run}"
    );
    // And it really ran: a run that came to rest reports when it completed.
    assert!(
        run.get("CompletedOn").is_some(),
        "case {case:?}: a run that came to rest reports its CompletedOn, got {run}"
    );
}
