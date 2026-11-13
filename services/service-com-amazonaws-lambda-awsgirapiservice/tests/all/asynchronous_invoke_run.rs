// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ASYNCHRONOUS invoke data plane: `Invoke` with
//! `InvocationType=Event` (`POST /2015-03-31/functions/{FunctionName}/invocations`) and
//! `InvokeAsync` (`POST /2014-11-13/functions/{FunctionName}/invoke-async`).
//!
//! The rule under test: an asynchronous invocation Lambda ACCEPTS is DELIVERED to the
//! function — the accepting call answers only the queueing receipt ("A success response
//! (202 Accepted) indicates that the request is queued for invocation"), and the handler of
//! the deployment package the store holds for the resolved version is then run with the
//! event, so what the handler does becomes observable after the queueing delay rather than
//! never.
//!
//! No modelled Lambda operation reports an accepted asynchronous invocation back out — AWS
//! exposes none — so each row reads the attempt's own outcome out of the service's state,
//! which is where the delivery lands. The handler used by the table raises ONLY when its
//! event asks it to, so an outcome of "the handler returned" versus "the handler raised" is
//! evidence both that the guest code ran and that it was handed this call's event.
//!
//! The rows differ along what the rule generalises over: which operation accepted the
//! invocation, what the event carried, which qualifier the call addressed (the unpublished
//! version, a published version, an alias), and what the function's runtime is.
//!
//! Every wait here is for a CONDITION, looked at again until it holds — the function
//! reporting `Active`, the delivery attempt having settled — and no row bounds the delivery
//! as a whole by a span: an attempt that found no capacity to run the function ran nothing, so
//! the event is attempted AGAIN for as long as it is retained, and how many such attempts a
//! loaded host costs an event is not a row's to bound. What a row bounds is the emulator going
//! quiet about the event ([`NO_PROGRESS_CEILING`], derived from the bounds the emulator itself
//! puts on one attempt and on the wait between two), and every limit no row is about — the
//! `Timeout` a handler is given above all — is far above what the work takes. So how fast the
//! host runs decides nothing a row asserts: a slow host is waited for, and only a delivery
//! nothing is working on fails a row.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ABANDON_GRACE, FUNCTION_ERROR_UNHANDLED, HOST_WAIT_CEILING, INIT_PHASE_LIMIT, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, AsyncInvocationRecord, ComAmazonawsLambdaAwsgirapiserviceState,
    ASYNC_DELIVERY_DELAY, ASYNC_RETRY_DELAY, THROTTLED_MAX_RETRY,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// Waiting for a function to report `Active` is the same wait every suite over the invoke
// planes makes, so this one makes it with the helper those suites publish rather than with a
// sleep of its own: it looks at `GetFunctionConfiguration` until the state the clock owes is
// the one reported.
use crate::event_source_poll_authorization::wait_until_active;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The longest ONE delivery attempt can take, which is the sum of the spans the EMULATOR
/// itself bounds an attempt by: the waits for the two pools a run claims (the mounts it
/// inflates and its share of the run memory, each bounded by `PERMIT_WAIT`), the
/// initialization of the execution environment the handler is called in and the handler's
/// own run, plus the grace a run past its budget is still waited for.
const ONE_ATTEMPT_CEILING: Duration = PERMIT_WAIT
    .saturating_mul(2)
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(ABANDON_GRACE);

/// How long a row waits with NOTHING HAPPENING to the delivery it accepted before it is a
/// failure.
///
/// Not a budget for the delivery as a whole, because the emulator does not bound one: an
/// attempt that found no capacity to run the function RAN nothing, so "Lambda returns the
/// event to the queue and attempts to run the function again" for as long as the event is
/// retained — six hours by default — and how many such attempts a loaded host costs one event
/// is not a row's to bound. What a row CAN bound is the emulator going QUIET about the event:
/// one attempt is bounded by [`ONE_ATTEMPT_CEILING`], the waits the retry policy puts between
/// two attempts of one event reach [`THROTTLED_MAX_RETRY`] and no further, and the first
/// attempt falls due [`ASYNC_DELIVERY_DELAY`] after the accepting call. So an event that
/// neither settles nor has another attempt made for it inside this span is one nothing is
/// working on — which is the only thing a row here can call a failure, and is what
/// [`settled_newest`] waits on. A row whose attempts keep being made is waited for however
/// many of them the host costs it.
const NO_PROGRESS_CEILING: Duration = ASYNC_DELIVERY_DELAY
    .saturating_add(ONE_ATTEMPT_CEILING)
    .saturating_add(ASYNC_RETRY_DELAY)
    .saturating_add(THROTTLED_MAX_RETRY);

/// The `Timeout` every function below is created with: the longest a handler of it may run.
///
/// Generous on purpose, because no row here is about the timeout: a handler stopped at this
/// deadline is reported as a run that ERRORED, so a tight value would turn a slow host into
/// a `FunctionError` the handler never raised. Minutes for handlers that return in
/// milliseconds, and well inside the range the model allows the member
/// (`smithy_query(query="shape", shape="Timeout")`: 1 to 5400).
const GENEROUS_TIMEOUT_SECONDS: u32 = 300;

/// How often a row looks at the entry while it waits for the attempt.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The python package's handler, as `Handler` names it (`module.attribute`).
pub(crate) const PYTHON_HANDLER: &str = "lambda_function.lambda_handler";

/// The node package's handler, as `Handler` names it (`module.export`).
const NODE_HANDLER: &str = "index.handler";

/// A python handler that returns what it was handed, and raises exactly when the event asks
/// it to — so the outcome recorded for a delivery says whether the guest ran and which event
/// it ran on.
pub(crate) const PYTHON_SOURCE: &str = "def lambda_handler(event, context):\n\
                             \x20   if isinstance(event, dict) and event.get('fail'):\n\
                             \x20       raise RuntimeError('the event asked this handler to fail')\n\
                             \x20   return {'received': event}\n";

/// The same handler for the node runtimes.
const NODE_SOURCE: &str = "exports.handler = async (event) => {\n\
                           \x20 if (event && event.fail) {\n\
                           \x20   throw new Error('the event asked this handler to fail');\n\
                           \x20 }\n\
                           \x20 return { received: event };\n\
                           };\n";

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    body: bytes::Bytes,
}

impl Answer {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
pub(crate) fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request, with the headers `headers` names.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// A .zip deployment package holding one source file, written in memory the way a caller
/// uploads one.
fn package(name: &str, source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file(name, options).expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a function whose code is `source`, publish version 1, and point the alias `live`
/// at it, so a row can address the unpublished version, a published one or an alias.
pub(crate) async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    runtime: &str,
    handler: &str,
    entry: &str,
    source: &str,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": handler,
            "Publish": true,
            "Timeout": GENEROUS_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(entry, source)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    let alias = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": "1" }))
            .expect("the alias body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias on {name:?} must succeed, got {} {}",
        alias.status,
        alias.text()
    );
    // The function is invocable once it reports `Active`, which the clock owes shortly after
    // the create. That CONDITION is what a row waits for — not a span of time — so a host
    // that takes longer to get there is waited for rather than failed.
    wait_until_active(router, name, name).await;
}

/// How the invocation this row accepts is submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accept {
    /// `Invoke` with `InvocationType: Event`.
    InvokeEvent,
    /// `InvokeAsync`, whose only invocation type is the asynchronous one.
    InvokeAsync,
}

/// Submit one asynchronous invocation of `reference` carrying `event`, the way `accept`
/// names, and answer what the accepting call reported.
pub(crate) async fn accept_invocation(
    router: &MultiServiceRouter,
    accept: Accept,
    reference: &str,
    event: &[u8],
) -> Answer {
    match accept {
        Accept::InvokeEvent => {
            call(
                router,
                "POST",
                &format!("{FUNCTIONS_URI}/{reference}/invocations"),
                event,
                &[("x-amz-invocation-type", "Event")],
            )
            .await
        }
        Accept::InvokeAsync => {
            call(
                router,
                "POST",
                &format!("/2014-11-13/functions/{reference}/invoke-async"),
                event,
                &[],
            )
            .await
        }
    }
}

/// Every event this function holds, in queue order, read out of the service's own state
/// because no modelled operation reports an accepted asynchronous invocation.
pub(crate) fn held(engine: &Arc<engine::InMemoryEngine>, name: &str) -> Vec<AsyncInvocationRecord> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map(|function| function.async_invocations.iter().cloned().collect())
        .unwrap_or_default()
}

/// A deadline that starts again every time the delivery it watches makes PROGRESS.
///
/// The emulator bounds one delivery ATTEMPT and the wait between two of them, and nothing
/// else: an attempt that found no capacity to run the function is made again for as long as
/// the event is retained, so the only failure a row can state is the emulator going quiet
/// about the event. This is that statement: each look hands in how far the delivery has got,
/// and the ceiling runs from the last look that saw it further on than the one before.
struct NoProgress {
    /// How far the delivery had got when it was last seen to move.
    reached: Option<String>,
    /// When nothing having happened since becomes a failure.
    deadline: Instant,
}

impl NoProgress {
    fn new() -> Self {
        Self {
            reached: None,
            deadline: Instant::now() + NO_PROGRESS_CEILING,
        }
    }

    /// Record how far the delivery has got, and answer whether the wait may go on: `false`
    /// once nothing has happened to it for [`NO_PROGRESS_CEILING`].
    fn still_going(&mut self, reached: String) -> bool {
        if self.reached.as_ref() != Some(&reached) {
            self.reached = Some(reached);
            self.deadline = Instant::now() + NO_PROGRESS_CEILING;
        }
        Instant::now() < self.deadline
    }
}

/// Whether the delivery attempt of `event` has been decided, which is when the entry carries
/// what became of the event rather than where it is in the queue.
fn has_settled(event: &AsyncInvocationRecord) -> bool {
    !matches!(
        event.outcome,
        AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
    )
}

/// How far the delivery of `event` has got: the attempts made for it and the outcome its
/// entry carries, which is the whole of what one look can see change.
fn reached(event: &AsyncInvocationRecord) -> String {
    format!("{} attempt(s), {:?}", event.attempts, event.outcome)
}

/// The newest event held for `name`, waited for until its delivery attempt has settled.
///
/// The wait is for the delivery to make PROGRESS, not for a span of wall clock
/// ([`NoProgress`]): an event whose attempts keep being made is waited for however many of
/// them a loaded host costs it, and only an event nothing is working on fails a row. That is
/// what makes this wait independent of how fast the host runs — the spans a delivery spends
/// waiting for this process's run capacity, and the attempts a throttled event is made again
/// for, are the emulator's to choose and are simply waited for.
pub(crate) async fn settled_newest(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
) -> AsyncInvocationRecord {
    let mut progress = NoProgress::new();
    loop {
        let newest = held(engine, name).into_iter().next_back();
        if let Some(event) = newest.as_ref().filter(|event| has_settled(event)) {
            return event.clone();
        }
        let standing = newest
            .as_ref()
            .map_or_else(|| "no entry at all".to_string(), reached);
        assert!(
            progress.still_going(standing.clone()),
            "case {case:?}: nothing happened to the delivery of this event for \
             {NO_PROGRESS_CEILING:?} — no attempt was made for it and it did not settle — \
             and it stands at {standing}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every event held for `name` once `wanted` of them are there and the delivery attempt of
/// each has settled, in the order they were accepted.
///
/// The same progress-aware wait as [`settled_newest`], over the whole of a function's queue.
pub(crate) async fn settled_all(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    wanted: usize,
) -> Vec<AsyncInvocationRecord> {
    let mut progress = NoProgress::new();
    loop {
        let entries = held(engine, name);
        if entries.len() == wanted && entries.iter().all(has_settled) {
            return entries;
        }
        let standing = entries
            .iter()
            .map(reached)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            progress.still_going(standing.clone()),
            "case {case:?}: nothing happened to the {wanted} deliveries of this function for \
             {NO_PROGRESS_CEILING:?} — no attempt was made for any of them and none settled — \
             and they stand at [{standing}]"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the delivery table.
struct Delivery {
    case: &'static str,
    /// The function the row invokes, and so the runtime the handler is run by.
    function: &'static str,
    /// How the invocation is accepted.
    accept: Accept,
    /// The reference the call addresses, which decides the version the event is delivered
    /// to.
    reference: String,
    /// The event the call carries.
    event: Vec<u8>,
    /// The version label the entry must be held against: the qualifier the call resolved,
    /// which is `$LATEST` for an unqualified reference and the alias's own name for one
    /// addressed through an alias.
    version: &'static str,
    /// The `FunctionError` the run must report: `None` for a handler that returned, and
    /// `Some("Unhandled")` for one that raised.
    function_error: Option<&'static str>,
}

/// The delivery table: both accepting operations, a handler that returns and one that
/// raises, each documented qualifier form, both runtimes this emulator embeds an engine for,
/// and an event of each shape a caller sends — including the empty body, which is the
/// documented no-event invocation.
fn delivery_cases() -> Vec<Delivery> {
    vec![
        Delivery {
            case: "an Invoke Event whose handler returns",
            function: "async-python",
            accept: Accept::InvokeEvent,
            reference: "async-python".to_string(),
            event: br#"{"fail":false}"#.to_vec(),
            version: "$LATEST",
            function_error: None,
        },
        Delivery {
            case: "an Invoke Event whose event asks the handler to raise",
            function: "async-python",
            accept: Accept::InvokeEvent,
            reference: "async-python".to_string(),
            event: br#"{"fail":true}"#.to_vec(),
            version: "$LATEST",
            function_error: Some(FUNCTION_ERROR_UNHANDLED),
        },
        Delivery {
            case: "an InvokeAsync whose handler returns",
            function: "async-python",
            accept: Accept::InvokeAsync,
            reference: "async-python".to_string(),
            event: br#"{"index":1}"#.to_vec(),
            version: "$LATEST",
            function_error: None,
        },
        Delivery {
            case: "an InvokeAsync whose event asks the handler to raise",
            function: "async-python",
            accept: Accept::InvokeAsync,
            reference: "async-python".to_string(),
            event: br#"{"fail":true}"#.to_vec(),
            version: "$LATEST",
            function_error: Some(FUNCTION_ERROR_UNHANDLED),
        },
        Delivery {
            case: "an empty event, the documented no-event invocation",
            function: "async-python",
            accept: Accept::InvokeEvent,
            reference: "async-python".to_string(),
            event: Vec::new(),
            version: "$LATEST",
            function_error: None,
        },
        Delivery {
            case: "a JSON array event",
            function: "async-python",
            accept: Accept::InvokeEvent,
            reference: "async-python".to_string(),
            event: br#"[1,2,3]"#.to_vec(),
            version: "$LATEST",
            function_error: None,
        },
        Delivery {
            case: "an Invoke Event addressed to a published version",
            function: "async-python",
            accept: Accept::InvokeEvent,
            reference: "async-python:1".to_string(),
            event: br#"{"fail":true}"#.to_vec(),
            version: "1",
            function_error: Some(FUNCTION_ERROR_UNHANDLED),
        },
        Delivery {
            case: "an Invoke Event addressed through an alias",
            function: "async-python",
            accept: Accept::InvokeEvent,
            reference: "async-python:live".to_string(),
            event: br#"{"fail":false}"#.to_vec(),
            version: "live",
            function_error: None,
        },
        Delivery {
            case: "an Invoke Event of a node function whose handler returns",
            function: "async-node",
            accept: Accept::InvokeEvent,
            reference: "async-node".to_string(),
            event: br#"{"fail":false}"#.to_vec(),
            version: "$LATEST",
            function_error: None,
        },
        Delivery {
            case: "an InvokeAsync of a node function whose event asks the handler to raise",
            function: "async-node",
            accept: Accept::InvokeAsync,
            reference: "async-node".to_string(),
            event: br#"{"fail":true}"#.to_vec(),
            version: "$LATEST",
            function_error: Some(FUNCTION_ERROR_UNHANDLED),
        },
    ]
}

/// An accepted asynchronous invocation is answered with the queueing receipt and then
/// DELIVERED: the handler runs with the event the call carried, and the attempt's own
/// outcome lands on the entry the call queued.
#[tokio::test]
async fn an_accepted_asynchronous_invocation_runs_the_function_with_its_event() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "async-python",
        "python3.12",
        PYTHON_HANDLER,
        "lambda_function.py",
        PYTHON_SOURCE,
    )
    .await;
    create_function(
        &router,
        "async-node",
        "nodejs20.x",
        NODE_HANDLER,
        "index.js",
        NODE_SOURCE,
    )
    .await;

    for case in delivery_cases() {
        let accepted = accept_invocation(&router, case.accept, &case.reference, &case.event).await;
        assert_eq!(
            accepted.status,
            202,
            "case {:?}: expected the documented 202 Accepted, got {} {}",
            case.case,
            accepted.status,
            accepted.text()
        );

        let settled = settled_newest(&engine, case.case, case.function).await;
        assert!(
            settled.payload == case.event,
            "case {:?}: expected the delivery to carry the event the call did, got {:?}",
            case.case,
            String::from_utf8_lossy(&settled.payload)
        );
        assert_eq!(
            settled.version, case.version,
            "case {:?}: expected the event delivered to version {}, got {}",
            case.case, case.version, settled.version
        );
        let AsyncInvocationOutcome::Delivered { function_error } = &settled.outcome else {
            panic!(
                "case {:?}: expected the event delivered to the function and run, got {:?}",
                case.case, settled.outcome
            )
        };
        assert_eq!(
            function_error.as_deref(),
            case.function_error,
            "case {:?}: expected the run to report FunctionError {:?}, got {:?}",
            case.case,
            case.function_error,
            function_error
        );
        assert!(
            !settled.request_id.is_empty(),
            "case {:?}: expected the delivery to carry its own request id, got {:?}",
            case.case,
            settled.request_id
        );
    }
}

/// An asynchronous invocation of a function whose package this emulator cannot run reports
/// NOTHING to its caller — the answer is the queueing receipt either way — and the entry
/// records that the attempt found no handler to run, never a result no function produced.
///
/// One row per condition a run cannot start on that a caller can create: a runtime this
/// emulator embeds no engine for, and a package that is not a readable archive.
#[tokio::test]
async fn an_event_whose_function_cannot_run_records_why_rather_than_a_result() {
    let cases: &[(&str, &str, Vec<u8>, &str)] = &[
        (
            "a runtime with no engine",
            "ruby3.3",
            package("handler.rb", "def handler(event:, context:)\n  {}\nend\n"),
            "embeds an engine for",
        ),
        (
            "a package that is not an archive",
            "python3.12",
            b"not a zip archive at all".to_vec(),
            "could not be read",
        ),
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, runtime, code, expected)) in cases.iter().enumerate() {
        let name = format!("unrunnable-{index}");
        let created = call(
            &router,
            "POST",
            FUNCTIONS_URI,
            &serde_json::to_vec(&json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": runtime,
                "Handler": "handler.handler",
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(code),
                },
            }))
            .expect("the create body serializes"),
            &[],
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateFunction must succeed, got {} {}",
            created.status,
            created.text()
        );
        wait_until_active(&router, case, &name).await;

        let accepted =
            accept_invocation(&router, Accept::InvokeEvent, &name, br#"{"event":true}"#).await;
        assert_eq!(
            accepted.status,
            202,
            "case {case:?}: expected the acceptance to be unaffected by the run, got {} {}",
            accepted.status,
            accepted.text()
        );

        let settled = settled_newest(&engine, case, &name).await;
        let AsyncInvocationOutcome::Failed(reason) = &settled.outcome else {
            panic!(
                "case {case:?}: expected the attempt to report that nothing ran, got {:?}",
                settled.outcome
            )
        };
        assert!(
            reason.contains(expected),
            "case {case:?}: expected the reason to name {expected:?}, got {reason:?}"
        );
    }
}

/// Every delivery is of ONE accepted event: two acceptances are two runs, and a `Value`
/// recorded for one is never recorded for the other.
#[tokio::test]
async fn each_accepted_event_is_delivered_once_and_on_its_own() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "async-each",
        "python3.12",
        PYTHON_HANDLER,
        "lambda_function.py",
        PYTHON_SOURCE,
    )
    .await;

    // One event per outcome, accepted back to back: the first asks the handler to raise and
    // the second does not, so the two entries cannot be confused for one another.
    let events: &[(&str, &[u8], Option<&str>)] = &[
        ("the first event raises", br#"{"fail":true}"#, Some(FUNCTION_ERROR_UNHANDLED)),
        ("the second event returns", br#"{"fail":false}"#, None),
    ];
    for (case, event, _) in events {
        let accepted = accept_invocation(&router, Accept::InvokeEvent, "async-each", event).await;
        assert_eq!(
            accepted.status,
            202,
            "case {case:?}: expected 202 Accepted, got {} {}",
            accepted.status,
            accepted.text()
        );
    }

    // Both attempts settle, each on its own entry and in the order the events were accepted.
    let settled = settled_all(&engine, "two events accepted back to back", "async-each", events.len()).await;

    for (entry, (case, event, expected)) in settled.iter().zip(events) {
        assert!(
            entry.payload == *event,
            "case {case:?}: expected the entry to hold its own event, got {:?}",
            String::from_utf8_lossy(&entry.payload)
        );
        let AsyncInvocationOutcome::Delivered { function_error } = &entry.outcome else {
            panic!(
                "case {case:?}: expected the event to have been run, got {:?}",
                entry.outcome
            )
        };
        assert_eq!(
            function_error.as_deref(),
            *expected,
            "case {case:?}: expected FunctionError {expected:?}, got {function_error:?}"
        );
    }
    let mut ids: Vec<&str> = settled
        .iter()
        .map(|entry| entry.request_id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        events.len(),
        "expected each accepted event to carry its own request id, got {ids:?}"
    );
}

/// The value of an asynchronous invocation is what the HANDLER does, which a caller observes
/// through the state the handler leaves rather than through the accepting call. This row
/// drives that end to end over the one effect a contained run can have on this emulator: the
/// run's own answer, which the entry keeps.
#[tokio::test]
async fn the_accepting_call_reports_nothing_about_the_run_it_queued() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "async-silent",
        "python3.12",
        PYTHON_HANDLER,
        "lambda_function.py",
        PYTHON_SOURCE,
    )
    .await;

    // The handler raises, which an asynchronous invocation's answer says nothing about:
    // "the API response only includes a status code".
    let accepted = accept_invocation(
        &router,
        Accept::InvokeEvent,
        "async-silent",
        br#"{"fail":true}"#,
    )
    .await;
    assert_eq!(
        accepted.status,
        202,
        "expected 202 Accepted for a handler that raises, got {} {}",
        accepted.status,
        accepted.text()
    );
    assert!(
        accepted.body.is_empty(),
        "expected an empty body for an accepted event, got {:?}",
        accepted.text()
    );

    // And the entry, read once the attempt has settled, carries the run the caller was told
    // nothing about.
    let settled = settled_newest(&engine, "a handler that raises", "async-silent").await;
    assert_eq!(
        settled.outcome,
        AsyncInvocationOutcome::Delivered {
            function_error: Some(FUNCTION_ERROR_UNHANDLED.to_string()),
        },
        "expected the delivery to record the run's own FunctionError, got {:?}",
        settled.outcome
    );
}
