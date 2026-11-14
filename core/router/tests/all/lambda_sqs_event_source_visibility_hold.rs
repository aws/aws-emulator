// SPDX-License-Identifier: CC0-1.0

//! The VISIBILITY RESERVATION a Lambda event source mapping holds on the Amazon SQS records
//! it has taken, across the two services that own the halves of it: Amazon SQS holds the
//! queue and decides who may receive a record, and Lambda's mapping is the consumer holding
//! it.
//!
//! This suite needs more than one service by construction: the reservation is observable
//! only as the relation between what the mapping is doing with a record and what an
//! INDEPENDENT consumer of the same queue is handed, so no single-service test can see it.
//!
//! The RULE asserted over the table below: a record an event source mapping has taken stays
//! invisible to every other consumer of the queue for as long as that mapping HOLDS it —
//! the whole of the `MaximumBatchingWindowInSeconds` it gathers the batch over and the
//! attempt it then hands it to — even when the queue's own `VisibilityTimeout` is shorter
//! than that span: the mapping is the consumer that received the record, and a consumer
//! still working on a record keeps its reservation rather than letting the queue hand the
//! same record to a second consumer.
//!
//! Nothing here invokes the function and nothing runs a deployment package: each row's
//! function carries a package no engine can run, so the attempt the mapping makes with the
//! gathered batch FAILS — which is what makes the instant the mapping stopped gathering
//! observable (`LastProcessingResult`), and is the instant each row's watch over the queue
//! ends. The watch is a POLL LOOP, not a wait on the clock: an independent
//! `ReceiveMessage` is made again and again until the mapping reports its attempt, and a row
//! fails the moment one of those receives is handed the record.
//!
//! The rows differ along what the rule generalises over: the queue's own `VisibilityTimeout`
//! against the window the mapping gathers over, and how many records the reservation covers.

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED;
use smithy_server_runtime::MultiServiceRouter;

use crate::lambda_sqs_event_source_batching_window::{
    await_state, given_queue_hiding_for, given_unrunnable_function, lambda,
    last_processing_result, queue_attribute, send_message, shared_emulator, sqs, BUDGET,
    FUNCTION_TIMEOUT, IN_FLIGHT_MESSAGES, LOOK_AGAIN, MAPPINGS_URI, STATE_ENABLED,
};

/// One row: the queue's own `VisibilityTimeout`, the window the mapping gathers over and how
/// many records are sent.
struct Row {
    case: &'static str,
    /// The queue's `VisibilityTimeout`, in seconds — every row's is SHORTER than its window,
    /// which is the case the rule is about, and at least the function's own `Timeout`, which
    /// is what Lambda accepts a mapping on.
    visibility_timeout: i64,
    /// `MaximumBatchingWindowInSeconds`: how long the mapping spends gathering, and so how
    /// long it holds what it took.
    window: i64,
    /// How many records are sent to the queue, all of them inside one batch
    /// (`BatchSize` below is larger), so the reservation covers each of them.
    records: usize,
}

/// The `BatchSize` every row polls under: larger than the records it sends, so no row's
/// batch fills and each waits its whole window out.
const BATCH_SIZE: i64 = 10;

/// Receive from the queue `url` addresses as an INDEPENDENT consumer: whatever it is handed,
/// which must be nothing while the mapping holds the record.
async fn independent_receive(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let (status, received) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an independent ReceiveMessage answered {status} {received}"
    );
    received["Messages"].as_array().cloned().unwrap_or_default()
}

/// How many of the queue's records are in flight with a consumer right now.
async fn in_flight(router: &MultiServiceRouter, case: &str, url: &str) -> u64 {
    queue_attribute(router, case, url, IN_FLIGHT_MESSAGES)
        .await
        .and_then(|count| count.parse().ok())
        .unwrap_or_default()
}

/// Wait until the mapping has TAKEN every record of the row — the queue reports them in
/// flight with a consumer — which is where the reservation this suite is about begins.
async fn await_taken(router: &MultiServiceRouter, case: &str, url: &str, records: u64) {
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        let held = in_flight(router, case, url).await;
        if held >= records {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the mapping must take the {records} record(s) it polls, {held} \
             were in flight with it"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: while a mapping holds the records it took, no other consumer of the queue is handed
/// one — whatever the queue's own `VisibilityTimeout` is.
#[tokio::test]
async fn a_mapping_keeps_its_records_reserved_for_as_long_as_it_holds_them() {
    let rows = [
        Row {
            case: "visibility-shorter-than-the-window",
            visibility_timeout: FUNCTION_TIMEOUT,
            window: 4,
            records: 1,
        },
        Row {
            case: "a-longer-window-over-a-longer-visibility-timeout",
            // Still shorter than the window, so the reservation is the mapping's to keep,
            // and above the function's own `Timeout`, which is what Lambda accepts a
            // mapping on.
            visibility_timeout: FUNCTION_TIMEOUT + 1,
            window: 5,
            records: 2,
        },
    ];
    let router = shared_emulator();
    for row in &rows {
        let case = row.case;
        let (url, arn) =
            given_queue_hiding_for(&router, case, &format!("{case}-source"), row.visibility_timeout)
                .await;
        given_unrunnable_function(&router, case, case).await;

        let (status, created) = lambda(
            &router,
            "POST",
            MAPPINGS_URI,
            Some(&json!({
                "FunctionName": case,
                "EventSourceArn": arn,
                "BatchSize": BATCH_SIZE,
                "MaximumBatchingWindowInSeconds": row.window,
                "Enabled": true,
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateEventSourceMapping answered {status} {created}"
        );
        let uuid = created["UUID"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {created}"))
            .to_string();
        await_state(&router, case, &uuid, STATE_ENABLED).await;

        for index in 0..row.records {
            send_message(&router, case, &url, &format!("{case}-{index}")).await;
        }
        // The reservation begins where the mapping TAKES the records: until it has, the
        // queue is right to hand them to whoever asks, so the watch below starts here.
        await_taken(&router, case, &url, row.records as u64).await;

        // The watch: receive as an independent consumer until the mapping has made its
        // attempt — which is when it stopped holding the batch — and fail the moment one of
        // those receives is handed a record the mapping took.
        let mut attempt: Option<String> = None;
        let mut receives: u32 = 0;
        while attempt.is_none() {
            let stolen = independent_receive(&router, case, &url).await;
            assert!(
                stolen.is_empty(),
                "case {case:?}: a record the mapping is holding must stay invisible to every \
                 other consumer for as long as it holds it (the queue hides a delivery for \
                 {}s of its own, the mapping gathers over {}s), an independent receive was \
                 handed {stolen:?} after {receives} receive(s)",
                row.visibility_timeout,
                row.window
            );
            receives = receives.saturating_add(1);
            attempt = last_processing_result(&router, &uuid).await;
            if attempt.is_none() {
                tokio::time::sleep(LOOK_AGAIN).await;
            }
        }
        assert_eq!(
            attempt.as_deref(),
            Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
            "case {case:?}: the mapping's attempt is one its function could not take, so the \
             batch it held is still in flight"
        );
        assert!(
            receives > 1,
            "case {case:?}: the watch must have looked more than once while the mapping \
             gathered, it looked {receives} time(s)"
        );
    }
}
