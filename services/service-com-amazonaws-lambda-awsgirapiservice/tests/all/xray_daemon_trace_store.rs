// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the X-RAY DAEMON a traced `python3.x` invocation reports its own work
//! to: `CreateFunction` then `Invoke` (`POST /2015-03-31/functions/{FunctionName}/invocations`)
//! over the router, with AWS Lambda and AWS X-Ray on ONE engine so a datagram a handler wrote
//! is read back where a client reads it.
//!
//! The rule under test: **a traced invocation's environment names an X-Ray daemon its own
//! datagrams reach (`AWS_XRAY_DAEMON_ADDRESS`, "the X-Ray daemon address in the format
//! `IP_ADDRESS:PORT`") and the runtime it runs on presents the standard library of the runtime
//! it advertises — so a handler module that imports the socket interface IMPORTS CLEANLY and
//! writing a datagram to that address faults nothing: the invocation answers the handler's own
//! payload with no `X-Amz-Function-Error`. Every segment document a datagram the daemon's
//! framing admits carries — a header line declaring the format, then one document per line —
//! is submitted to X-Ray under the trace the document names, so it is read back through
//! `BatchGetTraces` and selected by `GetTraceSummaries` on its own annotations; a datagram the
//! framing does not admit is DROPPED, and a datagram behind it on the same socket still
//! lands.**
//!
//! The rows differ along what the rule generalises over: the FRAMING of the datagram (one
//! document, two documents in one datagram, a header line declaring another format, a header
//! line that is not JSON, no header line at all, a body line that is not a document) and its
//! DESTINATION (the daemon's own address, and a host nothing of a run reaches). Every row's
//! datagram is written in the table's order on ONE socket of ONE invocation, so a row standing
//! behind a dropped or undeliverable one also asserts that it still landed.
//!
//! What a `TracingConfig` mode is and how the context is composed is pinned beside the rule
//! itself (`crate::xray_daemon_datagram`, and the unit tests of this service's own
//! `state::execution::trace_context`); this
//! suite is the RUN-based half for a direct `Invoke`, where the handler itself writes to the
//! daemon. The sibling suite
//! `services/service-com-amazonaws-apigateway-backplanecontrolservice/tests/all/\
//! stage_trace_context.rs` drives the same framing from a handler an API Gateway STAGE
//! invoked, where the chain begins at another service and the function segment hangs from the
//! stage's own; what is asserted ONLY here is the direct `Invoke` — the chain the invocation
//! itself starts — and that a document the daemon stored is SELECTABLE by its own annotation
//! through `GetTraceSummaries`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
// Whether a value has the form X-Ray gives a trace id (`1-<8 hex>-<24 hex>`) or a span id (16
// hex) is the ONE reading of those forms this tree has, rather than a second copy of the rules
// here.
use service_common::xray_trace::{is_span_id, is_trace_id};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::node_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The handler module every function here ships, and the handler it declares.
const MODULE: &str = "lambda_function";
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The provisioning window a
/// create documents is owed by the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The `Timeout` every function this suite creates carries, in seconds. Sized the way the
/// sibling suites that RUN handlers size theirs: it has to cover this test binary's own
/// scheduling, not the handler's work, which is immediate.
const FUNCTION_TIMEOUT_SECONDS: i32 = 25;

/// How many times the invocation is re-driven while the answer reports one of this test
/// binary's own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 6;

/// The `origin` X-Ray files the two segments of a recorded invocation under.
const SERVICE_ORIGIN: &str = "AWS::Lambda";
const FUNCTION_ORIGIN: &str = "AWS::Lambda::Function";

/// The daemon's own port: the `2000/udp` the X-Ray daemon listens on, which the address the
/// execution environment names has to end in.
const DAEMON_PORT: &str = "2000";

/// A host no run reaches, in the form the environment names the daemon in. From the address
/// block RFC 5737 reserves for documentation, so it names no host of this emulator's.
const UNREACHABLE: &str = "198.51.100.7:2000";

/// What a traced run is told to do when a tracer finds no trace context:
/// "`AWS_XRAY_CONTEXT_MISSING` – For X-Ray tracing, Lambda sets this to `LOG_ERROR`" (the
/// Lambda Developer Guide's runtime environment variables).
const CONTEXT_MISSING: &str = "LOG_ERROR";

/// How long this suite waits for a submitted document to be readable, and how often it looks.
/// The submission is the emulator's own work, so nothing here sleeps on its behalf: the wait
/// is for the CONDITION, and the deadline is generous.
const READ_BACK_WAIT: Duration = Duration::from_secs(20);
const READ_BACK_LOOK: Duration = Duration::from_millis(50);

/// How wide a window the trace search is made over, either side of this suite's own clock:
/// well inside the widest window `GetTraceSummaries` serves and the allowance its `EndTime`
/// has past the clock.
const WINDOW_BEHIND_SECONDS: f64 = 300.0;
const WINDOW_AHEAD_SECONDS: f64 = 60.0;

/// How many pages of trace summaries a query walks before the paging is called broken.
const PAGE_LIMIT: usize = 32;

/// The handler this suite runs: it reports the trace context its own environment carries and
/// writes the datagrams the event names, so the invocation's own answer is evidence of what
/// the runtime let it do.
///
/// The test writes the FRAMING of each datagram and the run fills in the chain, the span and
/// the instant it really saw (`__ROOT__`, `__PARENT__`, `__NOW__`), because those are the
/// invocation's own values and no caller knows them beforehand.
const HANDLER_SOURCE: &str = r#"
import json
import os
import socket
import time


def _filled(text, fields):
    return (
        text.replace("__ROOT__", fields.get("Root") or "")
        .replace("__PARENT__", fields.get("Parent") or "")
        .replace("__NOW__", repr(time.time()))
    )


def lambda_handler(event, context):
    raw = os.environ.get("_X_AMZN_TRACE_ID") or ""
    fields = {}
    for field in raw.split(";"):
        name, _, value = field.partition("=")
        if name:
            fields[name] = value
    daemon = os.environ.get("AWS_XRAY_DAEMON_ADDRESS") or ""
    refused = []
    # ONE socket for every datagram the event names, written in the order it names them: a
    # datagram the daemon drops must not take the ones behind it on the same socket with it.
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as writer:
        for entry in event.get("datagrams") or []:
            named = entry.get("destination") or daemon
            host, _, port = named.rpartition(":")
            datagram = _filled(entry.get("datagram") or "", fields)
            try:
                writer.sendto(datagram.encode("utf-8"), (host, int(port)))
                refused.append(None)
            except OSError as error:
                refused.append(str(error))
    return {
        "raw_trace_header": raw,
        "daemon": daemon,
        "context_missing": os.environ.get("AWS_XRAY_CONTEXT_MISSING"),
        "root": fields.get("Root"),
        "parent": fields.get("Parent"),
        "sampled": fields.get("Sampled"),
        "refused": refused,
    }
"#;

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

/// AWS Lambda and AWS X-Ray on ONE engine, as `core/router/src/lib.rs` mounts them: the
/// documents the run submits are read back from the very store this suite queries.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`. Nothing in it is
/// verified: the harness turns request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Dispatch one request to `service`'s own endpoint.
async fn dispatch(
    router: &MultiServiceRouter,
    service: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Answer {
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization(service))
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

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    dispatch(router, "lambda", method, uri, body).await
}

/// One AWS X-Ray call over its `restJson1` wire.
async fn xray(router: &MultiServiceRouter, path: &str, body: &Value) -> Answer {
    dispatch(
        router,
        "xray",
        "POST",
        path,
        &serde_json::to_vec(body).expect("the X-Ray body serializes"),
    )
    .await
}

/// A `TracingConfig` `Active` `python3.12` function running [`HANDLER_SOURCE`], waited out of
/// the provisioning window the create documents.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let entry = format!("{MODULE}.py");
    let created = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "TracingConfig": { "Mode": "Active" },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[(entry.as_str(), HANDLER_SOURCE)])),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup CreateFunction {name:?}: expected HTTP 201, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// One `Invoke` request carrying `event`.
async fn invoke_once(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await
}

/// What the invocation answered, re-driven while the answer reports a bound this whole test
/// binary shares rather than a run.
async fn answered(router: &MultiServiceRouter, name: &str, case: &str, event: &Value) -> Value {
    let mut answer = invoke_once(router, name, event).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answer.status, &answer.text()) {
            break;
        }
        answer = invoke_once(router, name, event).await;
    }
    assert!(
        !is_host_bound(answer.status, &answer.text()),
        "case {case:?}: every attempt reported a bound this whole test binary shares, so \
         nothing of the rule was read: {}",
        answer.text()
    );
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answer.status,
        answer.text()
    );
    let failure = answer.header("x-amz-function-error");
    assert_eq!(
        failure, None,
        "case {case:?}: the handler's module imports the socket interface and writes its \
         datagrams, so the invocation carries no function error, got {failure:?} with {}",
        answer.text()
    );
    answer.json()
}

/// Every segment document X-Ray holds for `trace_id`, read through `BatchGetTraces` — the
/// operation a client reads a trace back with (`POST /Traces`).
async fn documents_of(router: &MultiServiceRouter, trace_id: &str) -> Vec<Value> {
    let answered = xray(router, "/Traces", &json!({ "TraceIds": [trace_id] })).await;
    assert_eq!(
        answered.status,
        200,
        "expected HTTP 200 from BatchGetTraces, got {} {}",
        answered.status,
        answered.text()
    );
    answered
        .json()
        .get("Traces")
        .and_then(Value::as_array)
        .map(|traces| {
            traces
                .iter()
                .flat_map(|trace| {
                    trace
                        .get("Segments")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|segment| {
                    segment
                        .get("Document")
                        .and_then(Value::as_str)
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `name` of every document in `documents`.
fn names_of(documents: &[Value]) -> BTreeSet<String> {
    documents
        .iter()
        .filter_map(|document| document.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The documents of `trace_id` once they hold every name in `expected`, looking again until
/// they do or the deadline passes — the submission is the emulator's own work, so the wait is
/// for the CONDITION rather than for a span of the clock.
async fn documents_holding(
    router: &MultiServiceRouter,
    trace_id: &str,
    expected: &[&str],
) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + READ_BACK_WAIT;
    loop {
        let documents = documents_of(router, trace_id).await;
        let names = names_of(&documents);
        let holds_segments = documents.iter().any(|document| {
            document.get("origin").and_then(Value::as_str) == Some(FUNCTION_ORIGIN)
        });
        if holds_segments && expected.iter().all(|name| names.contains(*name)) {
            return documents;
        }
        if tokio::time::Instant::now() >= deadline {
            return documents;
        }
        tokio::time::sleep(READ_BACK_LOOK).await;
    }
}

/// Which traces `expression` selects over a window around this suite's own clock, walking
/// every page of the selection (`POST /TraceSummaries`).
async fn selected(router: &MultiServiceRouter, expression: &str) -> BTreeSet<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let mut found = BTreeSet::new();
    let mut token: Option<String> = None;
    for _ in 0..PAGE_LIMIT {
        let mut body = json!({
            "StartTime": now - WINDOW_BEHIND_SECONDS,
            "EndTime": now + WINDOW_AHEAD_SECONDS,
            "FilterExpression": expression,
        });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let answered = xray(router, "/TraceSummaries", &body).await;
        assert_eq!(
            answered.status,
            200,
            "expected HTTP 200 from GetTraceSummaries for {expression:?}, got {} {}",
            answered.status,
            answered.text()
        );
        let page = answered.json();
        for summary in page
            .get("TraceSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(id) = summary.get("Id").and_then(Value::as_str) {
                found.insert(id.to_string());
            }
        }
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return found;
        }
    }
    panic!("GetTraceSummaries kept issuing a NextToken past {PAGE_LIMIT} pages");
}

/// One row of the framing table: ONE datagram the handler writes, and what X-Ray must hold
/// for it afterwards.
///
/// Every row's datagram is written on the SAME socket, in the order this table states them,
/// so a row standing behind a dropped one also asserts the drop did not take it along.
struct Row {
    case: &'static str,
    /// Where the handler addresses it: the daemon's own address when `None`.
    destination: Option<&'static str>,
    /// Its bytes, as the handler writes them — the FRAMING under test. `__ROOT__`,
    /// `__PARENT__` and `__NOW__` are the run's own values, and `__TAG__` is the annotation
    /// value naming this row.
    datagram: &'static str,
    /// Whether the address took it: `Some(false)` for one the daemon's own address accepted,
    /// and `None` where what the local call reports is not the rule's business — a datagram no
    /// address delivers is undelivered whether the write raised or was silently lost, and what
    /// the rule states for it is that nothing of it reaches the store and the run does not
    /// fault.
    refused: Option<bool>,
    /// The `name` of every document this row's datagram must leave in the store.
    stored: &'static [&'static str],
    /// The `name` of every document it must NOT leave there.
    dropped: &'static [&'static str],
}

/// One segment document, as an instrumented handler writes one: the chain, the span and the
/// instant are the run's own, and the annotation names the row that sent it.
macro_rules! document {
    ($id:literal, $name:literal) => {
        concat!(
            r#"{"trace_id":"__ROOT__","id":""#,
            $id,
            r#"","name":""#,
            $name,
            r#"","type":"subsegment","parent_id":"__PARENT__","#,
            r#""namespace":"aws","start_time":__NOW__,"#,
            r#""end_time":__NOW__,"#,
            r#""annotations":{"probe_tag":"__TAG__"}}"#,
        )
    };
}

const ROWS: &[Row] = &[
    Row {
        case: "a well-formed datagram carrying one document",
        destination: None,
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa00000000000001", "probe-one")
        ),
        refused: Some(false),
        stored: &["probe-one"],
        dropped: &[],
    },
    Row {
        case: "a well-formed datagram carrying two documents",
        destination: None,
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa00000000000002", "probe-first"),
            "\n",
            document!("aa00000000000003", "probe-second")
        ),
        refused: Some(false),
        stored: &["probe-first", "probe-second"],
        dropped: &[],
    },
    Row {
        case: "a datagram whose header line declares another format",
        destination: None,
        datagram: concat!(
            r#"{"format":"garbage"}"#,
            "\n",
            document!("aa00000000000004", "probe-malformed-header")
        ),
        refused: Some(false),
        stored: &[],
        dropped: &["probe-malformed-header"],
    },
    Row {
        case: "a well-formed datagram behind the one whose header declared another format, on \
               the same socket",
        destination: None,
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa00000000000005", "probe-behind-malformed")
        ),
        refused: Some(false),
        stored: &["probe-behind-malformed"],
        dropped: &[],
    },
    Row {
        case: "a datagram whose header line is not JSON at all",
        destination: None,
        datagram: concat!(
            "not-a-header",
            "\n",
            document!("aa00000000000006", "probe-unparseable-header")
        ),
        refused: Some(false),
        stored: &[],
        dropped: &["probe-unparseable-header"],
    },
    Row {
        case: "a well-formed datagram behind the one whose header was not JSON, on the same \
               socket",
        destination: None,
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa00000000000007", "probe-behind-unparseable")
        ),
        refused: Some(false),
        stored: &["probe-behind-unparseable"],
        dropped: &[],
    },
    Row {
        case: "a datagram with no header line at all",
        destination: None,
        datagram: document!("aa00000000000008", "probe-unframed"),
        refused: Some(false),
        stored: &[],
        dropped: &["probe-unframed"],
    },
    Row {
        case: "a datagram whose body carries a line that is not a document, beside one that is",
        destination: None,
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa00000000000009", "probe-beside-junk"),
            "\n",
            "not-a-document"
        ),
        refused: Some(false),
        stored: &["probe-beside-junk"],
        dropped: &[],
    },
    Row {
        case: "a well-formed datagram addressed somewhere other than the daemon",
        // The ONE destination a run can deliver to is the daemon the environment names, so
        // this address delivers nothing. Whether the local `sendto` raises (as it does in this
        // service's own `state::execution::xray_daemon`) or is silently lost (as a host with a
        // route to a documentation address would) is not asserted: the rule is that the
        // datagram reaches no store and the run is not faulted by it.
        destination: Some(UNREACHABLE),
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa0000000000000a", "probe-elsewhere")
        ),
        refused: None,
        stored: &[],
        dropped: &["probe-elsewhere"],
    },
    Row {
        case: "a well-formed datagram behind the undeliverable one, on the same socket",
        destination: None,
        datagram: concat!(
            r#"{"format":"json","version":1}"#,
            "\n",
            document!("aa0000000000000b", "probe-behind-undeliverable")
        ),
        refused: Some(false),
        stored: &["probe-behind-undeliverable"],
        dropped: &[],
    },
];

/// The whole rule, driven END TO END: one real `CreateFunction`, one real `Invoke` that writes
/// the whole framing table on one socket, and real X-Ray reads — all over the router.
///
/// ONE invocation for the whole table, the way the sibling suite that drives the same framing
/// through a deployed stage does: the memory bound a run is stopped on is read from the WHOLE
/// process (`state::execution::python`) and this binary runs its suites in parallel, so one
/// interpreter for ten datagrams is nine runs this suite does not cost the rest of the target.
/// An answer reporting one of those shared bounds says nothing about the rule, so the request
/// is made again; it is never SKIPPED, because a suite that read nothing asserted nothing.
#[tokio::test]
async fn a_traced_invocations_datagram_reaches_the_trace_store_through_the_daemon() {
    let _stream = one_invocation_stream_at_a_time().await;
    let router = emulator();
    let function = "xray-daemon-trace-store";
    create_function(&router, function).await;
    // The annotation value naming each row, so what one row's datagram left in the store is
    // told from what another's did.
    let tags: Vec<String> = (0..ROWS.len()).map(|at| format!("probe-tag-{at}")).collect();
    let datagrams: Vec<Value> = ROWS
        .iter()
        .zip(&tags)
        .map(|(row, tag)| {
            json!({
                "datagram": row.datagram.replace("__TAG__", tag),
                "destination": row.destination,
            })
        })
        .collect();
    let document = answered(
        &router,
        function,
        "the framing table",
        &json!({ "datagrams": datagrams }),
    )
    .await;
    // The environment of a traced invocation names a daemon and a chain of the documented
    // forms: `<ipv4>:2000`, and `Root=<trace id>;Parent=<span>;Sampled=1`.
    let daemon = document
        .get("daemon")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (address, port) = daemon.rsplit_once(':').unwrap_or((&daemon, ""));
    assert_eq!(
        port, DAEMON_PORT,
        "expected the daemon address to name the daemon's own port, got {daemon:?}"
    );
    assert!(
        address.split('.').count() == 4
            && address.split('.').all(|octet| octet.parse::<u8>().is_ok()),
        "expected an IPv4 address in the daemon address, got {daemon:?}"
    );
    assert_eq!(
        document.get("context_missing").and_then(Value::as_str),
        Some(CONTEXT_MISSING),
        "expected the context-missing strategy a traced run is given, got {document}"
    );
    let root = document
        .get("root")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parent = document
        .get("parent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_trace_id(&root),
        "expected a trace id in the run's own context, got {document}"
    );
    assert!(
        is_span_id(&parent),
        "expected a 64-bit span in the run's own context, got {document}"
    );
    assert_eq!(
        document.get("sampled").and_then(Value::as_str),
        Some("1"),
        "a traced invocation is recorded, so its context is sampled, got {document}"
    );
    assert_eq!(
        document.get("raw_trace_header").and_then(Value::as_str),
        Some(format!("Root={root};Parent={parent};Sampled=1").as_str()),
        "expected the context's own three fields, got {document}"
    );
    // Each datagram was taken or not as its destination decides, and an address that takes
    // none is reported to the handler rather than faulting the run.
    let reports = document
        .get("refused")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reports.len(),
        ROWS.len(),
        "expected one report per datagram, got {document}"
    );
    for (at, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let Some(refused) = row.refused else {
            continue;
        };
        let reported = reports.get(at).unwrap_or(&Value::Null);
        assert_eq!(
            !reported.is_null(),
            refused,
            "case {case:?}: expected the datagram to be refused={refused}, got {reported}"
        );
    }
    // What X-Ray HOLDS for the chain: the two segments the invocation itself records, every
    // document a datagram the framing admits carried, and nothing a dropped one did.
    let expected: Vec<&str> = ROWS.iter().flat_map(|row| row.stored).copied().collect();
    let held = documents_holding(&router, &root, &expected).await;
    let names = names_of(&held);
    for row in ROWS {
        let case = row.case;
        for stored in row.stored {
            assert!(
                names.contains(*stored),
                "case {case:?}: expected X-Ray to hold the document {stored:?} the daemon was \
                 sent, got {held:?}"
            );
        }
        for dropped in row.dropped {
            assert!(
                !names.contains(*dropped),
                "case {case:?}: expected the daemon to drop {dropped:?}, got {held:?}"
            );
        }
    }
    let origins: BTreeSet<&str> = held
        .iter()
        .filter_map(|held| held.get("origin").and_then(Value::as_str))
        .collect();
    assert!(
        origins.contains(SERVICE_ORIGIN) && origins.contains(FUNCTION_ORIGIN),
        "expected the two segments a recorded invocation produces, got {held:?}"
    );
    // The function segment is the span the handler saw as its own `Parent`, and every document
    // it sent hangs from it.
    let function_segment = held
        .iter()
        .find(|held| held.get("origin").and_then(Value::as_str) == Some(FUNCTION_ORIGIN))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        function_segment.get("id").and_then(Value::as_str),
        Some(parent.as_str()),
        "expected the function segment to be the span the handler saw as its Parent, got \
         {held:?}"
    );
    for row in ROWS {
        let case = row.case;
        for stored in row.stored {
            let submitted = held
                .iter()
                .find(|held| held.get("name").and_then(Value::as_str) == Some(*stored))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                submitted.get("parent_id").and_then(Value::as_str),
                Some(parent.as_str()),
                "case {case:?}: expected {stored:?} to hang from the function segment, got \
                 {held:?}"
            );
        }
    }
    // And a document that landed is SELECTABLE by what it annotated, which is how a client
    // finds the chain it belongs to; one that never landed annotated nothing.
    for (row, tag) in ROWS.iter().zip(&tags) {
        let case = row.case;
        let landed = !row.stored.is_empty();
        let by_annotation = selected(&router, &format!("annotation.probe_tag = \"{tag}\"")).await;
        assert_eq!(
            by_annotation.contains(&root),
            landed,
            "case {case:?}: expected the chain {root:?} to be selected by the annotation \
             {tag:?}={landed}, got {by_annotation:?}"
        );
    }
    let by_other = selected(&router, "annotation.probe_tag = \"probe-tag-never-recorded\"").await;
    assert!(
        !by_other.contains(&root),
        "expected no chain to be selected by an annotation value nothing recorded, got \
         {by_other:?}"
    );
}
