// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for HOW a sandbox command execution's run is REPORTED, driven over
//! the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: a command execution submitted to a sandbox whose session is live
//! is RUN in that session exactly once, and what that one run answered — the status
//! (`SUCCEEDED` for a command the session answered zero for, `FAILED` for one it did not),
//! `exitCode` ("The exit code to return upon completion"), `standardOutputContent` ("The
//! text written by the command to stdout") and `standardErrContent` ("The text written by
//! the command to stderr") — is what EVERY reader of that execution reports, however often
//! it is read and whichever operation reads it. The run is the session's and the session is
//! this process's: it resolves no command name against the host and leaves no host file
//! behind, so a command naming a host program gets what a shell answers for a command it
//! cannot run, and a write to an absolute path lands in the session's own tree.
//!
//! This is the READING half of the sandbox-command rule: what each command FORM answers,
//! the one session per sandbox and the window an execution reports are
//! `tests/all/sandbox_command_runs.rs`'s subject, and the submission itself is
//! `tests/all/command_execution_lifecycle.rs`'s.
//!
//! The router, the call and the project body are the crate's shared test harness
//! ([`super::project_build_lifecycle`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use super::project_build_lifecycle::{call, create_project_body, emulator_on, SETTLE_DEADLINE};

/// How long a poll below leaves before looking again. The deadline it runs under is the
/// shared [`SETTLE_DEADLINE`], far longer than any span this service runs on
/// ([`state::COMMAND_EXECUTION_SETTLE_MILLIS`] and [`state::SANDBOX_SETTLE_MILLIS`] are
/// sub-second): the wait ends on the CONDITION, and the deadline only keeps a condition that
/// never holds from hanging the test.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The members a reader reports a RUN execution's result in: the status the session
/// answered, the exit code it answered, the two streams, and the window it occupied. Every
/// reader of one execution must report the same value for each.
const RESULT_MEMBERS: &[&str] = &[
    "status",
    "exitCode",
    "standardOutputContent",
    "standardErrContent",
    "startTime",
    "endTime",
];

/// Create `project` and start a sandbox of it over the wire, asserting both succeeded, and
/// hand back the sandbox id.
async fn start_sandbox(router: &smithy_server_runtime::MultiServiceRouter, project: &str) -> String {
    let (status, body) = call(router, "CreateProject", create_project_body(project, json!({}))).await;
    assert_eq!(
        status, 200,
        "CreateProject {project:?}: expected 200, got {status} with {body}"
    );
    let (status, body) = call(router, "StartSandbox", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartSandbox {project:?}: expected 200, got {status} with {body}"
    );
    body["sandbox"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Submit `command` to `sandbox_id`, asserting it was accepted, and hand back its id.
async fn submit(
    router: &smithy_server_runtime::MultiServiceRouter,
    sandbox_id: &str,
    command: &str,
) -> String {
    let (status, body) = call(
        router,
        "StartCommandExecution",
        json!({"sandboxId": sandbox_id, "command": command, "type": "SHELL"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartCommandExecution {command:?}: expected 200, got {status} with {body}"
    );
    body["commandExecution"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read one command execution back through `BatchGetCommandExecutions`.
async fn batch_get(
    router: &smithy_server_runtime::MultiServiceRouter,
    sandbox_id: &str,
    id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "BatchGetCommandExecutions",
        json!({"sandboxId": sandbox_id, "commandExecutionIds": [id]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetCommandExecutions {id:?}: expected 200, got {status} with {body}"
    );
    body["commandExecutions"][0].clone()
}

/// Read one command execution back through `ListCommandExecutionsForSandbox`, which reports
/// every execution of the sandbox.
async fn list_for_sandbox(
    router: &smithy_server_runtime::MultiServiceRouter,
    sandbox_id: &str,
    id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "ListCommandExecutionsForSandbox",
        json!({"sandboxId": sandbox_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListCommandExecutionsForSandbox {sandbox_id:?}: expected 200, got {status} with \
         {body}"
    );
    body["commandExecutions"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|execution| execution["id"] == json!(id))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Submit `command` and look again until the execution has left `IN_PROGRESS`, which the
/// sandbox's session brings about on its own clock. Hands back its id and the record.
///
/// Waits for the CONDITION rather than for a span: it looks again every [`POLL_INTERVAL`]
/// until the record is terminal or [`SETTLE_DEADLINE`] has passed.
async fn run_command(
    router: &smithy_server_runtime::MultiServiceRouter,
    sandbox_id: &str,
    command: &str,
) -> (String, Value) {
    let id = submit(router, sandbox_id, command).await;
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = batch_get(router, sandbox_id, &id).await;
        if last["status"] != json!(state::COMMAND_EXECUTION_STATUS_IN_PROGRESS) {
            return (id, last);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("command {command:?} never left IN_PROGRESS, last read {last}");
}

/// What one row of the table below expects of the execution: the status it settles on, the
/// exit code it reports, and the whole of each stream (`None` where the command wrote
/// nothing there, which the member must then be absent for).
struct Expected {
    /// The `CommandExecution$status` the execution must settle on.
    status: &'static str,
    /// The `exitCode` it must report, as the decimal string the member carries.
    exit_code: &'static str,
    /// The whole of `standardOutputContent`.
    standard_output: Option<&'static str>,
    /// The whole of `standardErrContent`.
    standard_err: Option<&'static str>,
}

/// RULE: what the one run of a command answered is what EVERY reader of that execution
/// reports — the execution's status, its exit code and the text it wrote on each stream are
/// the same whether the record is read by id or listed under its sandbox.
///
/// The rows span the outcomes a reader has to carry out unchanged: a command that writes a
/// line, one that writes nothing at all, one that writes on both streams, one whose status
/// is a utility's refusal, a pipeline whose stages filter text, and an expansion with a
/// default.
#[tokio::test]
async fn every_reader_reports_what_the_one_run_answered() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let sandbox_id = start_sandbox(&router, "sandbox-run-reports").await;

    let cases: Vec<(&str, &str, Expected)> = vec![
        (
            "a command that writes a line",
            "echo reported",
            Expected {
                status: state::COMMAND_EXECUTION_STATUS_SUCCEEDED,
                exit_code: "0",
                standard_output: Some("reported\n"),
                standard_err: None,
            },
        ),
        (
            "a command that writes nothing",
            "true",
            Expected {
                status: state::COMMAND_EXECUTION_STATUS_SUCCEEDED,
                exit_code: "0",
                standard_output: None,
                standard_err: None,
            },
        ),
        (
            "a command that writes on both streams",
            "echo written\necho diagnosed 1>&2",
            Expected {
                status: state::COMMAND_EXECUTION_STATUS_SUCCEEDED,
                exit_code: "0",
                standard_output: Some("written\n"),
                standard_err: Some("diagnosed\n"),
            },
        ),
        (
            "a command whose status is a utility's refusal",
            "test 1 -eq 2",
            Expected {
                status: state::COMMAND_EXECUTION_STATUS_FAILED,
                exit_code: "1",
                standard_output: None,
                standard_err: None,
            },
        ),
        (
            "a pipeline whose stages filter text",
            "printf 'b\\na\\n' | sort",
            Expected {
                status: state::COMMAND_EXECUTION_STATUS_SUCCEEDED,
                exit_code: "0",
                standard_output: Some("a\nb\n"),
                standard_err: None,
            },
        ),
        (
            "an expansion with a default",
            "echo ${NOTHING_SET_THIS:-fallback}",
            Expected {
                status: state::COMMAND_EXECUTION_STATUS_SUCCEEDED,
                exit_code: "0",
                standard_output: Some("fallback\n"),
                standard_err: None,
            },
        ),
    ];

    for (case, command, expected) in cases {
        let (id, by_id) = run_command(&router, &sandbox_id, command).await;
        assert_eq!(
            by_id["status"],
            json!(expected.status),
            "case {case:?}: expected status {:?}, got {by_id}",
            expected.status
        );
        assert_eq!(
            by_id["exitCode"],
            json!(expected.exit_code),
            "case {case:?}: expected exitCode {:?}, got {by_id}",
            expected.exit_code
        );
        for (member, written) in [
            ("standardOutputContent", expected.standard_output),
            ("standardErrContent", expected.standard_err),
        ] {
            match written {
                Some(text) => assert_eq!(
                    by_id[member],
                    json!(text),
                    "case {case:?}: expected {member} {text:?}, got {by_id}"
                ),
                None => assert!(
                    by_id[member].is_null(),
                    "case {case:?}: the command wrote nothing for {member}, got {by_id}"
                ),
            }
        }

        // The SAME run, read the other way: the listing of the sandbox's executions
        // reports the record the id resolves to, member for member.
        let listed = list_for_sandbox(&router, &sandbox_id, &id).await;
        assert_eq!(
            listed["command"],
            json!(command),
            "case {case:?}: the listing must carry the command it ran, got {listed}"
        );
        for member in RESULT_MEMBERS {
            assert_eq!(
                listed[member], by_id[member],
                "case {case:?}: both readers must report the same {member}, got {listed} \
                 listed against {by_id} by id"
            );
        }
    }
}

/// RULE: the run is owed ONCE — reading an execution materialises the run the clock owed it,
/// so reading it again reports that same result and never runs the command a second time.
///
/// The side effect is read back out of the session ITSELF, which is what tells a run that
/// happened once from one that happened on every read: a command that APPENDS a line to a
/// file of the sandbox's own tree is read many times, and the file still holds one line.
#[tokio::test]
async fn reading_an_execution_again_reports_the_same_run_and_never_runs_it_twice() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let sandbox_id = start_sandbox(&router, "sandbox-run-once").await;

    let (id, first) = run_command(&router, &sandbox_id, "echo once >> appended.txt").await;
    assert_eq!(
        first["status"],
        json!(state::COMMAND_EXECUTION_STATUS_SUCCEEDED),
        "the appending command must succeed, got {first}"
    );

    // Read the same execution again, both ways: each read is a reporting call, so each must
    // answer out of the one run rather than bring another about.
    for look in 1..=3 {
        for (reader, again) in [
            ("BatchGetCommandExecutions", batch_get(&router, &sandbox_id, &id).await),
            (
                "ListCommandExecutionsForSandbox",
                list_for_sandbox(&router, &sandbox_id, &id).await,
            ),
        ] {
            for member in RESULT_MEMBERS {
                assert_eq!(
                    again[member], first[member],
                    "look {look} through {reader}: a later read must report the same \
                     {member}, got {again} against {first}"
                );
            }
        }
    }

    // What the session holds is what ONE run of that command left: the appended file has
    // the single line, so none of the reads above ran it again.
    let (_, counted) = run_command(&router, &sandbox_id, "cat appended.txt | wc -l").await;
    assert_eq!(
        counted["standardOutputContent"],
        json!("1\n"),
        "the command must have run exactly once, got {counted}"
    );
}

/// RULE: the run happens inside this process — the session resolves no command name against
/// the host and holds its files in its own tree, so a host program name is a command it
/// could not run and a write to an absolute path leaves no file on the host.
#[tokio::test]
async fn a_run_reaches_no_host_program_and_leaves_no_host_file() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let sandbox_id = start_sandbox(&router, "sandbox-run-contained").await;

    // (case, the command) — each names something only the HOST could provide: a program by
    // absolute path, a program of a host `PATH`, and a transfer program.
    for (case, command) in [
        ("a program named by absolute path", "/bin/sh -c 'echo escaped'"),
        ("a program of the host's PATH", "uname -a"),
        ("a transfer program", "curl https://example.invalid/"),
    ] {
        let (_, record) = run_command(&router, &sandbox_id, command).await;
        assert_eq!(
            record["status"],
            json!(state::COMMAND_EXECUTION_STATUS_FAILED),
            "case {case:?}: nothing resolves a host program, so the command must fail, got \
             {record}"
        );
        assert_eq!(
            record["exitCode"],
            json!("127"),
            "case {case:?}: expected the status of a command the search did not find, got \
             {record}"
        );
        assert!(
            record["standardOutputContent"].is_null(),
            "case {case:?}: nothing ran, so nothing was written, got {record}"
        );
    }

    // A path only the host could hold, named so that nothing but this test run could have
    // left a file there: the write goes into the session's own tree, so the session reads it
    // back and the host has no file at that path.
    let host_path = format!(
        "/tmp/codebuild-sandbox-contained-{}.marker",
        std::process::id()
    );
    let (_, written) = run_command(
        &router,
        &sandbox_id,
        &format!("echo in-the-session > {host_path}"),
    )
    .await;
    assert_eq!(
        written["status"],
        json!(state::COMMAND_EXECUTION_STATUS_SUCCEEDED),
        "the redirection writes into the session's own tree, got {written}"
    );
    let (_, read_back) = run_command(&router, &sandbox_id, &format!("cat {host_path}")).await;
    assert_eq!(
        read_back["standardOutputContent"],
        json!("in-the-session\n"),
        "the session must read its own tree back, got {read_back}"
    );
    assert!(
        !std::path::Path::new(&host_path).exists(),
        "the run must leave no host file, yet {host_path} exists"
    );
}
