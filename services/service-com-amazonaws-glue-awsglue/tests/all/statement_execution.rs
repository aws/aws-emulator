// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a statement submitted to an interactive session EXECUTES: the
//! code the run carried, on the session's Spark runtime, reported back through `GetStatement`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour over state the service really holds.
//!
//! The rule under test: a statement run on a `READY` session is ONE execution of its own
//! `Code` on that session's runtime — so the statement is observably `RUNNING` while that code
//! is in flight, and the state it comes to rest in is what the execution came to (`AVAILABLE`
//! with the output the code itself wrote, the modelled error state carrying the CODE's own
//! exception for one that raised), and the effects the code makes are really made.
//!
//! The recorded case — a cell submitted to a `READY` session, observed `RUNNING`, then the
//! session deleted under it — is one row here; the rows of the table differ along what the
//! code DOES (writes output, answers one expression's value, computes over the Spark runtime
//! the session binds, raises, cannot be compiled, does nothing at all) and therefore along
//! what the statement comes to rest in. A second table pins the CONTAINMENT of the runtime the
//! cell runs on: one row per primitive a cell could try to reach the host with, each attempted
//! from the cell, against a control row that reaches this emulator and succeeds.
//!
//! The spans a statement's own lifecycle takes are made due THROUGH THE STORE rather than
//! slept for, and every statement is read back until it has come to rest
//! ([`crate::statements::statement_at_rest`]) — the execution a statement is owed is performed
//! by whichever call finds it owed — so nothing here depends on how fast the host runs.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_script_execution::{create_bucket, emulator_with_s3, object_body, BUCKET};
use crate::statements::{
    get, ready_session, run, statement_at_rest, STATEMENT_AT_REST_DEADLINE, STATE_AVAILABLE,
};
use crate::wire::{call, emulator_on, error_code, error_message, ok};

/// The `StatementState` values this suite observes, of the modelled enum (`WAITING`,
/// `RUNNING`, `AVAILABLE`, `CANCELLING`, `CANCELLED`, `ERROR`, from
/// `smithy_query shape=StatementState`).
const STATE_RUNNING: &str = "RUNNING";
const STATE_ERROR: &str = "ERROR";

/// What a row expects of the statement once it has come to rest.
#[derive(Debug)]
enum Expected {
    /// The code ran to its end, and this is the text its output must carry — compared with
    /// the surrounding whitespace trimmed, because what is pinned is what the code WROTE and
    /// not the line break a write ends with. `None` is a cell that wrote nothing, which
    /// reports no output data at all.
    Available(Option<&'static str>),
    /// The code failed, and this is the exception's own name; the value must be the message
    /// the code raised it with, where the row states one.
    Errored {
        /// `StatementOutput$ErrorName`.
        name: &'static str,
        /// `StatementOutput$ErrorValue`, where the row pins it.
        value: Option<&'static str>,
    },
}

/// The text one statement's output carries, or `None` when it reports none.
fn output_text(statement: &Value) -> Option<String> {
    statement
        .pointer("/Output/Data/TextPlain")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// What a statement's code came to is what the statement reports: the rows span what the code
/// does, and each one is read back until the statement has come to rest.
#[tokio::test]
async fn what_a_statements_code_came_to_is_what_the_statement_reports() {
    // (case, the session, the code submitted, what the statement must come to rest as)
    let rows: Vec<(&str, &str, &str, Expected)> = vec![
        (
            "a cell that writes output",
            "writes-output",
            "print('hello from the cell')",
            Expected::Available(Some("hello from the cell")),
        ),
        (
            // An interactive runtime answers the value of a cell that is one expression.
            "a cell that is one expression",
            "one-expression",
            "6 * 7",
            Expected::Available(Some("42")),
        ),
        (
            "a multi-line cell that computes and writes",
            "multi-line",
            "total = sum(n * n for n in range(5))\nprint(total)",
            Expected::Available(Some("30")),
        ),
        (
            // The names a Glue session binds for its cells: the Spark context is one of them,
            // and the computation is really performed over the program's own data.
            "a cell that computes over the session's Spark runtime",
            "spark-runtime",
            "sc.parallelize([1, 2, 3]).map(lambda n: n * 2).collect()",
            Expected::Available(Some("[2, 4, 6]")),
        ),
        (
            "a cell that raises",
            "raises",
            "raise ValueError('the partition is malformed')",
            Expected::Errored {
                name: "ValueError",
                value: Some("the partition is malformed"),
            },
        ),
        (
            "a cell whose arithmetic fails",
            "arithmetic-fails",
            "rows = 0\nprint(10 / rows)",
            Expected::Errored {
                name: "ZeroDivisionError",
                value: None,
            },
        ),
        (
            "a cell that cannot be compiled",
            "not-compilable",
            "print('unbalanced'",
            Expected::Errored {
                name: "SyntaxError",
                value: None,
            },
        ),
        (
            // "len 0..68000": the empty cell is inside the modelled length, and it computes
            // nothing, so it ran to its end having written nothing.
            "a cell that does nothing at all",
            "empty-cell",
            "",
            Expected::Available(None),
        ),
    ];

    for (case, session, code, expected) in &rows {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        ready_session(&router, &engine, session).await;

        let id = run(&router, session, code).await;
        let settled = statement_at_rest(&router, &engine, session, id).await;

        match expected {
            Expected::Available(text) => {
                assert_eq!(
                    settled["State"].as_str(),
                    Some(STATE_AVAILABLE),
                    "case {case}: code that ran to its end must come to rest in \
                     {STATE_AVAILABLE}, got {settled}"
                );
                assert_eq!(
                    settled.pointer("/Output/Status").and_then(Value::as_str),
                    Some(STATE_AVAILABLE),
                    "case {case}: the output status must be the state the statement rests \
                     in, got {settled}"
                );
                match text {
                    Some(expected_text) => assert_eq!(
                        output_text(&settled).as_deref().map(str::trim),
                        Some(*expected_text),
                        "case {case}: the output must carry what the code wrote, got {settled}"
                    ),
                    None => assert_eq!(
                        output_text(&settled),
                        None,
                        "case {case}: a cell that wrote nothing reports no output data, got \
                         {settled}"
                    ),
                }
                assert!(
                    settled
                        .pointer("/Output/ErrorName")
                        .is_none_or(Value::is_null),
                    "case {case}: code that failed at nothing names no error, got {settled}"
                );
            }
            Expected::Errored { name, value } => {
                assert_eq!(
                    settled["State"].as_str(),
                    Some(STATE_ERROR),
                    "case {case}: code that failed must come to rest in {STATE_ERROR}, got \
                     {settled}"
                );
                assert_eq!(
                    settled.pointer("/Output/ErrorName").and_then(Value::as_str),
                    Some(*name),
                    "case {case}: the output must name the exception the code raised, got \
                     {settled}"
                );
                let reported = settled
                    .pointer("/Output/ErrorValue")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match value {
                    Some(expected_value) => assert_eq!(
                        reported, *expected_value,
                        "case {case}: the output must carry the message the code raised, got \
                         {settled}"
                    ),
                    None => assert!(
                        !reported.is_empty(),
                        "case {case}: a failed statement must state why, got {settled}"
                    ),
                }
            }
        }

        // The record is the statement's own: its code and its id come back unchanged, and a
        // finished statement reports when it completed.
        assert_eq!(
            settled["Code"].as_str(),
            Some(*code),
            "case {case}: the code must round-trip, got {settled}"
        );
        assert!(
            settled["CompletedOn"].as_i64().unwrap_or_default()
                > settled["StartedOn"].as_i64().unwrap_or_default(),
            "case {case}: a statement that came to rest completed after it started, got \
             {settled}"
        );
        // "The execution count of the output": which execution of this session's runtime
        // produced it, which for the first statement of a session is its own id.
        assert_eq!(
            settled.pointer("/Output/ExecutionCount").and_then(Value::as_i64),
            Some(id),
            "case {case}: the output must name the execution that produced it, got {settled}"
        );
    }
}

/// A statement is observably RUNNING while its code is in flight, and the session can be
/// DELETED under it: the delete is served, and the session and the statement are then both
/// misses.
///
/// This is the recorded flow, with the wait a POLL for the state rather than a span anything
/// is expected to pass in: nothing is made due through the store here, so the statement
/// reaches `RUNNING` on the service's own clock.
#[tokio::test]
async fn a_statement_is_observable_running_and_its_session_can_be_deleted_under_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let session = "deleted-mid-statement";
    ready_session(&router, &engine, session).await;
    let id = run(&router, session, "total = sum(range(1000))").await;

    let running = statement_in_state(&router, session, id, STATE_RUNNING).await;
    assert_eq!(
        running["State"].as_str(),
        Some(STATE_RUNNING),
        "a statement whose code is in flight must be observable {STATE_RUNNING}, got {running}"
    );
    // A statement that has not finished reports no output yet.
    assert!(
        running.get("Output").is_none_or(Value::is_null),
        "a statement that has not finished reports no Output, got {running}"
    );

    // The delete lands while the statement is still running: it is served, and the id is a
    // miss for every later read of the session and of its statements.
    assert_eq!(
        ok(&router, "DeleteSession", &json!({ "Id": session }))
            .await
            .get("Id")
            .and_then(Value::as_str),
        Some(session),
        "DeleteSession must report the id it deleted"
    );
    for (operation, request) in [
        ("GetSession", json!({ "Id": session })),
        ("GetStatement", json!({ "SessionId": session, "Id": id })),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "{operation} on a deleted session must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "{operation} must answer EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert!(
            !error_message(&body).is_empty(),
            "{operation} must state what was not found, got {body}"
        );
    }
}

/// A cell's own EFFECTS are really made: code that writes an object through `boto3` leaves an
/// object a client then reads, so the session's runtime reaches this same emulator.
#[tokio::test]
async fn a_cells_own_aws_calls_reach_this_emulator() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    let session = "writes-an-object";
    ready_session(&router, &engine, session).await;

    let key = "statements/written-by-a-cell.txt";
    let code = format!(
        "import boto3\n\
         boto3.client('s3').put_object(Bucket='{BUCKET}', Key='{key}', Body=b'written by a cell')\n\
         print('wrote {key}')\n"
    );
    let id = run(&router, session, &code).await;
    let settled = statement_at_rest(&router, &engine, session, id).await;
    assert_eq!(
        settled["State"].as_str(),
        Some(STATE_AVAILABLE),
        "the cell must run to its end, got {settled}"
    );
    assert_eq!(
        object_body(&router, key).await.as_deref(),
        Some("written by a cell"),
        "the object the cell wrote must be readable from the object store, got {settled}"
    );
}

/// A cell reaches nothing of the HOST: each attempt raises inside the cell, so the statement
/// comes to rest in the modelled error state carrying the cell's own exception and no host
/// socket, process, library, file or environment variable is reached from a Glue statement.
///
/// The rows are one per primitive a cell could try to reach the host with — a socket, a host
/// process, a native library, a guest thread, the trace hook and the recursion limit the bounds
/// are spent through, the filesystem and the environment through `os`, the import machinery
/// (`importlib` and the import builtin) and the `open` builtin that needs no import at all —
/// and each is attempted FROM the cell. The control is the row that writes an object instead,
/// which must succeed, so the table cannot pass by every cell failing for some other reason.
///
/// What each row asserts is decided by the cell's own EXECUTION: what a contained cell reaches
/// for is a capability the engine never registered — the module itself where the runtime has
/// none, and otherwise the CALL that would need the host — so the cell raises and the statement
/// carries that exception.
#[tokio::test]
async fn a_cell_reaches_nothing_of_the_host() {
    // (case, the code submitted, whether the statement must come to rest having run to its end)
    let rows: [(&str, &str, bool); 12] = [
        (
            "connecting a socket to a host",
            "import socket\nsocket.socket().connect((\"example.com\", 80))\n",
            false,
        ),
        (
            "starting a host process",
            "import subprocess\nsubprocess.run([\"python3\", \"-c\", \"print(1)\"])\n",
            false,
        ),
        (
            "loading a host library",
            "import ctypes\nctypes.CDLL(None)\n",
            false,
        ),
        (
            "reading the host filesystem through the os module",
            "import os\nos.listdir(\"/\")\n",
            false,
        ),
        (
            "reading a host environment variable",
            "import os\nos.environ[\"PATH\"]\n",
            false,
        ),
        (
            // The same reach made through the import machinery rather than a statement, and
            // through the import builtin, so the refusal is not one `import` spells alone.
            "importing a host module through importlib",
            "import importlib\nimportlib.import_module(\"subprocess\").run([\"python3\"])\n",
            false,
        ),
        (
            "importing a host module through the import builtin",
            "__import__(\"subprocess\").run([\"python3\"])\n",
            false,
        ),
        (
            // Starting a thread RUNS its body on the cell's own thread, inside a slice of the
            // cell's own budget (`…lambda…::state::execution::guest_thread`), so it is no reach
            // of the host. What stays out of reach is the call that reaches ACROSS the threads
            // of this process, into the run the cell is served by.
            "interrupting the thread a cell runs on",
            "import _thread\n_thread.interrupt_main()\n",
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
        (
            // The builtin that reads a host path without importing anything at all.
            "opening a host file through the open builtin",
            "open(\"/etc/hostname\").read()\n",
            false,
        ),
        // The control: a cell that reaches only this emulator does its work.
        ("writing an object instead", "", true),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, code, succeeds)) in rows.into_iter().enumerate() {
        let session = format!("contained-{index}");
        ready_session(&router, &engine, &session).await;
        let key = format!("contained/{session}.txt");
        // The control row writes through the one seam a cell HAS — the AWS SDK over this
        // emulator's own callback — so what every other row is denied is the host and not the
        // ability to do anything at all.
        let written = format!(
            "import boto3\n\
             boto3.client('s3').put_object(Bucket='{BUCKET}', Key='{key}', Body=b'reached')\n"
        );
        let submitted = if succeeds { written.as_str() } else { code };

        let id = run(&router, &session, submitted).await;
        let settled = statement_at_rest(&router, &engine, &session, id).await;
        let expected = if succeeds { STATE_AVAILABLE } else { STATE_ERROR };
        assert_eq!(
            settled["State"].as_str(),
            Some(expected),
            "case {case:?}: expected the statement to come to rest in {expected}, got {settled}"
        );
        if succeeds {
            assert_eq!(
                object_body(&router, &key).await.as_deref(),
                Some("reached"),
                "case {case:?}: the cell that reaches only this emulator must leave its object"
            );
            continue;
        }
        assert!(
            settled
                .pointer("/Output/ErrorName")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.is_empty()),
            "case {case:?}: the statement must carry the exception the cell raised, got {settled}"
        );
        assert_eq!(
            object_body(&router, &key).await,
            None,
            "case {case:?}: a cell that could not reach the host wrote nothing"
        );
    }
}

/// A cell that runs past a BOUND this emulator gives one statement is stopped and reported as
/// a statement error rather than hanging — and the interpreter it held is given back, so the
/// session takes the next statement afterwards.
///
/// The rows are the two bounds a cell can spend from its own code: the step budget a cell that
/// computes without end spends, and the recursion limit a cell that recurses without end
/// reaches. Which of the engine's bounds answers a given cell is the engine's to decide, so
/// what is asserted is that the statement comes to REST in the modelled error state naming
/// something, inside the window the poll allows, and never that one particular bound answered.
#[tokio::test]
async fn a_cell_past_a_bound_is_stopped_and_reported_as_a_statement_error() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // (case, the session, the code submitted)
    let rows: [(&str, &str, &str); 2] = [
        (
            "a cell that computes without end",
            "no-end",
            "total = 0\nwhile True:\n    total += 1\n",
        ),
        (
            "a cell that recurses without end",
            "no-floor",
            "def deeper(depth):\n    return deeper(depth + 1)\n\n\ndeeper(0)\n",
        ),
    ];

    for (case, session, code) in rows {
        ready_session(&router, &engine, session).await;
        let id = run(&router, session, code).await;
        let settled = statement_at_rest(&router, &engine, session, id).await;
        assert_eq!(
            settled["State"].as_str(),
            Some(STATE_ERROR),
            "case {case}: a cell past a bound must come to rest in {STATE_ERROR}, got {settled}"
        );
        assert!(
            settled
                .pointer("/Output/ErrorName")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.is_empty()),
            "case {case}: the statement must name what ended it, got {settled}"
        );

        // The interpreter the stopped cell held is back: the session takes another statement
        // and that one runs to its end.
        let next = run(&router, session, "print('after the bound')").await;
        let after = statement_at_rest(&router, &engine, session, next).await;
        assert_eq!(
            after["State"].as_str(),
            Some(STATE_AVAILABLE),
            "case {case}: the session must take a statement again after the bound, got {after}"
        );
    }
}

/// How long one turn of a poll for a statement's state waits before reading again — the gap
/// between two reads of a client that polls, not a span anything is expected to finish
/// inside.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Read the statement `id` of `session` back until it reports `state`, and answer what it then
/// reports.
///
/// Polled for the CONDITION — that the statement has reached that state — against the same
/// generous window a poll for one at rest uses ([`STATEMENT_AT_REST_DEADLINE`]), so nothing
/// here asserts how long the service took to get there.
async fn statement_in_state(
    router: &MultiServiceRouter,
    session: &str,
    id: i64,
    state: &str,
) -> Value {
    let deadline = SystemTime::now() + STATEMENT_AT_REST_DEADLINE;
    loop {
        let read = get(router, session, id).await;
        if read["State"].as_str() == Some(state) {
            return read;
        }
        assert!(
            SystemTime::now() < deadline,
            "statement {id} of {session:?} must reach {state}, and it reports {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
