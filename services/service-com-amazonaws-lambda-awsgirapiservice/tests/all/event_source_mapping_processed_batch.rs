// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an event source mapping on an Amazon SQS queue CONSUMES once
//! its function has actually run the batch it was handed.
//!
//! `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda Event
//! Function. Lambda reads items from the event source and invokes the function", and the
//! rule under test, in one sentence: a message leaves the queue when the invocation it was
//! handed to processed it, and stays on the queue when it did not — so a batch an invocation
//! accepted whole is deleted whole, a batch whose function errored is deleted not at all,
//! and for a mapping whose `FunctionResponseTypes` asks for its function's report of which
//! records failed (`ReportBatchItemFailures`) exactly the records that report names stay
//! behind and come back when their visibility timeout lapses.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with
//! Lambda and Amazon SQS on ONE engine, as `core/router` assembles them: the queue is
//! created and written through Amazon SQS's own surface, the function carries a REAL
//! deployment package whose handler decides each record's fate from the record itself, and
//! what the row asserts is the queue's own message counts — so a batch nothing processed, or
//! one deleted without a function having run it, fails here.
//!
//! The rows differ along what the rule generalises over: whether the mapping asks for the
//! failure report, how many records of the batch the function reports as failed (none, one,
//! all), how it ends (a return, a raise), and how many messages are waiting. Each row runs a
//! real interpreter, so the table spans the class with as few of them as the class takes.
//!
//! The second table is the counterpart where NO invocation runs the batch at all: its rows
//! differ along WHY the function cannot be run — a package whose bytes are no archive, and a
//! runtime this emulator embeds no engine for — and along how many messages are waiting, and
//! each asserts the mapping reports the attempt it failed to make and consumes nothing.
//!
//! Nothing here waits a span of its own choosing: every state a row needs is waited for by
//! looking again until it is there (the function reporting `Active`, the mapping reporting
//! `Enabled`, the attempt having been recorded), and every pass of the poller is made by the
//! row and awaited, so no assertion depends on how fast the host runs.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    ABANDON_GRACE, HOST_WAIT_CEILING, INIT_PHASE_LIMIT, PERMIT_WAIT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_function_active, await_mapping_state, call, call_sqs, create_mapping, create_queue,
    get_mapping, messages_held, queue_message_counts, router_on, DrivenPolling, LOOK_AGAIN,
};
use crate::synchronous_invoke_run::package;

/// How long the queues below hide a message they hand over for, in seconds — the span a
/// message the invocation did not process is in flight for before the next pass may take it
/// again.
const VISIBILITY_TIMEOUT_SECONDS: u64 = 30;

/// How long a row gives the chain before it is a failure.
///
/// A CEILING on patience, derived from what the EMULATOR itself allows the work rather than
/// picked by hand: an invocation waits for its share of this process's run capacity (the two
/// pools a run claims, each bounded by [`PERMIT_WAIT`]), initializes an execution environment
/// ([`INIT_PHASE_LIMIT`]), runs the handler ([`HOST_WAIT_CEILING`]) and is still waited for
/// after its budget ([`ABANDON_GRACE`]) — and a pass that found no capacity at all ran nothing
/// and left its batch in flight, so the row is past it once that hold lapses and the next pass
/// takes the batch again. Those spans are shared with every other suite of a whole-workspace
/// run, so only a chain that never gets there fails a row.
const DEADLINE: Duration = PERMIT_WAIT
    .saturating_mul(2)
    .saturating_add(INIT_PHASE_LIMIT)
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(ABANDON_GRACE)
    .saturating_add(Duration::from_secs(VISIBILITY_TIMEOUT_SECONDS))
    .saturating_mul(2);

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A `Runtime` this emulator embeds an engine for, and the handler the archives below expose
/// (`crate::synchronous_invoke_run::package` writes `lambda_function.py`).
const RUNNING_RUNTIME: &str = "python3.12";
/// See [`RUNNING_RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// A `Runtime` the model names (`smithy_query(query="shape", shape="Runtime")`) that this
/// emulator embeds no engine for: it runs the Python and node runtimes, so an invocation of a
/// function carrying this one runs nothing whatever its package holds.
const UNSERVED_RUNTIME: &str = "java21";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `State` a mapping reports once it is polling its event source.
const STATE_ENABLED: &str = "Enabled";

/// The one value of the model's `FunctionResponseType` enum
/// (`smithy_query(query="shape", shape="FunctionResponseType")`).
const REPORT_BATCH_ITEM_FAILURES: &str = "ReportBatchItemFailures";

/// What `LastProcessingResult` reports for an attempt whose function could not be run at
/// all (`EventSourceMappingConfiguration$LastProcessingResult`).
const RESULT_FUNCTION_CALL_FAILED: &str = "PROBLEM: Function call failed";

/// Lambda over the same engine as Amazon SQS, with the poll passes of its mappings this
/// suite's own ([`DrivenPolling`]): a row makes one pass per batch and awaits it, so what it
/// reads afterwards is the state that pass left rather than whatever the background loop had
/// got to by the instant the row looked.
fn driven_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, DrivenPolling) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let driven = DrivenPolling::claiming(&engine, &router);
    (router, engine, driven)
}

/// Create a function carrying `runtime` whose deployment package is `package_bytes`, with a
/// `Timeout` of one second so the hold a batch's delivery takes stays short, and wait until it
/// reports `Active`.
///
/// The wait is part of the SETUP rather than a nicety: a mapping polls its source for a
/// function this emulator would invoke, which is one reporting `Active`, so a row that read
/// what a pass did before the function got there would be reading a pass that polled nothing.
/// Nothing here waits a span of its own for it — [`await_function_active`] looks again until
/// the state is there.
async fn create_function_with_package(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    runtime: &str,
    package_bytes: &[u8],
) {
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": HANDLER,
            "Timeout": 1,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package_bytes),
            },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: expected CreateFunction {name:?} to answer 201, got {} {}",
        answer.status,
        answer.text()
    );
    await_function_active(router, case, name).await;
}

/// Create a function whose handler is `source`, on a runtime this emulator runs, and wait
/// until it reports `Active`.
async fn create_runnable_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    source: &str,
) {
    create_function_with_package(router, case, name, RUNNING_RUNTIME, &package(source)).await;
}

/// `SendMessage` of `body` to the queue `url` addresses.
async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
}

/// The bodies of every message the queue `arn` names still HOLDS, read out of the queue's own
/// store — visible or in flight — so what is read does not depend on racing the poller for a
/// delivery.
fn held_bodies(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> BTreeSet<String> {
    let state = engine
        .state::<service_com_amazonaws_sqs_amazonsqs::state::ComAmazonawsSqsAmazonsqsState>();
    let store = state.lock();
    store
        .queue_by_arn(arn)
        .map(|queue| {
            queue
                .messages
                .iter()
                .map(|message| message.body.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Make poll passes until the queue `url` addresses holds `expected` messages, answering what
/// it holds, and asserting that it got there.
///
/// Each pass is awaited, so the loop turns once per pass rather than once per span of wall
/// clock: a pass whose invocation found no capacity to run the function processed nothing and
/// left its batch in flight, and the poller's own answer to that is the next pass, so the row
/// makes passes until the handler has really run the batch. [`DEADLINE`] is the backstop
/// against a chain that never gets there.
async fn poll_until_held(
    router: &MultiServiceRouter,
    driven: &mut DrivenPolling,
    case: &str,
    url: &str,
    expected: u32,
) -> (u32, u32, u32) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let counts = queue_message_counts(router, case, url).await;
        if messages_held(counts) == expected {
            return counts;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the queue to hold {expected} message(s) once the batch \
             had been processed, it still holds {} (visible, in flight, delayed) = {counts:?}",
            messages_held(counts)
        );
        driven.poll_once().await;
    }
}

/// Make poll passes until the mapping `uuid` names reports what its last processing attempt
/// came to, answering that report.
///
/// A pass records the attempt it made before it answers, so a pass that handed the batch over
/// has reported it by the time `poll_once` returns — but a pass that found nothing to hand
/// over made no attempt and so recorded none, and the poller's own answer to that is the next
/// pass. So a row waits for the ATTEMPT to have been made rather than for one pass of its own
/// counting: the loop turns once per pass, and [`DEADLINE`] is the backstop against a chain
/// that never makes one.
async fn poll_until_processing_result(
    router: &MultiServiceRouter,
    driven: &mut DrivenPolling,
    case: &str,
    uuid: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        driven.poll_once().await;
        let reading = get_mapping(router, uuid).await;
        assert_eq!(
            reading.status,
            200,
            "case {case:?}: expected GetEventSourceMapping to answer 200, got {} {}",
            reading.status,
            reading.text()
        );
        if let Some(result) = reading.string("LastProcessingResult") {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the mapping to report what its attempt at the batch came \
             to, and it reports none: {}",
            reading.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Read what the queue `url` addresses holds, and assert it is `expected`.
async fn held_now(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    expected: u32,
    label: &str,
) -> (u32, u32, u32) {
    let counts = queue_message_counts(router, case, url).await;
    assert_eq!(
        messages_held(counts),
        expected,
        "case {case:?}: {label}, expected the queue to hold {expected} message(s), it holds \
         {} (visible, in flight, delayed) = {counts:?}",
        messages_held(counts)
    );
    counts
}

/// Wait for the mapping `uuid` names to report `Enabled`, which is the state in which it
/// polls.
async fn wait_for_enabled(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let read = get_mapping(router, uuid).await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: expected GetEventSourceMapping to answer 200, got {} {}",
        read.status,
        read.text()
    );
    await_mapping_state(router, case, uuid, STATE_ENABLED).await;
}

/// One row: the handler the function runs, what the mapping asks of its response, the bodies
/// sent to the queue, and which of them must be left on it.
struct Row {
    case: &'static str,
    /// The Python handler source. Each row's handler decides a record's fate from the
    /// RECORD, never from a count or an order, so the same source serves any batch.
    handler: &'static str,
    /// Whether the mapping carries `ReportBatchItemFailures`.
    reports_failures: bool,
    /// The message bodies sent to the queue, in order.
    sent: &'static [&'static str],
    /// The bodies the queue must still hold once the batch has been processed.
    left: &'static [&'static str],
}

/// A handler that reports every record whose body says `fail` as failed, and nothing else.
/// It names those records by the `messageId` of the record it was handed, which is the
/// identifier a batch item failure report names a record by.
const REPORTING_HANDLER: &str = r#"
import json


def lambda_handler(event, context):
    failures = []
    for record in event["Records"]:
        if json.loads(record["body"]).get("fail"):
            failures.append({"itemIdentifier": record["messageId"]})
    return {"batchItemFailures": failures}
"#;

/// A handler that returns nothing at all, which is the response of a function that processed
/// its whole batch and reports no failure.
const SILENT_HANDLER: &str = r#"
def lambda_handler(event, context):
    return None
"#;

/// A handler that raises, so the invocation the batch was handed to errored.
const RAISING_HANDLER: &str = r#"
def lambda_handler(event, context):
    raise RuntimeError("this batch cannot be processed")
"#;

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a mapping with no failure report, whose function returned",
            handler: SILENT_HANDLER,
            reports_failures: false,
            sent: &[r#"{"id":1}"#, r#"{"id":2}"#, r#"{"id":3}"#],
            left: &[],
        },
        Row {
            case: "a mapping with no failure report, whose function raised",
            handler: RAISING_HANDLER,
            reports_failures: false,
            sent: &[r#"{"id":1}"#, r#"{"id":2}"#],
            left: &[r#"{"id":1}"#, r#"{"id":2}"#],
        },
        Row {
            case: "a report naming no record of the batch",
            handler: REPORTING_HANDLER,
            reports_failures: true,
            sent: &[r#"{"id":1}"#, r#"{"id":2}"#],
            left: &[],
        },
        Row {
            case: "a report naming one record of the batch",
            handler: REPORTING_HANDLER,
            reports_failures: true,
            sent: &[r#"{"id":1}"#, r#"{"id":2,"fail":true}"#, r#"{"id":3}"#],
            left: &[r#"{"id":2,"fail":true}"#],
        },
        Row {
            case: "a report naming every record of the batch",
            handler: REPORTING_HANDLER,
            reports_failures: true,
            sent: &[r#"{"id":1,"fail":true}"#, r#"{"id":2,"fail":true}"#],
            left: &[r#"{"id":1,"fail":true}"#, r#"{"id":2,"fail":true}"#],
        },
    ]
}

/// The whole rule, per row: the mapping polls the queue, the function runs the batch, and
/// exactly the messages its invocation did not process are left on the queue.
#[tokio::test]
async fn a_mapping_consumes_the_records_its_function_processed_and_leaves_the_rest() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine, mut driven) = driven_emulator();

        let function = format!("processed-batch-{index}");
        create_runnable_function(&router, case, &function, row.handler).await;
        // The queue's default visibility timeout, so a message the invocation did not
        // process stays in flight for the whole of this row rather than coming back to be
        // handed over again: what the row asserts is that it was not DELETED, which the
        // queue's own counts report whether it is visible or in flight.
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("processed-batch-{index}"),
            &json!({ "VisibilityTimeout": VISIBILITY_TIMEOUT_SECONDS.to_string() }),
        )
        .await;

        let mut request = json!({
            "FunctionName": function,
            "EventSourceArn": arn,
            "BatchSize": 10,
        });
        if row.reports_failures {
            request["FunctionResponseTypes"] = json!([REPORT_BATCH_ITEM_FAILURES]);
        }
        let created = create_mapping(&router, &request).await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
        wait_for_enabled(&router, case, &uuid).await;

        for body in row.sent {
            send_message(&router, case, &url, body).await;
        }

        let expected_left = u32::try_from(row.left.len()).expect("a small count");
        // Passes until the batch has been processed: ONE pass takes the whole batch — the
        // ceiling is above what is waiting — hands it to the function and deletes what the
        // invocation processed, all before it answers, and a pass that found no capacity to run
        // the function at all is followed by another.
        poll_until_held(&router, &mut driven, case, &url, expected_left).await;
        // And nothing more leaves on a further pass: a message the invocation processed is
        // gone for good, and one it did not is still held.
        driven.poll_once().await;
        held_now(
            &router,
            case,
            &url,
            expected_left,
            "after a further poll pass",
        )
        .await;
        let held = held_bodies(&engine, &arn);
        let expected: BTreeSet<String> = row.left.iter().map(|body| (*body).to_string()).collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected exactly the unprocessed messages to be left on the \
             queue, got {held:?}"
        );
    }
}

/// The deployment package a row below uploads.
enum Deployment {
    /// Bytes that are no .zip archive at all, so there is no package to read.
    NotAnArchive,
    /// A readable archive holding a handler, so what stops the invocation is the function's
    /// runtime rather than its bytes.
    ReadableArchive,
}

impl Deployment {
    /// The bytes `CreateFunction` uploads for it.
    fn bytes(&self) -> Vec<u8> {
        match self {
            Self::NotAnArchive => b"these bytes are not a zip archive".to_vec(),
            Self::ReadableArchive => package(SILENT_HANDLER),
        }
    }
}

/// One row of the table below: a function no invocation of can run the batch, and the
/// messages waiting for it.
struct UnrunnableRow {
    case: &'static str,
    /// The function's `Runtime`, which decides whether this emulator has an engine for it.
    runtime: &'static str,
    /// The deployment package it carries.
    deployment: Deployment,
    /// The message bodies sent to the queue, in order. Every one of them must be left.
    sent: &'static [&'static str],
}

/// The ways an invocation can come to nothing without the batch ever reaching a handler, each
/// of which the `Invoke` surface of this emulator answers as one of its own modelled errors:
/// a package whose bytes cannot be read as an archive, and a runtime it embeds no engine for.
/// The rows also differ in how many messages are waiting, since what a failed attempt
/// consumes is none of them whatever the batch holds.
fn unrunnable_rows() -> Vec<UnrunnableRow> {
    vec![
        UnrunnableRow {
            case: "a package that is not a readable archive",
            runtime: RUNNING_RUNTIME,
            deployment: Deployment::NotAnArchive,
            sent: &[r#"{"id":1}"#],
        },
        UnrunnableRow {
            case: "a readable archive on a runtime this emulator runs no engine for",
            runtime: UNSERVED_RUNTIME,
            deployment: Deployment::ReadableArchive,
            sent: &[r#"{"id":1}"#, r#"{"id":2}"#, r#"{"id":3}"#],
        },
    ]
}

/// What a mapping consumes is decided by the response of the invocation it handed the batch
/// to, so a function nothing can run the package of processes nothing: the batch stays whole
/// on the queue. The counterpart of the rows above, which the same mapping and the same queue
/// would have consumed had the function run.
#[tokio::test]
async fn a_mapping_whose_function_cannot_run_consumes_nothing() {
    for (index, row) in unrunnable_rows().iter().enumerate() {
        let case = row.case;
        let (router, engine, mut driven) = driven_emulator();

        let function = format!("unrunnable-batch-{index}");
        create_function_with_package(
            &router,
            case,
            &function,
            row.runtime,
            &row.deployment.bytes(),
        )
        .await;
        // The queue's default visibility timeout, so the batch this mapping polls is in
        // flight for the whole of the row rather than coming back to be handed over again:
        // what the row asserts is that nothing was DELETED, which the queue's own counts
        // report whether a message is visible or in flight.
        let (url, arn) = create_queue(
            &router,
            case,
            &function,
            &json!({ "VisibilityTimeout": VISIBILITY_TIMEOUT_SECONDS.to_string() }),
        )
        .await;
        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "BatchSize": 10,
                "FunctionResponseTypes": [REPORT_BATCH_ITEM_FAILURES],
            }),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
        wait_for_enabled(&router, case, &uuid).await;

        for body in row.sent {
            send_message(&router, case, &url, body).await;
        }

        // Passes until the attempt has been MADE: handing the batch to an invocation and
        // deciding what that invocation came to is a span no caller controls — the attempt
        // waits for this process's run capacity like any other — and a pass that polled
        // nothing made no attempt at all, so the row looks again rather than trusting one
        // pass of its own counting.
        //
        // "The result of the last Lambda invocation of your function"
        // (`EventSourceMappingConfiguration$LastProcessingResult`): the attempt failed, so
        // that is what the mapping reports rather than a success nothing earned.
        let reported = poll_until_processing_result(&router, &mut driven, case, &uuid).await;
        assert_eq!(
            reported, RESULT_FUNCTION_CALL_FAILED,
            "case {case:?}: expected the mapping to report the failed attempt, got \
             {reported:?}"
        );

        // The attempt has been decided, so what the queue holds now is what that attempt
        // consumed: nothing.
        let expected_held = u32::try_from(row.sent.len()).expect("a small count");
        held_now(
            &router,
            case,
            &url,
            expected_held,
            "once the attempt had been decided",
        )
        .await;
        let held = held_bodies(&engine, &arn);
        let expected: BTreeSet<String> = row.sent.iter().map(|body| (*body).to_string()).collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected every message to stay on the queue with its body, got \
             {held:?}"
        );
    }
}
