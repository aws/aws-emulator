// SPDX-License-Identifier: CC0-1.0

//! What a WARM execution environment carries from one invocation into the next: the work the
//! handler left on the event loop.
//!
//! RULE: an execution environment is kept after an invocation and serves the next one — "after
//! a Lambda function is invoked, Lambda maintains the execution environment for some time in
//! anticipation of another invocation", and an invocation that environment serves "skips the
//! initialization phase" — and the runtime inside it is not reset between invocations. Its
//! event loop is the same loop: a callback the handler scheduled and did not wait for is NOT
//! cancelled when the invocation is answered, so it runs during a LATER invocation served in
//! that environment, and that invocation observes its side effect. Module scope is the
//! environment's for the same reason: what a handler wrote there is what the next invocation
//! reads.
//!
//! So a handler may rely on warm-environment carry-over, which AWS documents handlers may:
//! a background flush, a deferred write or a cache refresh a handler starts without awaiting
//! is work that still happens, and an emulator that tore the environment down at response time
//! would lose it silently and invert the documented behaviour of an unawaited timer.
//!
//! The rows differ along what the rule generalises over: WHAT the handler left on the loop (a
//! timer with no delay, a timer with a delay, an interval it never cleared, a promise
//! continuation behind a timer) — each left by one invocation, each observed by the next.
//!
//! Every row asserts both halves: the invocation that SCHEDULED the work sees nothing of it
//! (it did not wait for it), and the invocation after it sees it. Nothing here sleeps for a
//! span: the observing invocation yields to its own event loop, which is where the pending work
//! runs, and this suite waits for the CONDITION it reports.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{create_function, json_call, make_activation_due, router_on, ROLE};

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `Runtime` every function here carries: a managed Node release.
const RUNTIME: &str = "nodejs22.x";

/// The function `Timeout`, in seconds: far longer than any row needs, because what is under
/// test is the work the loop carried and not how fast a host gets through it.
const FUNCTION_TIMEOUT: u64 = 60;

/// How long the OBSERVING invocation gives the work the one before it left to run, in
/// milliseconds: generous, because what is under test is that the loop still holds that work
/// and not how fast this host turns it over. The observing handler polls for the work and
/// answers as soon as it has run, so a row never spends this.
const OBSERVE_WAIT_MS: u64 = 30_000;

/// The handler every row runs.
///
/// `schedule` leaves one piece of work on the event loop WITHOUT awaiting it and answers at
/// once; `observe` yields to that loop — which is where the pending work runs — and answers
/// what the environment has seen. Both phases are the same handler, so both are served by the
/// same execution environment.
const HANDLER_SOURCE: &str = r#"
// The tags the work this environment has left on its loop has reported, in the order it ran:
// module scope belongs to the EXECUTION ENVIRONMENT, so this survives an invocation.
const fired = [];
let interval = null;

exports.handler = async (event) => {
    if (event.phase === "observe") {
        // Yield to the event loop until the work the invocation before this one left has run
        // — which is where a node process runs it — or until the deadline this invocation was
        // given runs out. The LOOP is what is under test, so this waits for the work rather
        // than for a span: a callback already due runs on the first turn.
        const deadline = Date.now() + event.waitMs;
        while (fired.length < event.owed && Date.now() < deadline) {
            await new Promise((resolve) => setTimeout(resolve, 1));
        }
        if (interval !== null) {
            clearInterval(interval);
            interval = null;
        }
        return { phase: "observe", fired: fired.slice() };
    }
    if (event.work === "timer") {
        setTimeout(() => { fired.push(event.tag); }, event.delayMs);
    } else if (event.work === "interval") {
        interval = setInterval(() => { fired.push(event.tag); }, event.delayMs);
    } else if (event.work === "continuation") {
        const waited = new Promise((resolve) => setTimeout(resolve, event.delayMs));
        waited.then(() => { fired.push(event.tag); });
    }
    return { phase: "schedule", fired: fired.slice() };
};
"#;

/// One row: what the handler leaves on the loop, how long before it comes due, and the tag the
/// work reports.
struct Row {
    case: &'static str,
    work: &'static str,
    delay_ms: u64,
    tag: &'static str,
}

/// A .zip deployment package holding [`HANDLER_SOURCE`] as `index.js`.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("index.js", options)
            .expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function and make the activation the clock owes it due, so nothing here waits
/// for a span the service already owes.
async fn given_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": FUNCTION_TIMEOUT,
            "MemorySize": 256,
            "Code": {
                "ZipFile": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    package(),
                ),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// One `Invoke` of `name` with `event`, as the document the handler answered.
async fn invoke(router: &MultiServiceRouter, case: &str, name: &str, event: &Value) -> Value {
    let answer = json_call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.header("x-amz-function-error"),
        None,
        "case {case:?}: expected the handler to answer rather than fail, got {}",
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// RULE: work a handler left on the event loop without waiting for it is not cancelled when
/// its invocation is answered — the NEXT invocation served in that environment runs it and
/// observes what it did.
#[tokio::test]
async fn work_one_invocation_left_on_the_loop_runs_during_the_next() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let rows: &[Row] = &[
        Row {
            case: "a timer with no delay the handler did not await",
            work: "timer",
            delay_ms: 0,
            tag: "flushed",
        },
        Row {
            case: "a timer with a delay the handler did not await",
            work: "timer",
            delay_ms: 5,
            tag: "deferred",
        },
        Row {
            case: "an interval the handler never cleared",
            work: "interval",
            delay_ms: 5,
            tag: "ticked",
        },
        Row {
            case: "a continuation behind a timer the handler did not await",
            work: "continuation",
            delay_ms: 5,
            tag: "continued",
        },
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One function per row, so no row observes another's pending work.
        let function = format!("warm-loop-{index}");
        given_function(&router, &engine, &function).await;

        // The invocation that SCHEDULES the work does not wait for it, so it sees nothing of
        // it in its own answer.
        let scheduled = invoke(
            &router,
            case,
            &function,
            &json!({
                "phase": "schedule",
                "work": row.work,
                "delayMs": row.delay_ms,
                "tag": row.tag,
            }),
        )
        .await;
        assert_eq!(
            scheduled,
            json!({ "phase": "schedule", "fired": [] }),
            "case {case:?}: expected the scheduling invocation to answer without waiting for \
             the work it left, got {scheduled}"
        );

        // The invocation after it is served in the same environment, whose loop still holds
        // that work: it runs there, and this invocation observes it.
        let observed = invoke(
            &router,
            case,
            &function,
            &json!({ "phase": "observe", "owed": 1, "waitMs": OBSERVE_WAIT_MS }),
        )
        .await;
        assert_eq!(
            observed,
            json!({ "phase": "observe", "fired": [row.tag] }),
            "case {case:?}: expected the next invocation to run the work the one before it \
             left, got {observed}"
        );
    }
}
