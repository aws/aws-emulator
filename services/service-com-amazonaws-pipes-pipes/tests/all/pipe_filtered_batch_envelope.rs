// SPDX-License-Identifier: CC0-1.0

//! What a FILTERED pipe puts on its target for ONE polled batch, end to end over the wire.
//!
//! Pipes, Amazon SQS and IAM are mounted on the SAME engine and every call goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — because
//! what the pipe wrote is read back the way a client reads it: off the target queue.
//!
//! The RULE asserted over a table: of the records ONE poll takes from an Amazon SQS source,
//! exactly those the pipe's `FilterCriteria` admit reach its target, each as the record
//! envelope that reports its polled message AS AMAZON SQS REPORTS IT — the message's id,
//! receipt handle, body, system attributes, message attributes and the digests the queue
//! published for it, beside which source it came from — while the records the patterns drop
//! reach no target at all and are consumed from the source just the same.
//!
//! `tests/all/pipe_record_envelope.rs` pins the digests of a record an UNFILTERED pipe
//! delivers, one record at a time. What is new here is the batch a filter DECIDES: a poll
//! holding records it admits beside records it drops, which is the shape a client filtering
//! at the pipe actually produces — and the two envelope members that suite does not read,
//! the four system attributes a polled message carries and the absence of any message
//! attribute on the target message itself.
//!
//! The rows differ along what the rule generalises over: which records of the batch the
//! pattern admits (the second only, the first only, both, none), where the dropped records
//! sit in it, how many message attributes each admitted record carries (none, one, two —
//! which is what the attribute digest is computed over), and therefore whether the queue
//! published an attribute digest for it at all.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{given_queue, sqs, wait_for_counted, LOOK_AGAIN, QUIET};
use crate::pipe_execution_role::wait_for_state;
use crate::pipe_lifecycle::{call, REGION};
use crate::pipe_metrics::{emulator, given_role_stating};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;

/// The pattern every pipe below filters by: it admits the records whose body spells a
/// document saying `kind: wanted`, and no others.
const WANTED_PATTERN: &str = r#"{"body": {"kind": ["wanted"]}}"#;

/// How long a delivery the pipe has to make is given before a row fails. Well above the
/// data plane's poll interval and the span a pipe takes to reach RUNNING, so a row fails
/// only when the pipe did not deliver at all.
const BUDGET: Duration = Duration::from_secs(25);

/// The members an Amazon SQS record envelope carries for EVERY polled message.
///
/// Source: the `Message` members `ReceiveMessage` reports (`MessageId`, `ReceiptHandle`,
/// `Body`, `Attributes`, `MessageAttributes`, `MD5OfBody`), as the record envelope spells
/// them, plus the three that say which source the record came from.
const ALWAYS_PRESENT: &[&str] = &[
    "attributes",
    "awsRegion",
    "body",
    "eventSource",
    "eventSourceARN",
    "md5OfBody",
    "messageAttributes",
    "messageId",
    "receiptHandle",
];

/// The member carrying the digest of the message's ATTRIBUTES, which Amazon SQS publishes
/// for a message that carries message attributes and for no other
/// (`Message$MD5OfMessageAttributes`).
const ATTRIBUTE_DIGEST: &str = "md5OfMessageAttributes";

/// The system attributes a polled message's `attributes` member reports.
///
/// Source: the `MessageSystemAttributeName` values Amazon SQS reports for a standard
/// queue's message — `SenderId`, `SentTimestamp`, `ApproximateReceiveCount` and
/// `ApproximateFirstReceiveTimestamp` — which is also what a Lambda SQS event record
/// carries under the same member.
const SYSTEM_ATTRIBUTES: &[&str] = &[
    "ApproximateFirstReceiveTimestamp",
    "ApproximateReceiveCount",
    "SenderId",
    "SentTimestamp",
];

/// The attributes that together say how many records a queue still HOLDS: the ones waiting
/// on it plus the ones a consumer has taken and not deleted ("messages that are in flight",
/// `ApproximateNumberOfMessagesNotVisible`).
const HELD_ATTRIBUTES: [&str; 2] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
];

/// The `eventSource` an Amazon SQS record envelope reports.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// One message attribute a record of a row carries: its name, its `DataType` and the
/// `StringValue` it travels with.
struct Attribute {
    name: &'static str,
    data_type: &'static str,
    value: &'static str,
}

/// One record a row sends to the source queue.
struct Record {
    /// What the row calls this record in a failure.
    label: &'static str,
    /// The body it is sent with, which is what the pipe's pattern decides about.
    body: &'static str,
    /// The message attributes it is sent with.
    attributes: &'static [Attribute],
    /// Whether the pipe's pattern admits it, which is what the row asserts about.
    admitted: bool,
}

/// One row of the table: the records ONE poll takes, in the order they are sent.
struct Row {
    /// The case, which also names the row's pipe and queues.
    case: &'static str,
    records: &'static [Record],
}

/// RULE: of the records one poll takes, exactly those the pipe's `FilterCriteria` admit
/// reach its target, each as the envelope reporting its message as Amazon SQS reports it —
/// digests included — and every record of the batch, admitted or dropped, is consumed from
/// the source.
#[tokio::test]
async fn one_polled_batch_delivers_the_admitted_records_as_their_own_envelopes() {
    let rows = [
        Row {
            case: "dropped-first-then-admitted",
            records: &[
                Record {
                    label: "the dropped record",
                    body: r#"{"kind": "unwanted", "marker": "suppressed"}"#,
                    attributes: &[Attribute {
                        name: "probe",
                        data_type: "String",
                        value: "attr-bad",
                    }],
                    admitted: false,
                },
                Record {
                    label: "the admitted record",
                    body: r#"{"kind": "wanted", "marker": "control"}"#,
                    attributes: &[Attribute {
                        name: "probe",
                        data_type: "String",
                        value: "attr-good",
                    }],
                    admitted: true,
                },
            ],
        },
        Row {
            case: "admitted-first-then-dropped",
            records: &[
                Record {
                    label: "the admitted record",
                    body: r#"{"kind": "wanted", "marker": "leading"}"#,
                    attributes: &[Attribute {
                        name: "probe",
                        data_type: "String",
                        value: "attr-good",
                    }],
                    admitted: true,
                },
                Record {
                    label: "the dropped record",
                    body: r#"{"kind": "other", "marker": "trailing"}"#,
                    attributes: &[],
                    admitted: false,
                },
            ],
        },
        Row {
            case: "two-admitted-around-a-dropped-one",
            records: &[
                Record {
                    label: "the admitted record carrying no attribute",
                    body: r#"{"kind": "wanted", "marker": "bare"}"#,
                    attributes: &[],
                    admitted: true,
                },
                Record {
                    label: "the dropped record",
                    body: r#"{"kind": "unwanted", "marker": "middle"}"#,
                    attributes: &[Attribute {
                        name: "probe",
                        data_type: "String",
                        value: "attr-bad",
                    }],
                    admitted: false,
                },
                Record {
                    label: "the admitted record carrying two attributes",
                    body: r#"{"kind": "wanted", "marker": "pair"}"#,
                    attributes: &[
                        Attribute {
                            name: "probe",
                            data_type: "String",
                            value: "attr-good",
                        },
                        Attribute {
                            name: "count",
                            data_type: "Number",
                            value: "42",
                        },
                    ],
                    admitted: true,
                },
            ],
        },
        Row {
            case: "every-record-dropped",
            records: &[
                Record {
                    label: "the first dropped record",
                    body: r#"{"kind": "unwanted", "marker": "first"}"#,
                    attributes: &[Attribute {
                        name: "probe",
                        data_type: "String",
                        value: "attr-bad",
                    }],
                    admitted: false,
                },
                Record {
                    label: "the second dropped record",
                    body: "a body spelling no document at all",
                    attributes: &[],
                    admitted: false,
                },
            ],
        },
    ];

    for row in &rows {
        run_row(row).await;
    }
}

/// Keep the table honest: every row is driven by this one body.
async fn run_row(row: &Row) {
    let case = row.case;
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{case}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;
    let role = given_role(&router, case, &source_arn, &target_arn).await;

    // The batch is made ONE batch by sending every record of it while the pipe is still
    // STOPPED: the poll that follows the start takes the whole backlog, so what the filter
    // decides about is a batch holding the row's records together rather than whichever of
    // them a poll happened to catch.
    let published: Vec<Published> = {
        let mut published = Vec::with_capacity(row.records.len());
        for record in row.records {
            published.push(send(&router, &source_url, case, record).await);
        }
        published
    };

    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": role,
            "DesiredState": "STOPPED",
            "SourceParameters": {
                "SqsQueueParameters": { "BatchSize": 10 },
                "FilterCriteria": { "Filters": [{ "Pattern": WANTED_PATTERN }] },
            },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    // The pipe is accepted `CREATING` and rests `STOPPED` a moment later; it is started
    // once it rests there, which is when a start is a move it can make.
    let resting = wait_for_state(&router, case, "STOPPED", BUDGET).await;
    assert_eq!(
        resting, "STOPPED",
        "case {case:?}: expected the pipe to rest STOPPED before it is started, \
         it reported {resting}"
    );
    let started = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}/start"),
        Some(json!({})),
        REGION,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartPipe answered {} {}",
        started.status, started.body
    );

    let admitted: Vec<(&Record, &Published)> = row
        .records
        .iter()
        .zip(published.iter())
        .filter(|(record, _)| record.admitted)
        .collect();

    // The source is drained whatever the filter decided: a dropped record entered the pipe
    // and the pipe decided about it, so it is consumed rather than offered again.
    let held = wait_for_counted(&router, &source_url, &HELD_ATTRIBUTES, 0, BUDGET).await;
    assert_eq!(
        held, 0,
        "case {case:?}: expected every record of the batch consumed from the source, \
         got {held} still held"
    );

    let delivered: Vec<Value> = if admitted.is_empty() {
        messages_during(&router, &target_url, QUIET).await
    } else {
        let mut collected =
            collect_messages(&router, &target_url, admitted.len(), BUDGET).await;
        // Nothing beyond the admitted records: the quiet window is what makes the absence
        // of a dropped record's delivery an answer rather than a race.
        collected.extend(messages_during(&router, &target_url, QUIET).await);
        collected
    };
    assert_eq!(
        delivered.len(),
        admitted.len(),
        "case {case:?}: expected the {} admitted record(s) delivered and nothing else, \
         got {delivered:?}",
        admitted.len()
    );
    // A pipe carries the record's own attributes INSIDE the envelope it delivers, so the
    // message it puts on the target carries none of its own.
    for message in &delivered {
        assert!(
            message.get("MessageAttributes").is_none(),
            "case {case:?}: expected a delivery to carry no message attribute of its own, \
             got {message}"
        );
    }

    // Each delivery is the envelope of the record it was made for, matched to that record
    // by the body it carries — the one member a delivery can be attributed by.
    for (record, published) in admitted {
        let payload = delivered
            .iter()
            .filter_map(|message| message["Body"].as_str())
            .find(|body| {
                serde_json::from_str::<Value>(body)
                    .ok()
                    .and_then(|envelope| {
                        envelope.get("body").and_then(Value::as_str).map(str::to_string)
                    })
                    .is_some_and(|body| body == record.body)
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a delivery for {}, got {delivered:?}",
                    record.label
                )
            });
        assert_envelope(case, payload, record, published, &source_arn);
    }
}

/// Every message that arrives at `url`, until there are at least `want` of them or `budget`
/// has elapsed — the WHOLE messages, not their bodies alone, which is what lets a row read
/// whether a delivery carried message attributes of its own (the collectors of
/// `tests/all/pipe_data_plane.rs` answer bodies).
async fn collect_messages(
    router: &MultiServiceRouter,
    url: &str,
    want: usize,
    budget: Duration,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + budget;
    let mut collected: Vec<Value> = Vec::new();
    loop {
        collected.extend(received(router, url).await);
        if collected.len() >= want || std::time::Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every message that arrives at `url` during the whole of `budget`.
async fn messages_during(
    router: &MultiServiceRouter,
    url: &str,
    budget: Duration,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + budget;
    let mut collected: Vec<Value> = Vec::new();
    while std::time::Instant::now() < deadline {
        collected.extend(received(router, url).await);
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    collected
}

/// One `ReceiveMessage` of up to ten messages, asking for every message attribute a
/// message carries, as the whole messages.
async fn received(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receiving from {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"].as_array().cloned().unwrap_or_default()
}

/// Assert that `payload` is the record envelope of `record` as the queue published it.
fn assert_envelope(
    case: &str,
    payload: &str,
    record: &Record,
    published: &Published,
    source_arn: &str,
) {
    let label = record.label;
    let envelope: Value = serde_json::from_str(payload).unwrap_or_else(|error| {
        panic!("case {case:?}: {label} was delivered as something that is not JSON ({error}): {payload:?}")
    });
    let members = envelope.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected {label} as a record envelope, got {envelope}")
    });

    // The MEMBERS: the ones every record carries, plus the attribute digest exactly when
    // the queue published one for this message.
    let mut expected: Vec<String> = ALWAYS_PRESENT.iter().map(|name| name.to_string()).collect();
    if published.attribute_digest.is_some() {
        expected.push(ATTRIBUTE_DIGEST.to_string());
    }
    expected.sort();
    let mut present: Vec<String> = members.keys().cloned().collect();
    present.sort();
    assert_eq!(
        present, expected,
        "case {case:?}: expected {label} to carry {expected:?}, got {envelope}"
    );

    // The DIGESTS are the queue's own, as the send that produced them published them.
    assert_eq!(
        members.get("md5OfBody").and_then(Value::as_str),
        Some(published.body_digest.as_str()),
        "case {case:?}: expected {label} to carry the body digest {:?}, got {envelope}",
        published.body_digest
    );
    assert_eq!(
        members.get(ATTRIBUTE_DIGEST).and_then(Value::as_str),
        published.attribute_digest.as_deref(),
        "case {case:?}: expected {label} to carry the attribute digest {:?}, got {envelope}",
        published.attribute_digest
    );
    assert_eq!(
        members.get("messageId").and_then(Value::as_str),
        Some(published.message_id.as_str()),
        "case {case:?}: expected {label} to carry the id the send published ({:?}), \
         got {envelope}",
        published.message_id
    );
    assert_eq!(
        members.get("body").and_then(Value::as_str),
        Some(record.body),
        "case {case:?}: expected {label} to carry its body as sent, got {envelope}"
    );
    assert_eq!(
        members.get("eventSource").and_then(Value::as_str),
        Some(SQS_EVENT_SOURCE),
        "case {case:?}: expected {label} to name the Amazon SQS event source, got {envelope}"
    );
    assert_eq!(
        members.get("eventSourceARN").and_then(Value::as_str),
        Some(source_arn),
        "case {case:?}: expected {label} to name the SOURCE queue, got {envelope}"
    );
    assert_eq!(
        members.get("awsRegion").and_then(Value::as_str),
        Some(REGION),
        "case {case:?}: expected {label} to name the region of the call, got {envelope}"
    );
    assert!(
        !members
            .get("receiptHandle")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: expected {label} to carry the receipt handle it was handed over \
         with, got {envelope}"
    );

    // The SYSTEM attributes the queue reports for the message, and the delivery count a
    // first delivery reports.
    let mut reported: Vec<String> = envelope["attributes"]
        .as_object()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {label} to report its system attributes, got {envelope}")
        })
        .keys()
        .cloned()
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        SYSTEM_ATTRIBUTES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: expected {label} to report {SYSTEM_ATTRIBUTES:?}, got {envelope}"
    );
    assert_eq!(
        envelope["attributes"]["ApproximateReceiveCount"].as_str(),
        Some("1"),
        "case {case:?}: expected {label} handed over once, got {envelope}"
    );

    // The MESSAGE attributes the digest above covers, each as the type and value it was
    // sent with, under the lowercase-keyed form a record envelope reports them in.
    let attributes = envelope["messageAttributes"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected {label} to report its message attributes, got {envelope}")
    });
    assert_eq!(
        attributes.len(),
        record.attributes.len(),
        "case {case:?}: expected {label} to report {} message attribute(s), got {envelope}",
        record.attributes.len()
    );
    for attribute in record.attributes {
        let reported = &envelope["messageAttributes"][attribute.name];
        assert_eq!(
            reported["dataType"].as_str(),
            Some(attribute.data_type),
            "case {case:?}: expected {label} to report {:?} as {:?}, got {envelope}",
            attribute.name,
            attribute.data_type
        );
        assert_eq!(
            reported["stringValue"].as_str(),
            Some(attribute.value),
            "case {case:?}: expected {label} to report {:?} carrying {:?}, got {envelope}",
            attribute.name,
            attribute.value
        );
    }
}

/// What Amazon SQS published for one sent message: its id and the digests, which the
/// envelope has to report rather than recompute.
struct Published {
    message_id: String,
    body_digest: String,
    /// `None` when the message carried no message attributes, for which Amazon SQS
    /// publishes no attribute digest.
    attribute_digest: Option<String>,
}

/// Send `record` to the queue at `url`, answering what the send published for it.
async fn send(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    record: &Record,
) -> Published {
    let label = record.label;
    let mut request = json!({ "QueueUrl": url, "MessageBody": record.body });
    if !record.attributes.is_empty() {
        let mut attributes = serde_json::Map::new();
        for attribute in record.attributes {
            attributes.insert(
                attribute.name.to_string(),
                json!({ "DataType": attribute.data_type, "StringValue": attribute.value }),
            );
        }
        request["MessageAttributes"] = Value::Object(attributes);
    }
    let (status, answer) = sqs(router, "SendMessage", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage for {label} answered {status} {answer}"
    );
    Published {
        message_id: answer["MessageId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no MessageId for {label} in {answer}"))
            .to_string(),
        body_digest: answer["MD5OfMessageBody"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no MD5OfMessageBody for {label} in {answer}"))
            .to_string(),
        attribute_digest: answer["MD5OfMessageAttributes"].as_str().map(str::to_string),
    }
}

/// The execution role every pipe here acts as: it may take the records of the source, read
/// how long that source holds them, delete them, and write to the target — the calls the
/// pipe's stages make and no others.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    source_arn: &str,
    target_arn: &str,
) -> String {
    given_role_stating(
        router,
        &format!("{case}-role"),
        SERVICE_PRINCIPAL,
        json!([
            {
                "Effect": "Allow",
                "Action": [
                    "sqs:ReceiveMessage",
                    "sqs:DeleteMessage",
                    "sqs:GetQueueAttributes",
                ],
                "Resource": source_arn,
            },
            {
                "Effect": "Allow",
                "Action": "sqs:SendMessage",
                "Resource": target_arn,
            },
        ]),
    )
    .await
}
