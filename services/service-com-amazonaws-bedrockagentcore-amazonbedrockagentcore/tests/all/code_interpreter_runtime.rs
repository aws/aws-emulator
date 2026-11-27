// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the code interpreter's RUNTIME surface: the Python half of
//! `executeCode`, the command plane (`executeCommand`, `startCommandExecution`,
//! `getTask`, `stopTask`), and how a tool-level failure is reported.
//!
//! The rules under test:
//!
//! * `executeCode` with `language: python` RUNS the submitted program — the tables below
//!   assert values no handler could hold as literals, which is what distinguishes a real
//!   interpreter from a canned answer — and what it bound stays in the session's
//!   namespace for its later runs unless `clearContext` asks for it to be dropped;
//! * `executeCommand` PERFORMS the command line in the session's own workspace and
//!   reports `stdout`, `stderr` and `exitCode` — 0 for a command that succeeded and
//!   non-zero for one that failed;
//! * the file tools, a run and a command are three views of ONE filesystem, and that
//!   filesystem is the session's: nothing of another session's is reachable from it;
//! * `startCommandExecution` mints a task with a non-terminal status, `getTask`
//!   converges it to a terminal one, and `stopTask` cancels one that has not converged;
//! * a failure of the invoked TOOL — a path that does not address the session workspace,
//!   a task id that names none — is reported IN BAND on a 200 whose result carries
//!   `isError` and a text block beginning `Error executing tool ` and the tool's name,
//!   never as a modelled API error;
//! * every result reports `isError`, the success flag included, and only a tool that
//!   performed a run reports `structuredContent`;
//! * a run reaches nothing of the HOST: no socket, no process, no host file and no bound
//!   of its own.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so what is asserted is the wire behaviour a client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::{Duration, Instant};

use super::code_interpreter_tools::{
    emulator, invoke, is_error, result_event, start_session, structured, text_of, Call,
    SYSTEM_INTERPRETER,
};
use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// `executeCode` arguments for a Python program.
fn python(code: &str) -> Value {
    json!({"code": code, "language": "python"})
}

/// The `result` of one tool invocation on `session`, asserting the wire shape around it.
async fn tool(
    router: &MultiServiceRouter,
    case: &str,
    session: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let call: Call = invoke(router, Some(session), name, arguments).await;
    result_event(case, &call)
}

/// The text of `member` in a result's `structuredContent`.
fn structured_text(result: &Value, member: &str) -> String {
    structured(result, member)
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How long a case waits for a clock-owed transition before giving up. Generous: what is
/// asserted is that the transition HAPPENS, never how fast this host is.
const CONVERGENCE_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Poll `getTask` until the task reaches a status nothing moves it out of, or the
/// deadline passes. Answers the last result it read.
async fn poll_task(router: &MultiServiceRouter, case: &str, session: &str, task_id: &str) -> Value {
    let started = Instant::now();
    loop {
        let result = tool(
            router,
            case,
            session,
            "getTask",
            json!({"taskId": task_id}),
        )
        .await;
        let status = structured_text(&result, "taskStatus");
        if matches!(status.as_str(), "completed" | "failed" | "canceled") {
            return result;
        }
        assert!(
            started.elapsed() < CONVERGENCE_DEADLINE,
            "case {case:?}: the task stayed {status:?} for the whole deadline, got {result}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `executeCode` with `language: python` RUNS the program: each row's expected `stdout`
/// is what the Python language produces for it, computed by the interpreter rather than
/// held anywhere in this emulator.
///
/// Rows differ along what the rule generalises over: the kind of computation
/// (arithmetic, string, sequence, dictionary, closure, recursion, a module of the
/// standard library), the program's own form, which stream it writes to and how it ends.
#[tokio::test]
async fn python_execute_code_runs_the_submitted_program() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // (case, program, expected stdout, expected stderr fragment, expected exit code)
    let cases: Vec<(&str, &str, &str, &str, i64)> = vec![
        ("arithmetic", "print(6 * 7)", "42\n", "", 0),
        (
            "integer-precision",
            "print(2 ** 70)",
            "1180591620717411303424\n",
            "",
            0,
        ),
        (
            "string-methods",
            "print('héllo wörld'.upper())",
            "HÉLLO WÖRLD\n",
            "",
            0,
        ),
        (
            "sorted-sequence",
            "print(','.join(str(n) for n in sorted([10, 1, 5, 2])))",
            "1,2,5,10\n",
            "",
            0,
        ),
        (
            "dictionary-comprehension",
            "print({n: n * n for n in range(4)})",
            "{0: 0, 1: 1, 2: 4, 3: 9}\n",
            "",
            0,
        ),
        (
            "closure-and-recursion",
            "def fib(n):\n    return n if n < 2 else fib(n - 1) + fib(n - 2)\nprint(fib(20))",
            "6765\n",
            "",
            0,
        ),
        (
            "a-standard-library-module",
            "import json\nprint(json.dumps({'b': [1, 2], 'a': None}, sort_keys=True))",
            "{\"a\": null, \"b\": [1, 2]}\n",
            "",
            0,
        ),
        (
            "written-to-the-error-stream",
            "import sys\nsys.stderr.write('to-stderr\\n')",
            "",
            "to-stderr",
            0,
        ),
        (
            "an-undefined-name",
            "print(no_such_name)",
            "",
            "NameError",
            1,
        ),
        (
            "a-raised-exception",
            "raise ValueError('a bad value')",
            "",
            "ValueError: a bad value",
            1,
        ),
        (
            "a-program-that-exits",
            "import sys\nprint('before')\nsys.exit(3)",
            "before\n",
            "",
            3,
        ),
    ];

    for (case, program, stdout, stderr, exit_code) in cases {
        let result = tool(&router, case, &session, "executeCode", python(program)).await;
        assert_eq!(
            structured_text(&result, "stdout"),
            stdout,
            "case {case:?}: expected the program's own output, got {result}"
        );
        assert!(
            structured_text(&result, "stderr").contains(stderr),
            "case {case:?}: expected {stderr:?} on the error stream, got {result}"
        );
        assert_eq!(
            structured(&result, "exitCode").as_i64(),
            Some(exit_code),
            "case {case:?}: expected the status the run ended with, got {result}"
        );
        assert_eq!(
            is_error(&result),
            exit_code != 0,
            "case {case:?}: isError must follow how the run ended, got {result}"
        );
        assert!(
            structured(&result, "executionTime").as_f64().is_some(),
            "case {case:?}: a run reports how long it took, got {result}"
        );
    }
}

/// The names a Python program bound stay in the session's namespace for its later runs,
/// and `clearContext` drops them; a program that ended on an exception leaves nothing.
#[tokio::test]
async fn a_python_namespace_persists_until_clear_context_drops_it() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    let bound = tool(
        &router,
        "bind-a-name",
        &session,
        "executeCode",
        python("secret = 'kept'\ndef doubled(n):\n    return n * 2\nprint('set-ok')"),
    )
    .await;
    assert!(
        !is_error(&bound),
        "binding a name must succeed, got {bound}"
    );
    assert_eq!(structured_text(&bound, "stdout"), "set-ok\n");

    let read_back = tool(
        &router,
        "read-the-name-back",
        &session,
        "executeCode",
        python("print(secret, doubled(21))"),
    )
    .await;
    assert_eq!(
        structured_text(&read_back, "stdout"),
        "kept 42\n",
        "a later run sees what an earlier one bound, got {read_back}"
    );

    // A run that ended on an exception defines nothing a later run can rely on.
    let failed = tool(
        &router,
        "a-run-that-raised",
        &session,
        "executeCode",
        python("later = 'gone'\nraise RuntimeError('stop')"),
    )
    .await;
    assert!(is_error(&failed), "got {failed}");
    let absent = tool(
        &router,
        "the-name-of-a-failed-run",
        &session,
        "executeCode",
        python("print(later)"),
    )
    .await;
    assert!(
        structured_text(&absent, "stderr").contains("NameError"),
        "a failed run leaves no binding, got {absent}"
    );

    // `clearContext` drops the namespace BEFORE the submitted program runs.
    let mut cleared = python("print(secret)");
    cleared["clearContext"] = json!(true);
    let cleared = tool(&router, "clear-the-context", &session, "executeCode", cleared).await;
    assert!(
        structured_text(&cleared, "stderr").contains("NameError"),
        "a cleared context holds no earlier binding, got {cleared}"
    );

    // ... and the session still runs afterwards, on the namespace its own code builds.
    let after = tool(
        &router,
        "after-the-clear",
        &session,
        "executeCode",
        python("fresh = 1\nprint(fresh + 1)"),
    )
    .await;
    assert_eq!(structured_text(&after, "stdout"), "2\n", "got {after}");

    // A context program that no longer runs is not a later run's failure: it is dropped
    // from the session's context and the submitted program runs regardless.
    let wrote = tool(
        &router,
        "a-context-program-that-reads-a-file",
        &session,
        "executeCode",
        python(
            "open('context.txt', 'w').close()\nheld = open('context.txt').read()\n\
             print('read-ok')",
        ),
    )
    .await;
    assert_eq!(structured_text(&wrote, "stdout"), "read-ok\n", "got {wrote}");
    let removed = tool(
        &router,
        "remove-what-the-context-reads",
        &session,
        "removeFiles",
        json!({"paths": ["context.txt"]}),
    )
    .await;
    assert!(!is_error(&removed), "got {removed}");
    let later = tool(
        &router,
        "a-run-after-the-context-stopped-working",
        &session,
        "executeCode",
        python("print('later-ok')"),
    )
    .await;
    assert_eq!(
        structured_text(&later, "stdout"),
        "later-ok\n",
        "a context program that no longer runs must not fail a later run, got {later}"
    );
    assert!(!is_error(&later), "got {later}");
}

/// The file tools, a Python run and a command are three views of ONE filesystem: each
/// reads what the others wrote, in both directions.
#[tokio::test]
async fn the_three_views_of_a_session_share_one_filesystem() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    let written = tool(
        &router,
        "write-with-the-file-tool",
        &session,
        "writeFiles",
        json!({"content": [{"path": "data/notes.txt", "text": "from the file tool\n"}]}),
    )
    .await;
    assert!(!is_error(&written), "got {written}");

    // A Python run reads the file the file tool wrote, and writes one of its own.
    let read_in_python = tool(
        &router,
        "read-it-in-python",
        &session,
        "executeCode",
        python(
            "with open('data/notes.txt') as handle:\n    print(handle.read().strip())\n\
             with open('data/from-python.txt', 'w') as handle:\n    handle.write('from python\\n')",
        ),
    )
    .await;
    assert_eq!(
        structured_text(&read_in_python, "stdout"),
        "from the file tool\n",
        "a run reads and writes the same workspace the file tools do, got {read_in_python}"
    );

    // The file tool reads what the run wrote.
    let read_back = tool(
        &router,
        "read-the-run's-file-back",
        &session,
        "readFiles",
        json!({"paths": ["data/from-python.txt"]}),
    )
    .await;
    let resource = read_back
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.first())
        .and_then(|block| block.get("resource"))
        .and_then(|resource| resource.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        resource, "from python\n",
        "the file tool must read what the run wrote, got {read_back}"
    );

    // ... and the file tool lists what the run wrote.
    let listed = tool(
        &router,
        "list-what-the-run-wrote",
        &session,
        "listFiles",
        json!({"directoryPath": "data"}),
    )
    .await;
    let names: Vec<String> = listed
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["from-python.txt".to_string(), "notes.txt".to_string()],
        "the file tool must list what the run wrote, got {listed}"
    );

    // ... and so does a command.
    let catted = tool(
        &router,
        "cat-both-files",
        &session,
        "executeCommand",
        json!({"command": "cat data/notes.txt data/from-python.txt"}),
    )
    .await;
    assert_eq!(
        structured_text(&catted, "stdout"),
        "from the file tool\nfrom python\n",
        "a command must read the same workspace, got {catted}"
    );
    assert_eq!(structured(&catted, "exitCode").as_i64(), Some(0));
}

/// `executeCommand` performs the command and reports what it wrote and the status it
/// ended with: `exitCode` 0 for a command that succeeded, non-zero for one that failed,
/// and the two streams as the command wrote them.
///
/// Rows differ along what the rule generalises over: a command that writes output, one
/// that fails, one that changes the workspace, one made of several commands, and one
/// naming a program this session provides nothing for.
#[tokio::test]
async fn execute_command_reports_the_two_streams_and_the_exit_status() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;
    let written = tool(
        &router,
        "seed-a-file",
        &session,
        "writeFiles",
        json!({"content": [{"path": "held.txt", "text": "held\n"}]}),
    )
    .await;
    assert!(!is_error(&written), "got {written}");

    // (case, command, expected stdout, expected stderr fragment, expected exit code zero)
    let cases: Vec<(&str, &str, &str, &str, bool)> = vec![
        ("echo", "echo hello there", "hello there\n", "", true),
        ("cat-a-file-it-holds", "cat held.txt", "held\n", "", true),
        (
            "cat-a-file-it-does-not-hold",
            "cat absent.txt",
            "",
            "No such file",
            false,
        ),
        ("a-command-that-fails", "false", "", "", false),
        (
            "two-commands",
            "echo first && echo second",
            "first\nsecond\n",
            "",
            true,
        ),
        (
            "a-program-the-session-provides-none-for",
            "definitely-not-a-program --help",
            "",
            "",
            false,
        ),
        (
            "diagnostics-on-the-output-stream",
            "cat absent.txt 2>&1",
            "cat: absent.txt: No such file or directory\n",
            "",
            false,
        ),
    ];

    for (case, command, stdout, stderr, succeeded) in cases {
        let result = tool(
            &router,
            case,
            &session,
            "executeCommand",
            json!({"command": command}),
        )
        .await;
        assert_eq!(
            structured_text(&result, "stdout"),
            stdout,
            "case {case:?}: expected what the command wrote, got {result}"
        );
        assert!(
            structured_text(&result, "stderr").contains(stderr),
            "case {case:?}: expected {stderr:?} on the error stream, got {result}"
        );
        let exit_code = structured(&result, "exitCode").as_i64();
        assert!(
            exit_code.is_some(),
            "case {case:?}: a command reports an exit status, got {result}"
        );
        assert_eq!(
            exit_code == Some(0),
            succeeded,
            "case {case:?}: expected the status the command ended with, got {result}"
        );
        assert_eq!(
            is_error(&result),
            !succeeded,
            "case {case:?}: isError must follow the exit status, got {result}"
        );
    }

    // A command that CHANGES the workspace: the file tools read what it left.
    let redirected = tool(
        &router,
        "a-command-that-writes",
        &session,
        "executeCommand",
        json!({"command": "echo written-by-the-shell > from-shell.txt"}),
    )
    .await;
    assert_eq!(structured(&redirected, "exitCode").as_i64(), Some(0));
    let read = tool(
        &router,
        "read-what-the-command-wrote",
        &session,
        "executeCommand",
        json!({"command": "cat from-shell.txt"}),
    )
    .await;
    assert_eq!(
        structured_text(&read, "stdout"),
        "written-by-the-shell\n",
        "got {read}"
    );

    // A command that WAITS takes the time it waited for, which is what the result reports
    // as its own duration. The duration is read off the RESULT, never off this host's
    // clock.
    let waited = tool(
        &router,
        "a-command-that-waits",
        &session,
        "executeCommand",
        json!({"command": "sleep 1 && echo awake"}),
    )
    .await;
    assert_eq!(structured(&waited, "exitCode").as_i64(), Some(0));
    assert_eq!(structured_text(&waited, "stdout"), "awake\n", "got {waited}");
    assert!(
        // `executionTime` is "the execution time of the tool operation in milliseconds"
        // (source: smithy_query query="shape" shape="ToolResultStructuredContent").
        structured(&waited, "executionTime")
            .as_f64()
            .is_some_and(|reported| reported >= 1_000.0),
        "a command that waited a second reports at least that long, got {waited}"
    );
}

/// The sandbox a command runs in has the READ-ONLY utilities that report on its own
/// filesystem installed, beside the shell's own: each answers out of the session's
/// workspace, and a utility it does not have is still a command that could not run.
///
/// Rows differ along which utility is asked, what it is asked about (the workspace root, a
/// directory in it, a file, a path it holds nothing at) and whether the call names a count.
#[tokio::test]
async fn the_sandbox_provides_the_utilities_that_report_on_its_filesystem() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;
    let written = tool(
        &router,
        "seed-the-workspace",
        &session,
        "writeFiles",
        json!({"content": [
            {"path": "lines.txt", "text": "one\ntwo\nthree\n"},
            {"path": "data/notes.txt", "text": "noted\n"},
        ]}),
    )
    .await;
    assert!(!is_error(&written), "got {written}");

    // (case, command, expected stdout, expected exit zero)
    let cases: Vec<(&str, &str, &str, bool)> = vec![
        ("list-the-root", "ls", "data\nlines.txt\n", true),
        ("list-a-directory", "ls data", "notes.txt\n", true),
        ("list-a-file", "ls lines.txt", "lines.txt\n", true),
        (
            "list-a-path-it-holds-nothing-at",
            "ls absent",
            "ls: cannot access 'absent': No such file or directory\n",
            false,
        ),
        ("count-the-lines", "wc -l lines.txt", "3 lines.txt\n", true),
        ("count-everything", "wc data/notes.txt", "1 1 6 data/notes.txt\n", true),
        ("the-first-line", "head -n 1 lines.txt", "one\n", true),
        ("the-last-line", "tail -n 1 lines.txt", "three\n", true),
        ("the-first-lines", "head lines.txt", "one\ntwo\nthree\n", true),
        ("the-directory-it-runs-in", "pwd", "/\n", true),
    ];

    for (case, command, stdout, succeeded) in cases {
        let result = tool(
            &router,
            case,
            &session,
            "executeCommand",
            json!({"command": command}),
        )
        .await;
        assert_eq!(
            structured_text(&result, "stdout"),
            stdout,
            "case {case:?}: expected what the utility reports, got {result}"
        );
        assert_eq!(
            structured(&result, "exitCode").as_i64() == Some(0),
            succeeded,
            "case {case:?}: expected the status the utility ends with, got {result}"
        );
    }
}

/// The task plane is BOUNDED by the emulator, not by the caller: a command line longer than
/// one call carries is refused in band, and a session never holds more tasks than it may
/// however many a caller submits.
#[tokio::test]
async fn the_task_plane_is_bounded() {
    let (router, engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // `ToolArguments$command` targets `MaxLenString`, which admits 100 MB (source:
    // smithy_query query="shape" shape="ToolArguments"), so the ceiling is the emulator's
    // own: 64 KiB of command line.
    let past_the_ceiling = format!("echo {}", "x".repeat(70_000));
    for name in ["executeCommand", "startCommandExecution"] {
        let refused = tool(
            &router,
            name,
            &session,
            name,
            json!({"command": past_the_ceiling}),
        )
        .await;
        assert!(
            is_error(&refused),
            "case {name:?}: a command past the ceiling is refused, got {refused}"
        );
        assert!(
            text_of(&refused).starts_with(&format!("Error executing tool {name}")),
            "case {name:?}: expected the tool-failure lead-in, got {:?}",
            text_of(&refused)
        );
    }

    // A caller chooses how many tasks it submits; the session holds at most as many as it
    // may, whatever that number is.
    for index in 0..(agentcore::state::MAX_SESSION_TASKS + 16) {
        let submitted = tool(
            &router,
            "submit-many-tasks",
            &session,
            "startCommandExecution",
            json!({"command": format!("echo task-{index}")}),
        )
        .await;
        // Either the task was taken, with a report of its own, or the session had no room
        // for it and says so in band; what is bounded is what it HOLDS.
        assert!(
            submitted.get("structuredContent").is_some() || is_error(&submitted),
            "case {index}: a submission is answered either way, got {submitted}"
        );
        let held = {
            let state = agentcore::state::service_state(&*engine).expect("the service state");
            let store = state.lock();
            store
                .code_interpreter_session(SYSTEM_INTERPRETER, &session)
                .map_or(0, |session| session.tasks.len())
        };
        assert!(
            held <= agentcore::state::MAX_SESSION_TASKS,
            "case {index}: a session holds at most {} tasks, got {held}",
            agentcore::state::MAX_SESSION_TASKS
        );
    }
}

/// One session's filesystem is not another's: a command in one session cannot read a path
/// the other session holds, and says so the way a shell says it.
#[tokio::test]
async fn a_session_filesystem_is_not_visible_to_another_session() {
    let (router, _engine) = emulator();
    let first = start_session(&router, json!({})).await;
    let second = start_session(&router, json!({})).await;
    assert_ne!(first, second, "two sessions are two sessions");

    for (case, session, path) in [
        ("first-session", first.as_str(), "only-in-first.txt"),
        ("second-session", second.as_str(), "only-in-second.txt"),
    ] {
        let written = tool(
            &router,
            case,
            session,
            "writeFiles",
            json!({"content": [{"path": path, "text": "private\n"}]}),
        )
        .await;
        assert!(!is_error(&written), "case {case:?}: got {written}");
    }

    for (case, session, other_path) in [
        ("first-reads-the-second's", first.as_str(), "only-in-second.txt"),
        ("second-reads-the-first's", second.as_str(), "only-in-first.txt"),
    ] {
        let shell = tool(
            &router,
            case,
            session,
            "executeCommand",
            json!({"command": format!("cat {other_path}")}),
        )
        .await;
        assert_ne!(
            structured(&shell, "exitCode").as_i64(),
            Some(0),
            "case {case:?}: another session's path must not be readable, got {shell}"
        );
        assert!(
            structured_text(&shell, "stderr").contains("No such file"),
            "case {case:?}: expected the shell's own diagnostic, got {shell}"
        );
        // A Python run in the same session cannot reach it either.
        let run = tool(
            &router,
            case,
            session,
            "executeCode",
            python(&format!(
                "try:\n    open({other_path:?}).read()\n    print('reached')\nexcept \
                 FileNotFoundError:\n    print('not-found')"
            )),
        )
        .await;
        assert_eq!(
            structured_text(&run, "stdout"),
            "not-found\n",
            "case {case:?}: a run must not reach another session's file, got {run}"
        );
    }
}

/// `startCommandExecution` submits a task: the result names it and reports a status
/// nothing has finished yet, `getTask` converges it to `completed` with the command's own
/// output and exit status, and `stopTask` cancels one that has not converged — so the
/// command it carried never runs.
#[tokio::test]
async fn a_command_task_converges_and_a_stop_cancels_it() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;
    let written = tool(
        &router,
        "write-the-script",
        &session,
        "writeFiles",
        json!({"content": [
            {"path": "run.sh", "text": "echo task-ran\n"},
            {"path": "cancel.sh", "text": "echo cancelled-task-ran > marker.txt\n"},
        ]}),
    )
    .await;
    assert!(!is_error(&written), "got {written}");

    let submitted = tool(
        &router,
        "submit-a-task",
        &session,
        "startCommandExecution",
        json!({"command": "sh run.sh"}),
    )
    .await;
    assert!(
        !is_error(&submitted),
        "submitting a task must be served, got {submitted}"
    );
    let task_id = structured_text(&submitted, "taskId");
    assert!(
        !task_id.is_empty(),
        "a submitted task is named by a taskId, got {submitted}"
    );
    let status = structured_text(&submitted, "taskStatus");
    assert!(
        // `TaskStatus` is submitted | working | completed | canceled | failed (source:
        // smithy_query query="shape" shape="TaskStatus"); the first two are the
        // non-terminal ones.
        matches!(status.as_str(), "submitted" | "working"),
        "a task that was just submitted is not finished, got {status:?} in {submitted}"
    );
    assert!(
        structured(&submitted, "exitCode").is_null(),
        "nothing has run yet, so there is no exit status, got {submitted}"
    );

    let converged = poll_task(&router, "poll-the-task", &session, &task_id).await;
    assert_eq!(
        structured_text(&converged, "taskStatus"),
        "completed",
        "a task whose command exits 0 converges to completed, got {converged}"
    );
    assert_eq!(
        structured(&converged, "exitCode").as_i64(),
        Some(0),
        "got {converged}"
    );
    assert_eq!(
        structured_text(&converged, "stdout"),
        "task-ran\n",
        "the converged task reports what its command wrote, got {converged}"
    );
    assert_eq!(
        structured_text(&converged, "taskId"),
        task_id,
        "a task is reported under the id it was minted with, got {converged}"
    );

    // A task cancelled before it converges: the command never runs, so nothing it would
    // have written is in the workspace.
    let cancelled = tool(
        &router,
        "submit-a-task-to-cancel",
        &session,
        "startCommandExecution",
        json!({"command": "sh cancel.sh"}),
    )
    .await;
    let cancel_id = structured_text(&cancelled, "taskId");
    let stopped = tool(
        &router,
        "stop-the-task",
        &session,
        "stopTask",
        json!({"taskId": cancel_id}),
    )
    .await;
    assert_eq!(
        structured_text(&stopped, "taskStatus"),
        "canceled",
        "a stop drives a task that has not finished to canceled, got {stopped}"
    );
    let read_back = poll_task(&router, "read-the-cancelled-task", &session, &cancel_id).await;
    assert_eq!(
        structured_text(&read_back, "taskStatus"),
        "canceled",
        "a cancelled task stays cancelled, got {read_back}"
    );
    let marker = tool(
        &router,
        "the-cancelled-command's-file",
        &session,
        "executeCommand",
        json!({"command": "cat marker.txt"}),
    )
    .await;
    assert_ne!(
        structured(&marker, "exitCode").as_i64(),
        Some(0),
        "a cancelled task's command must never have run, got {marker}"
    );

    // A taskId this session holds no task for is a tool-level failure, in band.
    for (case, name) in [("get", "getTask"), ("stop", "stopTask")] {
        let unknown = tool(
            &router,
            case,
            &session,
            name,
            json!({"taskId": "task-0000000000000000000000000"}),
        )
        .await;
        assert!(
            is_error(&unknown),
            "case {case:?}: an unknown task is an error result, got {unknown}"
        );
        assert!(
            text_of(&unknown).starts_with(&format!("Error executing tool {name}")),
            "case {case:?}: expected the tool-failure lead-in, got {:?}",
            text_of(&unknown)
        );
    }
}

/// A path that does not address the session workspace is a failure of the invoked TOOL:
/// HTTP 200, a `result` carrying `isError`, a text block that begins with the lead-in and
/// names the traversal, and no file content or resource link beside it.
///
/// Rows span the tools that take a path and both forms of leaving the workspace.
#[tokio::test]
async fn a_path_that_leaves_the_workspace_is_refused_on_the_result() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;
    // A control: the same tools answer normally for a path inside the workspace.
    let seeded = tool(
        &router,
        "a-path-inside-the-workspace",
        &session,
        "writeFiles",
        json!({"content": [{"path": "inside.txt", "text": "inside\n"}]}),
    )
    .await;
    assert!(!is_error(&seeded), "the positive control must pass, got {seeded}");

    let escapes = ["../../etc/hostname", "/etc/hostname", "data/../../escape.txt"];
    for path in escapes {
        let cases: Vec<(&str, &str, Value)> = vec![
            ("readFiles", "readFiles", json!({"paths": [path]})),
            ("removeFiles", "removeFiles", json!({"paths": [path]})),
            (
                "writeFiles",
                "writeFiles",
                json!({"content": [{"path": path, "text": "x"}]}),
            ),
            ("listFiles", "listFiles", json!({"directoryPath": path})),
        ];
        for (case, name, arguments) in cases {
            let case = format!("{case}-{path}");
            let result = tool(&router, &case, &session, name, arguments).await;
            assert!(
                is_error(&result),
                "case {case:?}: expected isError on the result, got {result}"
            );
            let text = text_of(&result);
            assert!(
                text.starts_with(&format!("Error executing tool {name}")),
                "case {case:?}: expected the lead-in and the tool name, got {text:?}"
            );
            assert!(
                text.contains("path traversal"),
                "case {case:?}: the reason must name the traversal, got {text:?}"
            );
            assert!(
                text.contains(path),
                "case {case:?}: the reason must name the path the request carried, got {text:?}"
            );
            let blocks = result
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                blocks.iter().all(|block| {
                    block.get("type").and_then(Value::as_str) == Some("text")
                }),
                "case {case:?}: no file content and no resource link accompanies a refusal, got \
                 {result}"
            );
            assert!(
                result.get("structuredContent").is_none(),
                "case {case:?}: a workspace tool reports no run, even when it failed, got {result}"
            );
        }
    }

    // The session is still usable afterwards: a refusal is not a broken session.
    let after = tool(
        &router,
        "after-the-refusals",
        &session,
        "readFiles",
        json!({"paths": ["inside.txt"]}),
    )
    .await;
    assert!(!is_error(&after), "got {after}");
}

/// A Python run reaches nothing of the HOST: every row is attempted FROM the run's own
/// code and must be refused, and the run is left with no way to switch off its bounds.
#[tokio::test]
async fn a_python_run_cannot_reach_the_host() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // (case, the attempt that must be refused). Every row is attempted in ONE run, each
    // inside its own `try`, so the rows are asserted individually while the test costs one
    // interpreter rather than one per row.
    let cases: Vec<(&str, &str)> = vec![
        (
            "a-socket",
            "import socket; socket.socket().connect(('1.1.1.1', 80))",
        ),
        ("a-raw-socket-extension", "import _socket"),
        ("the-process-starter", "import _posixsubprocess"),
        ("a-shell-of-the-host", "import os; os.system('echo hi')"),
        ("a-host-descriptor", "import os; os.open('/etc/hostname', 0)"),
        ("the-host-posix-stat", "import posix; posix.statvfs('/')"),
        ("a-native-library", "import ctypes"),
        ("a-memory-map", "import mmap"),
        (
            // Starting a thread RUNS its body in this process, on the run's own thread
            // (`performed` below). What stays out of reach is the call that would reach ACROSS
            // the threads of this process, into the run this one is served by.
            "an-interrupt-of-the-running-thread",
            "import _thread; _thread.interrupt_main()",
        ),
        (
            "a-re-import-of-a-dropped-module",
            "import sys; sys.modules.pop('posix', None); import posix; posix.fork()",
        ),
        (
            "a-built-in-module-of-its-own",
            "import _imp; _imp.create_builtin(None)",
        ),
        ("the-trace-slot", "import sys; sys.settrace(None)"),
        ("the-profile-slot", "import sys; sys.setprofile(None)"),
        ("the-recursion-limit", "import sys; sys.setrecursionlimit(10 ** 6)"),
        // A host path is not reachable through `open` either: the workspace is the whole
        // filesystem a run sees, and it holds no such entry.
        ("a-host-file", "open('/etc/hostname').read()"),
    ];

    // (case, the attempt that RUNS something of the sandbox rather than reaching the host).
    // Neither starting a child nor starting a thread is an escape. The child's program is one
    // of the sandbox's own utilities, resolved and performed IN this process over the files the
    // run was given (`…lambda…::state::execution::guest_child_process`), so no host process is
    // started and no host path is opened for it. A thread's body is the run's OWN code, called
    // on the run's own thread inside a slice of the run's own budget
    // (`…lambda…::state::execution::guest_thread`), so no thread of this process runs it and
    // every bound the run is judged against still applies to it. Each call answers what the
    // program, or the body, came to.
    let performed: Vec<(&str, &str)> = vec![
        ("a-subprocess", "import subprocess; subprocess.run(['echo'])"),
        ("a-process-pipe", "import os; os.popen('echo hi')"),
        ("a-thread", "import _thread; _thread.start_new_thread(print, ())"),
    ];

    let mut program = String::new();
    for (case, attempt) in cases.iter().chain(performed.iter()) {
        program.push_str(&format!(
            "try:\n    {attempt}\n    print({case:?}, 'reached')\nexcept BaseException as \
             error:\n    print({case:?}, 'refused', type(error).__name__)\n"
        ));
    }
    let result = tool(
        &router,
        "every-escape-from-one-run",
        &session,
        "executeCode",
        python(&program),
    )
    .await;
    let stdout = structured_text(&result, "stdout");
    for (case, attempt) in &cases {
        assert!(
            stdout.contains(&format!("{case} refused")),
            "case {case:?}: the host must not be reachable from a run ({attempt}), got \
             {stdout:?} in {result}"
        );
    }
    for (case, attempt) in &performed {
        assert!(
            stdout.contains(&format!("{case} reached")),
            "case {case:?}: what the sandbox itself runs is performed in this process, so the \
             call answers ({attempt}), got {stdout:?} in {result}"
        );
    }

    // No module that reaches the host is among the built-in modules a run is told about.
    let builtins = tool(
        &router,
        "the-built-in-module-names",
        &session,
        "executeCode",
        python(
            "import sys\nprint([name for name in ('_socket', '_ssl', 'select', \
             '_posixsubprocess', 'mmap', '_ctypes', '_thread') if name in \
             sys.builtin_module_names])",
        ),
    )
    .await;
    assert_eq!(
        structured_text(&builtins, "stdout"),
        "[]\n",
        "got {builtins}"
    );
}

/// A Python run is bounded: a program with no end of its own is STOPPED, and one that
/// recurses without end ends on the interpreter's own error rather than taking the
/// process down.
#[tokio::test]
async fn a_python_run_is_bounded() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    let stopped = tool(
        &router,
        "a-loop-with-no-end",
        &session,
        "executeCode",
        python("n = 0\nwhile True:\n    n += 1"),
    )
    .await;
    assert!(
        is_error(&stopped),
        "a program that does not finish is an error, got {stopped}"
    );
    assert!(
        structured_text(&stopped, "stderr").contains("stopped"),
        "the result must say the program was stopped, got {stopped}"
    );
    assert_eq!(
        structured(&stopped, "exitCode").as_i64(),
        Some(1),
        "got {stopped}"
    );

    // The session still runs afterwards, which is what says the bound stopped the
    // program rather than the engine.
    let after = tool(
        &router,
        "after-the-stop",
        &session,
        "executeCode",
        python("print('still-here')"),
    )
    .await;
    assert_eq!(structured_text(&after, "stdout"), "still-here\n", "got {after}");

    let deep = tool(
        &router,
        "recursion-with-no-end",
        &session,
        "executeCode",
        python("def down(n):\n    return down(n + 1)\ndown(0)"),
    )
    .await;
    assert!(
        is_error(&deep) && structured_text(&deep, "stderr").contains("Recursion"),
        "deep recursion ends on the interpreter's own error, got {deep}"
    );
}

/// EVERY `result` event reports `isError`, and only a tool that performed a run reports
/// `structuredContent`.
///
/// `isError` is the one in-band signal a 200 carries about whether the tool did what was
/// asked — it is `true` when "the content contains error messages or exception
/// information" and `false` when "the content contains successful execution results"
/// (source: smithy_query query="shape" shape="CodeInterpreterResult") — so a result that
/// leaves it out tells a client nothing, and a client reading the member gets a missing
/// key. `structuredContent` is the RUN's own report (`stdout`, `stderr`, `exitCode`,
/// `executionTime`, `taskId`, `taskStatus` are its only members, source: smithy_query
/// query="shape" shape="ToolResultStructuredContent"), so a tool that performed no run
/// reports none of it.
///
/// Rows span the tool classes of the surface (source: smithy_query query="shape"
/// shape="ToolName") and both outcomes of each: a workspace tool that did what was asked
/// and one that could not, a run that ended 0 and one that did not in each language this
/// session runs, a command of each kind, a language no engine here runs, and a task id
/// this session holds no task for.
#[tokio::test]
async fn every_result_reports_is_error_and_only_a_run_reports_a_structured_report() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;
    let seeded = tool(
        &router,
        "seed-the-workspace",
        &session,
        "writeFiles",
        json!({"content": [
            {"path": "held.txt", "text": "held\n"},
            {"path": "spare.txt", "text": "spare\n"},
        ]}),
    )
    .await;
    assert!(!is_error(&seeded), "the seed must be served, got {seeded}");

    // (case, tool, arguments, the result is an error, the tool performed a run)
    let cases: Vec<(&str, &str, Value, bool, bool)> = vec![
        (
            "write-a-file",
            "writeFiles",
            json!({"content": [{"path": "written.txt", "text": "x"}]}),
            false,
            false,
        ),
        (
            "read-a-file-it-holds",
            "readFiles",
            json!({"paths": ["held.txt"]}),
            false,
            false,
        ),
        ("list-the-workspace", "listFiles", json!({}), false, false),
        (
            "remove-a-file-it-holds",
            "removeFiles",
            json!({"paths": ["spare.txt"]}),
            false,
            false,
        ),
        (
            "read-a-file-it-does-not-hold",
            "readFiles",
            json!({"paths": ["absent.txt"]}),
            true,
            false,
        ),
        (
            "remove-a-file-it-does-not-hold",
            "removeFiles",
            json!({"paths": ["absent.txt"]}),
            true,
            false,
        ),
        (
            "write-outside-the-workspace",
            "writeFiles",
            json!({"content": [{"path": "../escape.txt", "text": "x"}]}),
            true,
            false,
        ),
        (
            "run-javascript-that-prints",
            "executeCode",
            json!({"code": "console.log(1 + 1)", "language": "javascript"}),
            false,
            true,
        ),
        (
            "run-javascript-that-throws",
            "executeCode",
            json!({"code": "throw new Error('boom')", "language": "javascript"}),
            true,
            true,
        ),
        (
            "run-python-that-prints",
            "executeCode",
            python("print(1 + 1)"),
            false,
            true,
        ),
        (
            "run-python-that-raises",
            "executeCode",
            python("print(absent_name)"),
            true,
            true,
        ),
        (
            "a-command-that-succeeds",
            "executeCommand",
            json!({"command": "cat held.txt"}),
            false,
            true,
        ),
        (
            "a-command-that-fails",
            "executeCommand",
            json!({"command": "cat absent.txt"}),
            true,
            true,
        ),
        (
            "a-language-no-engine-here-runs",
            "executeCode",
            json!({"code": "const a: number = 1;", "language": "typescript"}),
            true,
            false,
        ),
        (
            "a-task-this-session-holds-none-of",
            "getTask",
            json!({"taskId": "task-0000000000000000000000000"}),
            true,
            false,
        ),
    ];

    for (case, name, arguments, failed, ran) in cases {
        let result = tool(&router, case, &session, name, arguments).await;
        assert_eq!(
            result.get("isError").and_then(Value::as_bool),
            Some(failed),
            "case {case:?}: every result reports isError, got {result}"
        );
        assert_eq!(
            result.get("structuredContent").is_some(),
            ran,
            "case {case:?}: only a tool that performed a run reports structuredContent, \
             got {result}"
        );
    }

    // The asynchronous half reports the same way: the submission, and each read of the
    // task that follows it, carries the flag beside the task's own report.
    let submitted = tool(
        &router,
        "submit-a-task",
        &session,
        "startCommandExecution",
        json!({"command": "echo task-ran"}),
    )
    .await;
    assert_eq!(
        (
            submitted.get("isError").and_then(Value::as_bool),
            submitted.get("structuredContent").is_some(),
        ),
        (Some(false), true),
        "a submitted task is no error and carries its own report, got {submitted}"
    );
    let task_id = structured_text(&submitted, "taskId");
    let converged = poll_task(&router, "poll-the-task", &session, &task_id).await;
    assert_eq!(
        (
            converged.get("isError").and_then(Value::as_bool),
            structured_text(&converged, "taskStatus"),
        ),
        (Some(false), "completed".to_string()),
        "a task whose command exits 0 is no error, got {converged}"
    );

    let failing = tool(
        &router,
        "submit-a-failing-task",
        &session,
        "startCommandExecution",
        json!({"command": "cat absent.txt"}),
    )
    .await;
    let failing_id = structured_text(&failing, "taskId");
    let failed = poll_task(&router, "poll-the-failing-task", &session, &failing_id).await;
    assert_eq!(
        (
            failed.get("isError").and_then(Value::as_bool),
            structured_text(&failed, "taskStatus"),
        ),
        (Some(true), "failed".to_string()),
        "a task whose command failed is an error result, got {failed}"
    );
}
