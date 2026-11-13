// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an event source mapping CONSUMES of a batch it handed to its
//! function, which is decided by that invocation's result and the mapping's
//! `FunctionResponseTypes` — "(Kinesis, DynamoDB Streams, Amazon MSK, self-managed Apache
//! Kafka, and Amazon SQS) A list of current response type enums applied to the event source
//! mapping", whose one value (`smithy_query(query="shape",
//! shape="FunctionResponseType")`) is `ReportBatchItemFailures`.
//!
//! The rule under test, in one sentence: a mapping carrying that response type consumes
//! every record of the batch EXCEPT the ones its function's response names in
//! `batchItemFailures`, consumes nothing at all when that response cannot be read as a
//! report of records of the batch it handed over, and — carrying no such type — reads no
//! report at all, so the whole batch's fate is the invocation's own.
//!
//! The first suite drives the reading over the catalog of conditions AWS publishes for the
//! report, against the message ids of a REAL batch: the identifiers are the ones Amazon SQS
//! minted for messages sent and received through its own surface over the router, so a
//! reading that does not agree with the ids a queue hands out fails here. The second drives
//! the whole chain over HTTP — `CreateQueue`, `CreateFunction`, `CreateEventSourceMapping`,
//! `SendMessage` and `GetQueueAttributes` — and pins the arm a client can observe while this
//! emulator has no in-process runtime for a deployment package: no message is consumed on
//! the strength of a report no invocation produced.

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    processed_records, read_batch_item_failures, reports_batch_item_failures, BatchReport,
    REPORT_BATCH_ITEM_FAILURES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_mapping_state, call_sqs, create_mapping, create_queue, create_timed_function,
    driven_router_with_queues, get_mapping, messages_held, queue_message_counts,
    router_with_queues, STATE_ENABLED,
};

/// `SendMessage` of `body` to the queue `url` addresses, answering the `MessageId` Amazon SQS
/// minted for it — the identifier a report of a queue batch names a record by.
async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) -> String {
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
    sent["MessageId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no MessageId in {sent}"))
        .to_string()
}

/// One row of the catalog below: the case, the response document the function answered, and
/// the messages of the batch the attempt must then consume — `None` when it must consume
/// none of them.
type ReportRow = (&'static str, Option<Vec<u8>>, Option<Vec<String>>);

/// The report document a function returns when it names `identifiers` as the records it did
/// not process.
fn report_of(identifiers: &[&str]) -> Vec<u8> {
    let entries: Vec<serde_json::Value> = identifiers
        .iter()
        .map(|identifier| json!({ "itemIdentifier": identifier }))
        .collect();
    json!({ "batchItemFailures": entries })
        .to_string()
        .into_bytes()
}

#[tokio::test]
async fn a_report_of_failed_records_decides_which_of_a_real_batch_an_attempt_consumes() {
    let case = "a batch of three messages of one queue";
    let router = router_with_queues();
    let (url, _) = create_queue(&router, case, "esm-report-ids", &json!({})).await;
    // The identifiers are the queue's own: three messages sent through Amazon SQS, in the
    // order a batch would hold them.
    let mut batch: Vec<String> = Vec::new();
    for body in ["first", "second", "third"] {
        batch.push(send_message(&router, case, &url, body).await);
    }
    let (first, second, third) = (batch[0].clone(), batch[1].clone(), batch[2].clone());

    // Nothing but the declared response type asks for a report to be read, so a mapping
    // without it consumes the whole batch whatever its function answered.
    for (types, expected) in [
        (Vec::new(), false),
        (vec![REPORT_BATCH_ITEM_FAILURES.to_string()], true),
        (vec!["ReportBatchItemFailuresEventually".to_string()], false),
    ] {
        let reads = reports_batch_item_failures(&types);
        assert_eq!(
            reads, expected,
            "case {case:?}: FunctionResponseTypes {types:?} must ask for the report to be \
             read: {expected}, got {reads}"
        );
    }

    // One row per ANSWER the rule has, over the real ids: a report that names none of the
    // batch, one that cannot be read as a report of THIS batch (an id of no batch, which only
    // real queue-minted ids can show), and ones that name a message, two of them out of batch
    // order, and every message of the batch. The whole catalog of conditions AWS publishes is
    // the table in the unit module beside the reading itself.
    let rows: Vec<ReportRow> = vec![
        ("no response at all", None, Some(batch.clone())),
        (
            "a response naming no batchItemFailures",
            Some(json!({ "ok": true }).to_string().into_bytes()),
            Some(batch.clone()),
        ),
        ("a response that is not JSON", Some(b"<html/>".to_vec()), None),
        (
            "an itemIdentifier naming a message of no batch",
            Some(report_of(&["00000000-0000-4000-8000-000000000000"])),
            None,
        ),
        (
            "the first message of the batch",
            Some(report_of(&[first.as_str()])),
            Some(vec![second.clone(), third.clone()]),
        ),
        (
            "two messages, named out of batch order",
            Some(report_of(&[third.as_str(), first.as_str()])),
            Some(vec![second.clone()]),
        ),
        (
            "every message of the batch, which leaves nothing consumed",
            Some(report_of(&[first.as_str(), second.as_str(), third.as_str()])),
            None,
        ),
    ];
    for (row, payload, expected) in rows {
        let report = read_batch_item_failures(payload.as_deref(), &batch);
        let consumed = processed_records(&report, &batch);
        assert_eq!(
            consumed, expected,
            "case {case:?} row {row:?}: expected the attempt to consume {expected:?}, got \
             {consumed:?} from {report:?}"
        );
        // A report that cannot be read is the whole batch retried, and a report that names
        // records is exactly those records left behind.
        match expected {
            None => assert!(
                matches!(report, BatchReport::WholeBatchFailed(_)),
                "case {case:?} row {row:?}: the whole batch must be reported failed, got \
                 {report:?}"
            ),
            Some(consumed) if consumed.len() == batch.len() => assert_eq!(
                report,
                BatchReport::WholeBatchProcessed,
                "case {case:?} row {row:?}: the whole batch must be reported processed"
            ),
            Some(consumed) => {
                let left_behind: Vec<String> = batch
                    .iter()
                    .filter(|identifier| !consumed.contains(identifier))
                    .cloned()
                    .collect();
                assert_eq!(
                    report,
                    BatchReport::RecordsFailed(left_behind.clone()),
                    "case {case:?} row {row:?}: expected the records {left_behind:?} to be \
                     the ones left behind, got {report:?}"
                );
            }
        }
    }
}

/// One row of the suite below: what the mapping asks for, and how much is waiting.
struct Row {
    case: &'static str,
    /// The mapping's `FunctionResponseTypes`.
    response_types: serde_json::Value,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from your
    /// stream or queue".
    batch_size: usize,
    /// The bodies waiting on the source queue.
    bodies: &'static [&'static str],
}

#[tokio::test]
async fn no_message_is_consumed_on_a_report_no_invocation_produced() {
    // The rows differ along whether the mapping asks for the report to be read, how many
    // messages are waiting and how many one batch may hold. Running a deployment package is
    // not implemented in this emulator, so no invocation of these mappings' functions
    // succeeds and no report of any batch exists to be read — and the rule's answer for
    // that is the same in every row: nothing of the batch is consumed.
    let rows = [
        Row {
            case: "a mapping reading the report, one message",
            response_types: json!([REPORT_BATCH_ITEM_FAILURES]),
            batch_size: 10,
            bodies: &["only"],
        },
        Row {
            case: "a mapping reading the report, three messages one per batch",
            response_types: json!([REPORT_BATCH_ITEM_FAILURES]),
            batch_size: 1,
            bodies: &["a", "b", "c"],
        },
        Row {
            case: "a mapping reading no report, three messages in one batch",
            response_types: json!([]),
            batch_size: 10,
            bodies: &["a", "b", "c"],
        },
    ];
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // The poll passes are this suite's own, awaited one per batch: a row then reads the
        // state the passes it made left behind, rather than a span of wall clock after
        // which the background loop may or may not have got to the work.
        let (router, mut driven) = driven_router_with_queues();
        let function = format!("esm-report-fn-{index}");
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("esm-report-source-{index}"),
            &json!({ "VisibilityTimeout": "1" }),
        )
        .await;
        // A queue only maps onto a function it outlasts, so the function's `Timeout` is the
        // queue's own visibility timeout.
        create_timed_function(&router, case, &function, 1).await;
        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "BatchSize": row.batch_size,
                "FunctionResponseTypes": row.response_types,
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

        // A mapping polls from the moment it reports `Enabled`, a move the clock owes that
        // every read of the mapping materializes.
        await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;

        for body in row.bodies {
            send_message(&router, case, &url, body).await;
        }

        // One pass per batch the messages waiting take, each awaited: when a pass answers,
        // the batch it took has been handed to the function and refused, and what the attempt
        // came to is recorded.
        for _ in 0..row.bodies.len().div_ceil(row.batch_size.max(1)) {
            driven.poll_once().await;
        }

        // The queue still holds everything that was sent: a message leaves a queue only once
        // the attempt it was handed to processed it.
        let counts = queue_message_counts(&router, case, &url).await;
        assert_eq!(
            messages_held(counts),
            u32::try_from(row.bodies.len()).unwrap_or(u32::MAX),
            "case {case:?}: the queue must still hold the {} messages it was sent, got \
             {counts:?} as (visible, in flight, delayed)",
            row.bodies.len()
        );
        // And the mapping made the attempt: "the result of the event source mapping's last
        // processing attempt" is reported, so the batch really was handed over. The pass
        // recorded it before it answered, so this is read once.
        let reading = get_mapping(&router, &uuid).await;
        let result = reading.string("LastProcessingResult").unwrap_or_default();
        assert!(
            !result.is_empty(),
            "case {case:?}: a mapping that handed a batch to its function must report what \
             that attempt came to, got {}",
            reading.text()
        );
    }
}
