// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a Glue job run EXECUTES: the job's own script, read out of the
//! emulator's own Amazon S3 and run with the run's arguments on its command line, reported
//! back through `GetJobRun` and `GetJobRuns`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the scripts
//! the runs execute are staged through Amazon S3's own `PutObject`, so what is asserted is
//! HTTP behaviour over state the data plane really holds.
//!
//! The rule under test: a run of a job is ONE execution of the script its
//! `JobCommand$ScriptLocation` names, with the run's `Arguments` on that script's command
//! line — so the terminal `JobRunState` is what that execution came to (`SUCCEEDED` for a
//! script that exited cleanly, with `CompletedOn` set and NO `ErrorMessage`; the modelled
//! failure state carrying the SCRIPT's own error for one that raised; the `TIMEOUT` state
//! for one terminated for taking too long), and the script's own effects — an object it
//! writes with `boto3` — are really made.
//!
//! The recorded case — a script that writes a token object under the key its `--key` and
//! `--token` arguments name, run by a `glue:startJobRun.sync` task — is ONE row of the table
//! below; the rows differ along what the script does (exits cleanly, writes an object,
//! resolves its arguments through `awsglue.utils`, raises, fails to compile, reaches for the
//! host, runs past its wall clock), how the script is reachable at all (staged, missing, not
//! named, in a language no engine here runs) and what the run therefore comes to rest in.
//!
//! The due instant a run's terminal state falls at is moved THROUGH THE STORE rather than
//! slept for, and every run is read back until it has come to rest
//! ([`crate::job_runs::run_at_rest`]) — the execution a run is owed is performed by
//! whichever call finds it owed — so every transition here is deterministic and no outcome
//! depends on how fast the host runs.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    job_run_language_not_runnable, job_run_script_ceiling_reached, JOB_RUN_NO_SCRIPT_LOCATION,
    JOB_RUN_SCRIPT_UNREADABLE_LEAD, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_SUCCEEDED,
    JOB_RUN_STATE_TIMEOUT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{run_at_rest, runs_at_rest};
use crate::wire::{emulator_on, ok};

/// The role a job runs as: "the name or Amazon Resource Name (ARN) of the IAM role
/// associated with this job" (`CreateJob$Role`).
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The bucket the scripts and everything they write live in.
pub(crate) const BUCKET: &str = "glue-job-script-suite";

/// The prefix the staged scripts live under.
pub(crate) const SCRIPTS: &str = "scripts/";

/// An emulator with Glue AND Amazon S3 mounted on one shared engine, plus the engine itself
/// so a test can move the due instant a run's terminal state falls at.
///
/// Amazon S3 is mounted because a job run READS its script from it and a script WRITES
/// through it: both travel over the emulator's own in-process callback, so the object a test
/// stages is the script that runs, and the object the script writes is one a client can get.
pub(crate) fn emulator_with_s3() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(Arc::clone(&engine));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a run reads
/// its script through, so what a test stages here is what the run executes. The shared
/// harness owns the one client configuration ([`crate::wire::s3_client`]).
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    crate::wire::s3_client(router)
}

/// Create the suite's bucket.
pub(crate) async fn create_bucket(router: &MultiServiceRouter) {
    s3(router)
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {BUCKET} must be created, got {error:?}"));
}

/// Stage `source` as the script object `key`, and answer the `s3://` location that names it.
pub(crate) async fn stage_script(router: &MultiServiceRouter, key: &str, source: &str) -> String {
    s3(router)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(
            source.as_bytes().to_vec(),
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("the script {key} must be stored, got {error:?}"));
    format!("s3://{BUCKET}/{key}")
}

/// The body of one object, or `None` when the object store does not serve it.
pub(crate) async fn object_body(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let object = s3(router)
        .get_object()
        .bucket(BUCKET)
        .key(key)
        .send()
        .await
        .ok()?;
    let bytes = object.body.collect().await.ok()?.into_bytes();
    String::from_utf8(bytes.to_vec()).ok()
}

/// Start a run of `job_name` with `arguments` and answer the `JobRun` `GetJobRun` reports
/// once that run has come to rest.
///
/// The run's script is executed by whichever call finds that execution owed — the start, or
/// a later read when the start found no free interpreter — and the state it came to falls
/// due one owed span after it answered. So the run is READ BACK UNTIL it has come to rest
/// ([`crate::job_runs::run_at_rest`]), with the owed span made due through the store on
/// every turn rather than slept out: what the run reports then is what its own execution
/// came to, however busy the host was while it ran.
pub(crate) async fn run_to_rest(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    job_name: &str,
    arguments: &Value,
) -> Value {
    let mut start = json!({ "JobName": job_name });
    if let Some(members) = arguments.as_object() {
        if !members.is_empty() {
            start["Arguments"] = arguments.clone();
        }
    }
    let started = ok(router, "StartJobRun", &start).await;
    let run_id = started
        .get("JobRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {started}"))
        .to_string();
    run_at_rest(router, engine, job_name, job_name, &run_id).await
}

/// What a row expects of the run's `ErrorMessage`.
#[derive(Debug)]
enum Reason {
    /// The run carries none at all, which is what a run that failed at nothing reports.
    None,
    /// The whole message, exactly.
    Exact(String),
    /// The message begins with this — for a reason whose tail is the interpreter's or the
    /// object store's own detail.
    StartsWith(String),
}

/// One row of the table: a job whose script does something, and what the run of it must come
/// to rest in.
#[derive(Debug)]
struct Row {
    /// What makes the row recognisable in an assertion.
    case: &'static str,
    /// The job (and script object) name.
    name: &'static str,
    /// The script's source, or `None` for a row that stages none.
    script: Option<&'static str>,
    /// Whether the script object is really staged — `false` stages nothing, so the job
    /// points at an object the store does not serve.
    staged: bool,
    /// Whether the job's `Command` names a `ScriptLocation` at all.
    names_script: bool,
    /// `DefaultArguments` of the job, beyond the ones the start passes.
    default_arguments: Value,
    /// The `JobRunState` the run must come to rest in.
    state: &'static str,
    /// What its `ErrorMessage` must be.
    reason: Reason,
    /// Whether the token object the script writes must be present afterwards.
    writes_token: bool,
}

impl Row {
    /// A row whose script is staged, with no default arguments, expected to succeed.
    fn new(case: &'static str, name: &'static str, script: &'static str) -> Self {
        Self {
            case,
            name,
            script: Some(script),
            staged: true,
            names_script: true,
            default_arguments: json!({}),
            state: JOB_RUN_STATE_SUCCEEDED,
            reason: Reason::None,
            writes_token: false,
        }
    }

    /// The same row, expected to come to rest in `state` carrying `reason`.
    fn failing(mut self, state: &'static str, reason: Reason) -> Self {
        self.state = state;
        self.reason = reason;
        self
    }

    /// The key of this row's script object.
    fn script_key(&self) -> String {
        format!("{SCRIPTS}{}.py", self.name)
    }

    /// The key the row's script writes its token under.
    fn token_key(&self) -> String {
        format!("tokens/{}.txt", self.name)
    }

    /// The token the row's run passes, which is also the body the object must carry.
    fn token(&self) -> String {
        format!("token-for-{}", self.name)
    }
}

/// A script that writes the token its arguments name, reading them straight off the command
/// line the way any Python program reads its own arguments.
const WRITES_FROM_ARGV: &str = r#"
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
boto3.client("s3").put_object(
    Bucket=named["--bucket"],
    Key=named["--key"],
    Body=named["--token"].encode("utf-8"),
)
"#;

/// The same script, resolving its arguments the way an AWS Glue job script does.
const WRITES_FROM_RESOLVED_OPTIONS: &str = r#"
import sys

import boto3
from awsglue.utils import getResolvedOptions

args = getResolvedOptions(sys.argv, ["bucket", "key", "token"])
boto3.client("s3").put_object(
    Bucket=args["bucket"],
    Key=args["key"],
    Body=args["token"].encode("utf-8"),
)
"#;

/// A script that exits cleanly having computed something and written nothing.
const EXITS_CLEANLY: &str = r#"
rows = [row for row in range(10) if row % 2 == 0]
total = sum(rows)
assert total == 20, total
"#;

/// A script that runs under its `__main__` guard, as a program does.
const RUNS_ITS_MAIN_GUARD: &str = r#"
import sys

import boto3


def main():
    named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
    boto3.client("s3").put_object(
        Bucket=named["--bucket"],
        Key=named["--key"],
        Body=named["--token"].encode("utf-8"),
    )


if __name__ == "__main__":
    main()
"#;

/// A script that raises, which is what a failing Glue job does.
const RAISES: &str = r#"
raise ValueError("the input partition is malformed")
"#;

/// A script that raises something else, so the reason the run reports is the exception's own
/// type and message rather than one fixed sentence.
const RAISES_KEY_ERROR: &str = r#"
rows = {"a": 1}
rows["b"]
"#;

/// A script that does not compile at all.
const DOES_NOT_COMPILE: &str = r#"
def broken(:
    return 1
"#;

/// A script that tries to reach the HOST's filesystem, which the interpreter refuses — so
/// the run fails on the script's own exception and nothing of the host is read.
const REACHES_FOR_THE_HOST: &str = r#"
with open("/etc/passwd") as reading:
    print(reading.read())
"#;

/// A script that never finishes inside the wall clock a script is given.
const NEVER_FINISHES: &str = r#"
import time

time.sleep(600)
print("this script was not stopped")
"#;

/// The `CreateJob` body for one row.
fn job_body(row: &Row, location: Option<String>, language: Option<&str>) -> Value {
    let mut command = json!({ "Name": "pythonshell", "PythonVersion": "3.9" });
    if let Some(location) = location {
        command["ScriptLocation"] = json!(location);
    }
    let mut defaults = row.default_arguments.clone();
    if let Some(language) = language {
        defaults["--job-language"] = json!(language);
    }
    let mut body = json!({
        "Name": row.name,
        "Role": ROLE_ARN,
        "Command": command,
        "MaxCapacity": 0.0625,
        "Timeout": 5,
    });
    if defaults.as_object().is_some_and(|members| !members.is_empty()) {
        body["DefaultArguments"] = defaults;
    }
    body
}

/// Assert `run`'s `ErrorMessage` against what the row expects.
fn assert_reason(case: &str, reason: &Reason, run: &Value) {
    let actual = run.get("ErrorMessage").and_then(Value::as_str);
    match reason {
        Reason::None => assert!(
            actual.is_none(),
            "case {case:?}: a run that failed at nothing carries no ErrorMessage, got {run}"
        ),
        Reason::Exact(expected) => assert_eq!(
            actual,
            Some(expected.as_str()),
            "case {case:?}: expected the reason {expected:?}, got {run}"
        ),
        Reason::StartsWith(lead) => assert!(
            actual.is_some_and(|message| message.starts_with(lead.as_str())),
            "case {case:?}: expected a reason beginning {lead:?}, got {run}"
        ),
    }
}

/// The terminal `JobRunState` a run reaches is what its SCRIPT's execution came to, and the
/// script's own effects are really made.
///
/// One row per class of script and of script reachability; the recorded case (a script that
/// writes the token its `--key`/`--token` arguments name) is the second row.
#[tokio::test]
async fn a_runs_terminal_state_is_what_its_script_came_to() {
    let rows: Vec<Row> = vec![
        Row::new("a script that exits cleanly", "script-clean", EXITS_CLEANLY),
        Row {
            writes_token: true,
            ..Row::new(
                "a script that writes its token object, reading its own command line",
                "script-writes-argv",
                WRITES_FROM_ARGV,
            )
        },
        Row {
            writes_token: true,
            ..Row::new(
                "a script that resolves its arguments through awsglue.utils",
                "script-writes-resolved",
                WRITES_FROM_RESOLVED_OPTIONS,
            )
        },
        Row {
            writes_token: true,
            ..Row::new(
                "a script that does its work under its __main__ guard",
                "script-main-guard",
                RUNS_ITS_MAIN_GUARD,
            )
        },
        Row::new("a script that raises", "script-raises", RAISES).failing(
            JOB_RUN_STATE_FAILED,
            Reason::Exact("ValueError: the input partition is malformed".to_string()),
        ),
        Row::new(
            "a script that raises a different exception",
            "script-raises-key-error",
            RAISES_KEY_ERROR,
        )
        .failing(
            JOB_RUN_STATE_FAILED,
            Reason::StartsWith("KeyError".to_string()),
        ),
        Row::new(
            "a script that does not compile",
            "script-uncompilable",
            DOES_NOT_COMPILE,
        )
        .failing(
            JOB_RUN_STATE_FAILED,
            Reason::StartsWith("SyntaxError".to_string()),
        ),
        Row::new(
            "a script that reaches for a host file",
            "script-host-file",
            REACHES_FOR_THE_HOST,
        )
        .failing(
            JOB_RUN_STATE_FAILED,
            Reason::StartsWith("OSError".to_string()),
        ),
        Row::new(
            "a script that never finishes",
            "script-never-finishes",
            NEVER_FINISHES,
        )
        .failing(
            JOB_RUN_STATE_TIMEOUT,
            Reason::Exact(job_run_script_ceiling_reached()),
        ),
        Row {
            staged: false,
            ..Row::new(
                "a job pointing at a script the object store does not serve",
                "script-missing",
                EXITS_CLEANLY,
            )
        }
        .failing(
            JOB_RUN_STATE_FAILED,
            Reason::StartsWith(JOB_RUN_SCRIPT_UNREADABLE_LEAD.to_string()),
        ),
        Row {
            names_script: false,
            ..Row::new(
                "a job whose Command names no script at all",
                "script-unnamed",
                EXITS_CLEANLY,
            )
        }
        .failing(
            JOB_RUN_STATE_FAILED,
            Reason::Exact(JOB_RUN_NO_SCRIPT_LOCATION.to_string()),
        ),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for row in &rows {
        let location = match (row.script, row.staged, row.names_script) {
            (_, _, false) => None,
            (Some(source), true, true) => Some(stage_script(&router, &row.script_key(), source).await),
            // Nothing is staged: the location names an object the store does not serve.
            (_, false, true) => Some(format!("s3://{BUCKET}/{}", row.script_key())),
            (None, _, true) => None,
        };
        ok(&router, "CreateJob", &job_body(row, location, None)).await;

        let run = run_to_rest(
            &router,
            &engine,
            row.name,
            &json!({
                "--bucket": BUCKET,
                "--key": row.token_key(),
                "--token": row.token(),
            }),
        )
        .await;

        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(row.state),
            "case {:?}: expected the run to come to rest in {}, got {run}",
            row.case,
            row.state
        );
        assert_reason(row.case, &row.reason, &run);
        assert!(
            run.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {:?}: a run that has come to rest reports when it finished, got {run}",
            row.case
        );
        assert!(
            run.get("ExecutionTime").and_then(Value::as_i64).is_some(),
            "case {:?}: a finished run reports the seconds it consumed, got {run}",
            row.case
        );

        // The script's own effect: present exactly when the script that makes it ran to its
        // end, and absent for every row whose script never wrote one.
        let written = object_body(&router, &row.token_key()).await;
        if row.writes_token {
            assert_eq!(
                written.as_deref(),
                Some(row.token().as_str()),
                "case {:?}: the object the script wrote must carry the token it was passed, \
                 got {written:?}",
                row.case
            );
        } else {
            assert!(
                written.is_none(),
                "case {:?}: no object may appear for a script that writes none, got \
                 {written:?}",
                row.case
            );
        }
    }
}

/// A job whose script is written in a language no engine of this emulator runs is not
/// reported as having run: the run comes to rest in the modelled failure state saying so.
///
/// The rows differ along how the language is named — `Job$DefaultArguments` and the start's
/// own `Arguments` — and along the language itself, including the value AWS documents as the
/// default (`python`), which IS run.
#[tokio::test]
async fn a_script_in_a_language_no_engine_runs_is_not_reported_as_having_run() {
    // (case, the language the job names, the state the run must come to rest in)
    let rows: [(&str, Option<&str>, &str); 4] = [
        ("no language at all, which defaults to python", None, JOB_RUN_STATE_SUCCEEDED),
        ("python, named outright", Some("python"), JOB_RUN_STATE_SUCCEEDED),
        ("scala", Some("scala"), JOB_RUN_STATE_FAILED),
        ("a language AWS does not document", Some("ruby"), JOB_RUN_STATE_FAILED),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, language, state)) in rows.into_iter().enumerate() {
        let name = format!("script-language-{index}");
        let key = format!("{SCRIPTS}{name}.py");
        let location = stage_script(&router, &key, EXITS_CLEANLY).await;
        let row = Row {
            case,
            name: "",
            script: Some(EXITS_CLEANLY),
            staged: true,
            names_script: true,
            default_arguments: json!({}),
            state,
            reason: Reason::None,
            writes_token: false,
        };
        let mut body = job_body(&row, Some(location), language);
        body["Name"] = json!(name);
        ok(&router, "CreateJob", &body).await;

        let run = run_to_rest(&router, &engine, &name, &json!({})).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected the run to come to rest in {state}, got {run}"
        );
        match language.filter(|language| *language != "python") {
            Some(language) => assert_reason(
                case,
                &Reason::Exact(job_run_language_not_runnable(language)),
                &run,
            ),
            None => assert_reason(case, &Reason::None, &run),
        }
    }
}

/// A run whose script exited cleanly is not retried, and a run whose script failed is
/// retried once per unit of the job's retry budget — each retry being a further EXECUTION of
/// that script, which is what makes its effects happen again.
///
/// The rows differ along the retry budget and along whether the script succeeds or fails.
#[tokio::test]
async fn the_retry_budget_buys_a_further_execution_only_for_a_failed_run() {
    // A script that appends one object per execution, so the number of executions a chain
    // performed is readable from the object store rather than from the run records alone.
    const COUNTS_ITS_EXECUTIONS: &str = r#"
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
client = boto3.client("s3")
listed = client.list_objects_v2(Bucket=named["--bucket"], Prefix=named["--key"])
already = listed.get("KeyCount") or 0
client.put_object(
    Bucket=named["--bucket"],
    Key="%s%d" % (named["--key"], already),
    Body=named["--token"].encode("utf-8"),
)
raise RuntimeError("this attempt failed after writing its object")
"#;

    // (case, MaxRetries, the script, the attempts the chain must hold)
    let rows: [(&str, i64, &str, usize); 4] = [
        ("a clean script with no retry budget", 0, EXITS_CLEANLY, 1),
        ("a clean script with a retry budget", 3, EXITS_CLEANLY, 1),
        ("a failing script with no retry budget", 0, COUNTS_ITS_EXECUTIONS, 1),
        ("a failing script with a budget of two", 2, COUNTS_ITS_EXECUTIONS, 3),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, max_retries, script, attempts)) in rows.into_iter().enumerate() {
        let name = format!("script-retries-{index}");
        let key = format!("{SCRIPTS}{name}.py");
        let location = stage_script(&router, &key, script).await;
        let prefix = format!("runs/{name}/");
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
                "Timeout": 5,
                "MaxRetries": max_retries,
                "ExecutionProperty": { "MaxConcurrentRuns": 10 },
            }),
        )
        .await;

        ok(
            &router,
            "StartJobRun",
            &json!({
                "JobName": name,
                "Arguments": {
                    "--bucket": BUCKET,
                    "--key": prefix,
                    "--token": "attempt",
                },
            }),
        )
        .await;

        // Each attempt is a further EXECUTION the service performs once the attempt it
        // retries has come to rest, so the chain advances one attempt per owed span: the
        // span is made due rather than slept for and the runs are read back until no run of
        // the job is in flight any more, which is what a client polling them observes.
        let runs = runs_at_rest(&router, &engine, case, &name).await;
        assert_eq!(
            runs.len(),
            attempts,
            "case {case:?}: expected {attempts} attempt(s), got {runs:?}"
        );

        // One object per execution the chain really performed: a retry re-runs the script,
        // so the effects of each attempt are there, and a run nothing retried has one.
        let written = s3(&router)
            .list_objects_v2()
            .bucket(BUCKET)
            .prefix(&prefix)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing must answer, got {error:?}"));
        let expected_objects = if script == EXITS_CLEANLY { 0 } else { attempts };
        let actual_objects = written.key_count().unwrap_or_default();
        assert_eq!(
            usize::try_from(actual_objects).unwrap_or_default(),
            expected_objects,
            "case {case:?}: expected {expected_objects} object(s) from {attempts} execution(s), \
             got {actual_objects}"
        );
    }
}

/// A script reaches nothing of the HOST: each attempt raises inside the script, so the run
/// fails on the script's own exception and no host socket, process, library or file is
/// reached from a Glue job run.
///
/// The rows are one per primitive a script could try to reach the host with, and each is
/// attempted FROM the script. The control is the row that writes an object instead, which
/// must succeed — so the table cannot pass by every script failing for some other reason.
#[tokio::test]
async fn a_script_reaches_nothing_of_the_host() {
    // (case, the script, whether it must come to rest having succeeded)
    let rows: [(&str, &str, bool); 7] = [
        (
            "connecting a socket to a host",
            "import socket\nsocket.socket().connect((\"example.com\", 80))\n",
            false,
        ),
        (
            "starting a host process",
            "import subprocess\nsubprocess.run([\"/bin/ls\"])\n",
            false,
        ),
        (
            "loading a host library",
            "import ctypes\nctypes.CDLL(None)\n",
            false,
        ),
        (
            "starting a guest thread",
            "import _thread\n_thread.start_new_thread(lambda: None, ())\n",
            false,
        ),
        (
            "clearing the trace hook the bounds are spent through",
            "import sys\nsys.settrace(None)\n",
            false,
        ),
        (
            "raising the recursion limit",
            "import sys\nsys.setrecursionlimit(10 ** 6)\n",
            false,
        ),
        // The control: a script that reaches only this emulator does its work.
        ("writing an object instead", WRITES_FROM_ARGV, true),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, script, succeeds)) in rows.into_iter().enumerate() {
        let name = format!("script-contained-{index}");
        let key = format!("{SCRIPTS}{name}.py");
        let location = stage_script(&router, &key, script).await;
        let token_key = format!("contained/{name}.txt");
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
                "Timeout": 5,
            }),
        )
        .await;

        let run = run_to_rest(
            &router,
            &engine,
            &name,
            &json!({
                "--bucket": BUCKET,
                "--key": token_key,
                "--token": "reached",
            }),
        )
        .await;
        let expected = if succeeds {
            JOB_RUN_STATE_SUCCEEDED
        } else {
            JOB_RUN_STATE_FAILED
        };
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the run to come to rest in {expected}, got {run}"
        );
        if !succeeds {
            assert!(
                run.get("ErrorMessage").and_then(Value::as_str).is_some(),
                "case {case:?}: the run must carry the exception the script raised, got {run}"
            );
            assert_eq!(
                object_body(&router, &token_key).await,
                None,
                "case {case:?}: a script that could not reach the host wrote nothing"
            );
        }
    }
}

/// A run's script is executed EXACTLY ONCE, however many times the run is read.
///
/// The script appends one object per execution, so the number of executions is readable from
/// the object store: a read of a job's runs performs the execution a run was owed, and never
/// a second one. The rows differ along which read is made and how many times.
#[tokio::test]
async fn a_runs_script_is_executed_exactly_once_however_often_it_is_read() {
    // A script that writes one object per execution, under a key no two executions share.
    const APPENDS_ONE_OBJECT: &str = r#"
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
client = boto3.client("s3")
listed = client.list_objects_v2(Bucket=named["--bucket"], Prefix=named["--key"])
client.put_object(
    Bucket=named["--bucket"],
    Key="%s%d" % (named["--key"], listed.get("KeyCount") or 0),
    Body=named["--token"].encode("utf-8"),
)
"#;

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    let name = "script-executed-once";
    let location = stage_script(&router, &format!("{SCRIPTS}{name}.py"), APPENDS_ONE_OBJECT).await;
    let prefix = format!("once/{name}/");
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
            "Timeout": 5,
        }),
    )
    .await;

    let run = run_to_rest(
        &router,
        &engine,
        name,
        &json!({
            "--bucket": BUCKET,
            "--key": prefix,
            "--token": "once",
        }),
    )
    .await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "the script exits cleanly, so the run succeeds, got {run}"
    );
    let run_id = run
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the run reports its Id, got {run}"))
        .to_string();

    // Every read of the run, one after another: none of them runs the script again.
    for (case, body) in [
        ("GetJobRun", json!({ "JobName": name, "RunId": run_id })),
        ("GetJobRuns", json!({ "JobName": name })),
        ("GetJobRun again", json!({ "JobName": name, "RunId": run_id })),
        ("GetJobRuns again", json!({ "JobName": name })),
    ] {
        let operation = case.split_whitespace().next().unwrap_or(case);
        ok(&router, operation, &body).await;
        let listed = s3(&router)
            .list_objects_v2()
            .bucket(BUCKET)
            .prefix(&prefix)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing must answer, got {error:?}"));
        let written = listed.key_count().unwrap_or_default();
        assert_eq!(
            written, 1,
            "case {case:?}: the script must have run exactly once, got {written} object(s)"
        );
    }
}
///
/// The rows differ along which side names the value: the job alone, the start alone, and
/// both — where the start wins, as `StartJobRun$Arguments` documents ("For this job run,
/// they replace the default arguments set in the job definition itself").
#[tokio::test]
async fn the_script_reads_the_arguments_the_run_resolved() {
    // A script that writes every argument it was given, so what reached its command line is
    // readable from the object store.
    const WRITES_ITS_ARGUMENTS: &str = r#"
import json
import sys

import boto3

named = dict(zip(sys.argv[1::2], sys.argv[2::2]))
boto3.client("s3").put_object(
    Bucket=named["--bucket"],
    Key=named["--key"],
    Body=json.dumps(named, sort_keys=True).encode("utf-8"),
)
"#;

    // (case, DefaultArguments, Arguments of the start, the value --scale must have)
    let rows: [(&str, Value, Value, &str); 3] = [
        (
            "the job names it and the start does not",
            json!({ "--scale": "3" }),
            json!({}),
            "3",
        ),
        (
            "the start names it and the job does not",
            json!({}),
            json!({ "--scale": "7" }),
            "7",
        ),
        (
            "both name it, so the start replaces the default",
            json!({ "--scale": "3" }),
            json!({ "--scale": "11" }),
            "11",
        ),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, defaults, arguments, expected)) in rows.into_iter().enumerate() {
        let name = format!("script-arguments-{index}");
        let key = format!("{SCRIPTS}{name}.py");
        let location = stage_script(&router, &key, WRITES_ITS_ARGUMENTS).await;
        let token_key = format!("arguments/{name}.json");
        let mut body = json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": {
                "Name": "pythonshell",
                "PythonVersion": "3.9",
                "ScriptLocation": location,
            },
            "MaxCapacity": 0.0625,
            "Timeout": 5,
        });
        let mut all_defaults = defaults.clone();
        all_defaults["--bucket"] = json!(BUCKET);
        all_defaults["--key"] = json!(token_key);
        body["DefaultArguments"] = all_defaults;
        ok(&router, "CreateJob", &body).await;

        let run = run_to_rest(&router, &engine, &name, &arguments).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_SUCCEEDED),
            "case {case:?}: the script exits cleanly, so the run succeeds, got {run}"
        );

        let written = object_body(&router, &token_key)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the script must have written its object"));
        let read: Value = serde_json::from_str(&written)
            .unwrap_or_else(|_| panic!("case {case:?}: the script writes JSON, got {written:?}"));
        assert_eq!(
            read.get("--scale").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: the script must read --scale {expected:?}, got {read}"
        );
        // And the run reports the same resolved set the script read.
        assert_eq!(
            run.get("Arguments").and_then(Value::as_object).and_then(|arguments| {
                arguments.get("--scale").and_then(Value::as_str)
            }),
            Some(expected),
            "case {case:?}: the run must report the arguments it resolved, got {run}"
        );
    }
}
