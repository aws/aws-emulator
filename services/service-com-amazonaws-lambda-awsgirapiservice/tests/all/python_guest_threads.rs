// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the THREADS a `python3.x` function's own code starts:
//! `CreateFunction` then `Invoke` (`POST /2015-03-31/functions/{FunctionName}/invocations`)
//! over the router.
//!
//! RULE: a managed runtime's execution environment RUNS the threads the function's code starts
//! — `threading.Thread(target=…).start()` and `_thread.start_new_thread(…)` call the code they
//! name, with the arguments they were given — so the invocation is answered `200` with no
//! `X-Amz-Function-Error` and what the thread's own code did is what happened. A thread's own
//! failure is the thread's and not the invocation's, a thread whose body waits for something
//! only another thread could produce does not hold the invocation, and a start the platform
//! cannot serve is refused by the start itself with `RuntimeError: can't start new thread`
//! rather than answered with a thread nothing runs.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what the
//! rule generalises over: WHERE the thread is started (the handler, and the module body, which
//! is where an internal extension registers), WHICH interface starts it (`threading` and the
//! low-level `_thread`), how its body is given its arguments (none, positional, keyword), HOW
//! MANY are started (one, several, and past the count one invocation may start), and what the
//! body does (append, compute a value the handler joins for, raise, and block on the
//! extension event pump, which answers no event while the invocation that registered it is the
//! one in flight).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The `Timeout` every function this suite creates carries, in seconds. Nothing here asserts on
/// a deadline, so it is given generous room: how long this test binary's own scheduling takes is
/// not what any row reads.
const FUNCTION_TIMEOUT_SECONDS: i32 = 600;

/// How many times a case re-drives its invocation while the answer reports one of this test
/// binary's own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 6;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create a `python3.12` function whose package holds `source` as `lambda_function.py`, and wait
/// out the provisioning window the create documents.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.handler",
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("lambda_function.py", source)])),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` on `name` with `event`, re-driven while the answer reports a bound this whole test
/// binary shares rather than a run.
async fn invoke(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    let mut answered = invoke_once(router, name, event).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answered.status, &answered.text()) {
            break;
        }
        answered = invoke_once(router, name, event).await;
    }
    answered
}

/// One `Invoke` request on the function `name` names.
async fn invoke_once(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await
}

/// What one case of the probe answered, or `None` when the run was ended by a bound this whole
/// test binary shares rather than by anything the case asked for.
///
/// The invocation itself is part of every row's assertion: a handler that starts threads is
/// answered `200` with no function error, which is what the recorded symptom was not.
async fn answered(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
    event: &Value,
) -> Option<Value> {
    let answer = invoke(router, name, event).await;
    if is_host_bound(answer.status, &answer.text()) {
        return None;
    }
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the invocation to be answered 200, got {} {}",
        answer.status,
        answer.text()
    );
    let failure = answer.header("x-amz-function-error");
    assert_eq!(
        failure, None,
        "case {case:?}: a handler that starts a thread carries no function error, got \
         {failure:?} with {}",
        answer.text()
    );
    Some(answer.json())
}

/// The text a document's member holds, for a row that reads one.
fn text(case: &str, document: &Value, member: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the answer to carry {member:?}, got {document}")
        })
        .to_string()
}

/// The whole rule, driven END TO END as one test: ONE real `CreateFunction` whose module body
/// starts a thread of its own, and one real `Invoke` per row over the router.
///
/// One test, one function and one stream of invocations, the way the sibling runtime suites are
/// built: the memory bound a run is stopped on is read from the WHOLE process
/// (`state::execution::python`) and this binary runs its suites in parallel, so fewer
/// interpreters in flight at once is fewer runs stopped for memory belonging to another suite.
/// A row whose run was stopped by one of those shared bounds says nothing about the rule and is
/// left out of the reading — but not every one of them can be: each table answers whether a RUN
/// answered it, and a test that read none of them asserted nothing, so it fails rather than
/// passing silently.
#[tokio::test]
async fn the_python_runtime_runs_the_threads_a_function_starts() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let probe = "probe-guest-threads";
    create_function(&router, probe, PROBE_SOURCE).await;
    let mut read = 0;
    read += usize::from(a_thread_the_code_starts_runs_its_own_body(&router, probe).await);
    read += usize::from(a_thread_that_cannot_make_progress_does_not_hold_the_invocation(
        &router, probe,
    )
    .await);
    read += usize::from(a_start_past_what_one_invocation_may_make_is_refused(&router, probe).await);
    assert!(
        read > 0,
        "expected at least one of the tables above to be answered by a run, got {read}: every \
         invocation reported a bound this whole test binary shares, so nothing of the rule was \
         read"
    );
}

/// The recorded shape and its generalisation: the bodies of the threads the code starts RUN.
///
/// The rows differ along which interface started the thread, how its body was given its
/// arguments, how many threads were started, where the start was made and what the body did.
///
/// Answers whether a run answered this table.
async fn a_thread_the_code_starts_runs_its_own_body(
    router: &MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "the bodies of the threads a function starts";
    let Some(document) = answered(router, probe, case, &json!({"case": "ran"})).await else {
        return false;
    };
    // (the row, the value the answer must carry under `marks`)
    let rows: &[(&str, &str)] = &[
        ("a thread joined for its own work", "joined"),
        ("a thread given positional arguments", "positional"),
        ("a thread given keyword arguments", "positional+keyword"),
        ("the low-level interface's own start", "low-level"),
        ("one of several threads started at once", "several-0"),
        ("another of them", "several-3"),
        ("a thread whose body then raised", "raising"),
        // The module BODY's own thread, which is where an internal extension registers: it ran
        // while the execution environment was being initialized, before any invocation.
        ("a thread the module body started", "at-import"),
    ];
    let marks = document
        .get("marks")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected the answer to carry marks, got {document}"));
    let marks: Vec<&str> = marks.iter().filter_map(Value::as_str).collect();
    for (row, mark) in rows {
        assert!(
            marks.contains(mark),
            "case {case:?} / {row}: expected the thread's own body to have left {mark:?} behind, \
             got {marks:?}"
        );
    }
    // The value a joined thread computed is the value its own code computed.
    assert_eq!(
        document.get("computed").and_then(Value::as_i64),
        Some(4900),
        "case {case:?}: expected the joined thread's own sum, got {document}"
    );
    // A thread that has run is no longer alive, and joining one that has run returns.
    for member in ["alive_after_join", "alive_after_raising"] {
        assert_eq!(
            document.get(member).and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected {member} to be false, got {document}"
        );
    }
    // The failure of a thread's body belongs to that thread: the handler went on and answered.
    assert_eq!(
        text(case, &document, "after_raising"),
        "the handler went on",
        "case {case:?}: a thread's own failure is not the invocation's, got {document}"
    );
    // Each body ran as a thread of its own, identified apart from the one the handler runs on.
    let idents = document
        .get("idents")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected the answer to carry idents, got {document}"));
    let main = document.get("main_ident").and_then(Value::as_i64);
    for ident in idents {
        assert_ne!(
            ident.as_i64(),
            main,
            "case {case:?}: expected a body to run as a thread of its own, not as the handler's \
             own {main:?}, got {idents:?}"
        );
    }
    true
}

/// RULE: a thread whose body waits for something only another thread of the same invocation
/// could produce does not hold the invocation — it is ended by the span one body may take, and
/// the handler goes on and answers.
///
/// The row is the recorded shape: an internal extension registers at
/// `/2020-01-01/extension/register` and drains `/2020-01-01/extension/event/next` on a thread
/// of its own, which is a long poll no event this invocation dispatched ever answers. What the
/// surface BEHIND that thread then answers — the Telemetry API's own subscribe — is read in the
/// same run, because it is what such a handler goes on to do.
///
/// Answers whether a run answered this table.
async fn a_thread_that_cannot_make_progress_does_not_hold_the_invocation(
    router: &MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "a thread draining the extension event pump";
    let Some(document) = answered(router, probe, case, &json!({"case": "pump"})).await else {
        return false;
    };
    assert_eq!(
        document
            .get("registered")
            .and_then(|answer| answer.get("status"))
            .and_then(Value::as_i64),
        Some(200),
        "case {case:?}: expected the extension to register, got {document}"
    );
    assert_eq!(
        document
            .get("subscribed")
            .and_then(|answer| answer.get("status"))
            .and_then(Value::as_i64),
        Some(200),
        "case {case:?}: expected the telemetry subscribe to be answered, got {document}"
    );
    assert_eq!(
        document
            .get("subscribed")
            .and_then(|answer| answer.get("body"))
            .and_then(Value::as_str),
        Some("\"OK\""),
        "case {case:?}: expected the subscribe body the Telemetry API answers, got {document}"
    );
    // The pump's own thread was started and is over: its body ran, and what it was waiting for
    // was never produced, so the handler is what answered.
    assert_eq!(
        document.get("pump_started").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: expected the draining thread to have been started, got {document}"
    );
    assert_eq!(
        document.get("pump_alive").and_then(Value::as_bool),
        Some(false),
        "case {case:?}: expected the draining thread to be over, got {document}"
    );
    true
}

/// RULE: a start the platform cannot serve is refused BY THE START, with the error Python
/// raises for a thread that could not be started — and the threads that did start ran.
///
/// Answers whether a run answered this table.
async fn a_start_past_what_one_invocation_may_make_is_refused(
    router: &MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "more threads than one invocation may start";
    // Asked for far more than one invocation may start, so the row reads the refusal rather
    // than the count: how many an invocation may start is this host's own bound.
    let Some(document) = answered(
        router,
        probe,
        case,
        &json!({"case": "many", "threads": 300}),
    )
    .await
    else {
        return false;
    };
    let started = document
        .get("started")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected the answer to carry started, got {document}"));
    assert!(
        started > 0,
        "case {case:?}: expected the starts this invocation may make to have been served, got \
         {document}"
    );
    assert_eq!(
        document.get("ran").and_then(Value::as_i64),
        Some(started),
        "case {case:?}: expected every thread that started to have run its body, got {document}"
    );
    assert_eq!(
        text(case, &document, "refused"),
        "RuntimeError: can't start new thread",
        "case {case:?}: expected the start itself to be refused as Python refuses one, got \
         {document}"
    );
    true
}

/// The probe every row drives: its module BODY starts a thread of its own, and its handler
/// answers what the case the event names came to.
const PROBE_SOURCE: &str = r#"
import json
import os
import threading
import urllib.error
import urllib.request

import _thread

# What the bodies of this run's threads have left behind, and the threads they ran as.
_MARKS = []
_IDENTS = []


def _mark(value, mark=""):
    """The body of a thread: it notes what it was given and which thread it ran as."""
    _MARKS.append(value + mark)
    _IDENTS.append(_thread.get_ident())


# A thread started by the MODULE BODY, which is where an internal extension registers and
# starts draining: it runs while the execution environment is being initialized.
threading.Thread(target=_mark, args=("at-import",), daemon=True).start()


def _sent(method, path, body=None, headers=None):
    """One request to the runtime API the environment names, and what it answered."""
    address = os.environ["AWS_LAMBDA_RUNTIME_API"]
    request = urllib.request.Request(
        "http://" + address + path,
        data=body.encode("utf-8") if body is not None else None,
        method=method,
    )
    for name, value in (headers or {}).items():
        request.add_header(name, value)
    try:
        with urllib.request.urlopen(request) as answer:
            return {
                "status": answer.status,
                "body": answer.read().decode("utf-8"),
                "identifier": answer.headers.get("Lambda-Extension-Identifier"),
            }
    except urllib.error.HTTPError as error:
        return {
            "status": error.code,
            "body": error.read().decode("utf-8"),
            "identifier": error.headers.get("Lambda-Extension-Identifier"),
        }
    except Exception as error:
        return {"status": 0, "body": type(error).__name__ + ": " + str(error)}


def _ran(event):
    """Threads started every way the interface offers, and what their bodies did."""
    computed = []

    def _sum():
        computed.append(sum(index * index for index in range(25)))
        _mark("joined")

    joined = threading.Thread(target=_sum)
    joined.start()
    joined.join()
    threading.Thread(target=_mark, args=("positional",)).start()
    threading.Thread(target=_mark, args=("positional",), kwargs={"mark": "+keyword"}).start()
    _thread.start_new_thread(_mark, ("low-level",))
    for index in range(4):
        threading.Thread(target=_mark, args=("several-%d" % index,)).start()

    def _raises():
        _mark("raising")
        raise ValueError("this thread's own failure")

    raising = threading.Thread(target=_raises)
    raising.start()
    raising.join()
    return {
        "marks": list(_MARKS),
        "idents": list(_IDENTS),
        "main_ident": _thread.get_ident(),
        "computed": computed[0] if computed else None,
        "alive_after_join": joined.is_alive(),
        "alive_after_raising": raising.is_alive(),
        "after_raising": "the handler went on",
    }


def _pump(event):
    """An internal extension that registers and drains the event pump on its own thread."""
    registered = _sent(
        "POST",
        "/2020-01-01/extension/register",
        body=json.dumps({"events": ["INVOKE", "SHUTDOWN"]}),
        headers={"Lambda-Extension-Name": "lambda_function.py"},
    )
    identifier = registered.get("identifier") or ""
    drained = []

    def _drain():
        while True:
            drained.append(
                _sent(
                    "GET",
                    "/2020-01-01/extension/event/next",
                    headers={"Lambda-Extension-Identifier": identifier},
                )
            )

    pump = threading.Thread(target=_drain, daemon=True)
    started = True
    try:
        pump.start()
    except BaseException as error:
        started = type(error).__name__ + ": " + str(error)
    subscribed = _sent(
        "PUT",
        "/2022-07-01/telemetry",
        body=json.dumps(
            {
                "schemaVersion": "2022-12-13",
                "types": ["platform", "function", "extension"],
                "destination": {
                    "protocol": "HTTP",
                    "URI": "http://sandbox.localdomain:8080/x",
                },
            }
        ),
        headers={
            "Lambda-Extension-Identifier": identifier,
            "Content-Type": "application/json",
        },
    )
    return {
        "registered": registered,
        "subscribed": subscribed,
        "pump_started": started,
        "pump_alive": pump.is_alive(),
        "drained": len(drained),
    }


def _many(event):
    """More threads than one invocation may start, and what the start that could not be
    served raised."""
    ran = []
    started = 0
    refused = ""
    for _ in range(int(event.get("threads", 300))):
        try:
            threading.Thread(target=ran.append, args=(1,)).start()
        except BaseException as error:
            refused = type(error).__name__ + ": " + str(error)
            break
        started += 1
    return {"started": started, "ran": len(ran), "refused": refused}


_CASES = {"ran": _ran, "pump": _pump, "many": _many}


def handler(event, context):
    return _CASES[event["case"]](event)
"#;
