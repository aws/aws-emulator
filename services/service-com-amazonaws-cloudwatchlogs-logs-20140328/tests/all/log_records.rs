// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetLogRecord`: retrieving the WHOLE log event one `@ptr` addresses.
//!
//! The rules under test:
//!
//! * a `logRecordPointer` this service minted for a stored event ("the value of the @ptr
//!   field for a log event is the value to use as logRecordPointer to retrieve that complete
//!   log event record") is resolved to that event, and EVERY field of it is reported as a
//!   field name/field value pair — "All fields are retrieved, even if the original query that
//!   produced the logRecordPointer retrieved only a subset of fields" — with "the full
//!   unparsed log event […] returned within @message";
//! * the fields reported are the ones CloudWatch Logs generates for an event plus the fields
//!   the event's own message names, and nothing else;
//! * a field the query's row carried and the same field of the retrieved record carry one
//!   value, whichever subset the query projected — and for the two fields whose value is an
//!   INSTANT, one instant: a record states it in the epoch MILLISECONDS the event is stored
//!   under, where a result row renders it for reading;
//! * a pointer this service did not mint is refused as a parameter specified incorrectly, and
//!   a pointer whose log group, log stream or event this store no longer holds — and that no
//!   query's result set holds either — is refused as the missing resource it names, while a
//!   pointer a QUERY handed out keeps addressing the record that query read, because the
//!   result set holds the records its rows point at;
//! * retrieving a record changes nothing a later caller sees.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The table spans what the rules generalise over rather than one event: the FORM of the
//! ingested message (a flat JSON object, a nested one, one holding a list, bare text, a bare
//! JSON array), which stream of the group the event was ingested into, and which subset of
//! fields the query that produced the pointer projected.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    encode_token, insights::parse_timestamp, insights::GENERATED_FIELDS, log_record_pointer,
    log_record_pointer_parts, resource_not_found_message, QUERY_COMPLETION_DELAY_MS,
    QUERY_RUNNING_DELAY_MS,
};

use crate::harness::{base_ms, batch, messages, now_ms, Logs};

/// The operations under test.
const RECORD: &str = "GetLogRecord";
const START: &str = "StartQuery";
const RESULTS: &str = "GetQueryResults";

/// The modelled error codes the rules name.
const INVALID_PARAMETER: &str = "InvalidParameterException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The refusal a pointer this service cannot read is answered with.
const INVALID_POINTER: &str = "InvalidParameter Unable to deserialize token";

/// The log group every case ingests into, and the two streams of it.
const GROUP: &str = "/records/full-event";
const STREAM_A: &str = "stream-one";
const STREAM_B: &str = "stream-two";

/// How long a case waits before treating a started query as finished: past both spans the
/// start armed, with room for the test's own scheduling.
fn past_completion() -> Duration {
    Duration::from_millis(
        u64::try_from(QUERY_RUNNING_DELAY_MS + QUERY_COMPLETION_DELAY_MS + 400)
            .expect("a span of the clock is not negative"),
    )
}

/// One event of the table: the offset from the base instant, the stream it is ingested into,
/// its message, and the fields CloudWatch Logs discovers in that message.
type EventCase<'a> = (i64, &'a str, &'a str, Vec<(&'a str, &'a str)>);

/// The events every case retrieves, spanning the forms an ingested message takes: only a JSON
/// OBJECT names fields, a nested one names them by the dotted path they are reached by, and a
/// bare-text or bare-array message names none and is readable as `@message` alone.
fn events() -> Vec<EventCase<'static>> {
    vec![
        (
            1_000,
            STREAM_A,
            r#"{"level":"ERROR","duration":120,"user":"amy"}"#,
            vec![("level", "ERROR"), ("duration", "120"), ("user", "amy")],
        ),
        (
            2_000,
            STREAM_A,
            r#"{"request":{"path":"/orders","retries":2},"ok":false}"#,
            vec![
                ("request.path", "/orders"),
                ("request.retries", "2"),
                ("ok", "false"),
            ],
        ),
        (
            3_000,
            STREAM_A,
            r#"{"tags":["a","b"],"count":0}"#,
            vec![("tags.0", "a"), ("tags.1", "b"), ("count", "0")],
        ),
        (4_000, STREAM_A, "plain text WARN 42", Vec::new()),
        (5_000, STREAM_A, "[1,2,3]", Vec::new()),
        (
            6_000,
            STREAM_B,
            r#"{"level":"INFO","user":"bob"}"#,
            vec![("level", "INFO"), ("user", "bob")],
        ),
    ]
}

/// A log group holding [`events`] across both of its streams, and the window a query of it is
/// started with in the epoch SECONDS `StartQuery` documents for its range.
async fn stored_events(logs: &Logs) -> (i64, (i64, i64)) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    for stream in [STREAM_A, STREAM_B] {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": GROUP, "logStreamName": stream}),
        )
        .await;
    }
    // Aligned on a whole second, because a query's window is stated in whole SECONDS.
    let base = base_ms() / 1_000 * 1_000;
    for stream in [STREAM_A, STREAM_B] {
        let written: Vec<(i64, &str)> = events()
            .into_iter()
            .filter(|(_, into, _, _)| *into == stream)
            .map(|(offset, _, message, _)| (base + offset, message))
            .collect();
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": stream,
                "logEvents": batch(&written),
            }),
        )
        .await;
    }
    (base, (base / 1_000 - 60, now_ms() / 1_000 + 60))
}

/// The rows one finished query produced, each as the field/value pairs it carries.
async fn query_rows(
    logs: &Logs,
    window: (i64, i64),
    query_string: &str,
) -> Vec<BTreeMap<String, String>> {
    let started = logs
        .ok(
            START,
            &json!({
                "logGroupName": GROUP,
                "startTime": window.0,
                "endTime": window.1,
                "queryString": query_string,
            }),
        )
        .await;
    let query_id = started["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {started}"))
        .to_string();
    tokio::time::sleep(past_completion()).await;
    let answer = logs.ok(RESULTS, &json!({"queryId": query_id})).await;
    answer["results"]
        .as_array()
        .unwrap_or_else(|| panic!("GetQueryResults answers a results list, got {answer}"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("every result row is a list of fields, got {row}"))
                .iter()
                .map(|field| {
                    (
                        field["field"].as_str().unwrap_or_default().to_string(),
                        field["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// The record `GetLogRecord` reports for `pointer`, as its field name/field value pairs.
async fn retrieved(logs: &Logs, pointer: &str, unmask: bool) -> BTreeMap<String, String> {
    let answer = logs
        .ok(
            RECORD,
            &json!({"logRecordPointer": pointer, "unmask": unmask}),
        )
        .await;
    answer["logRecord"]
        .as_object()
        .unwrap_or_else(|| panic!("GetLogRecord answers a logRecord map, got {answer}"))
        .iter()
        .map(|(field, value)| {
            (
                field.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("every field of a record is text, got {value}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The value of one field, or the empty string when the map does not carry it.
fn value(fields: &BTreeMap<String, String>, field: &str) -> String {
    fields.get(field).cloned().unwrap_or_default()
}

/// The fields of a record whose value is an INSTANT: the two CloudWatch Logs generates for
/// every event, as `insights::GENERATED_FIELDS` names them.
const INSTANT_FIELDS: [&str; 2] = ["@timestamp", "@ingestionTime"];

/// The instant a RECORD states for `field`, which it states in epoch milliseconds, or `None`
/// when the value is in no such unit.
fn record_instant(fields: &BTreeMap<String, String>, field: &str) -> Option<i64> {
    value(fields, field).parse::<i64>().ok()
}

/// Every field of the addressed event is retrieved, whatever form its message takes and
/// however narrow the projection of the query that produced the pointer was.
#[tokio::test]
async fn every_field_of_the_event_a_pointer_addresses_is_retrieved() {
    let logs = Logs::new();
    let (base, window) = stored_events(&logs).await;

    // The query projects ONE column; the record reached through its `@ptr` carries them all.
    let rows = query_rows(&logs, window, "fields @timestamp").await;
    assert_eq!(
        rows.len(),
        events().len(),
        "case the rows: one row per stored event of the window, got {rows:?}"
    );

    let account = engine::emulator_account_id();
    let mut retrieved_messages: Vec<String> = Vec::new();
    for row in &rows {
        let pointer = value(row, "@ptr");
        let got = retrieved(&logs, &pointer, false).await;
        let message = value(&got, "@message");
        let case = message.clone();
        let (offset, stream, _, discovered) = events()
            .into_iter()
            .find(|(_, _, ingested, _)| *ingested == message)
            .unwrap_or_else(|| {
                panic!("case {case:?}: @message is the event's own text, got {got:?}")
            });

        assert_eq!(
            value(&got, "@ptr"),
            pointer,
            "case {case:?}: the record carries the pointer it was reached by, got {got:?}"
        );
        assert_eq!(
            value(&got, "@logStream"),
            stream,
            "case {case:?}: @logStream is the stream the event was ingested into, got {got:?}"
        );
        assert_eq!(
            value(&got, "@log"),
            format!("{account}:{GROUP}"),
            "case {case:?}: @log is the account-qualified log group, got {got:?}"
        );
        assert_eq!(
            record_instant(&got, "@timestamp"),
            Some(base + offset),
            "case {case:?}: @timestamp is the instant the event was stamped with, in epoch \
             milliseconds, got {got:?}"
        );
        assert!(
            record_instant(&got, "@ingestionTime").is_some_and(|accepted| accepted >= base + offset),
            "case {case:?}: @ingestionTime is the instant the event was accepted, in epoch \
             milliseconds, got {got:?}"
        );
        for (field, expected) in &discovered {
            assert_eq!(
                &value(&got, field),
                expected,
                "case {case:?}: {field} is the value the message names, got {got:?}"
            );
        }

        // The generated fields, as `insights::GENERATED_FIELDS` names them, plus the fields
        // this message names — and no field nothing in the event produced.
        let mut expected: Vec<String> = GENERATED_FIELDS.iter().map(|f| (*f).to_string()).collect();
        expected.extend(discovered.iter().map(|(field, _)| (*field).to_string()));
        expected.sort();
        let mut reported: Vec<String> = got.keys().cloned().collect();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the event's own fields, got {got:?}"
        );
        retrieved_messages.push(message);
    }
    retrieved_messages.sort();
    let mut ingested: Vec<String> = events()
        .into_iter()
        .map(|(_, _, message, _)| message.to_string())
        .collect();
    ingested.sort();
    assert_eq!(
        retrieved_messages, ingested,
        "case the pointers: each row's pointer addresses its own event, got \
         {retrieved_messages:?}"
    );
}

/// A field a query's row carried and the same field of the retrieved record carry one value,
/// whichever subset of the fields the query projected.
#[tokio::test]
async fn the_record_agrees_with_the_query_row_it_was_pointed_from() {
    let logs = Logs::new();
    let (_, window) = stored_events(&logs).await;

    // (case, the query string whose rows the records are reached from)
    let projections = [
        ("the default columns", "filter @message like /text/"),
        ("the generated fields", "fields @timestamp, @message, @logStream, @log, @ingestionTime"),
        ("the fields the messages name", "fields level, user, request.path"),
        ("one computed column beside a stored one", "fields @message | sort @timestamp desc"),
    ];
    for (case, query_string) in projections {
        let rows = query_rows(&logs, window, query_string).await;
        assert!(
            !rows.is_empty(),
            "case {case:?}: the query produces rows to be pointed from, got {rows:?}"
        );
        for row in &rows {
            let pointer = value(row, "@ptr");
            let got = retrieved(&logs, &pointer, false).await;
            for (field, projected) in row {
                // A row RENDERS an instant for reading and a record states it in epoch
                // milliseconds, so for those two fields the reads agree on the INSTANT rather
                // than on its text.
                if INSTANT_FIELDS.contains(&field.as_str()) {
                    assert_eq!(
                        record_instant(&got, field),
                        parse_timestamp(projected),
                        "case {case:?}: {field} of the row and of the record are one instant, \
                         got {got:?}"
                    );
                    continue;
                }
                assert_eq!(
                    &value(&got, field),
                    projected,
                    "case {case:?}: {field} of the row and of the record agree, got {got:?}"
                );
            }
            assert!(
                got.len() >= row.len(),
                "case {case:?}: the record carries at least every projected field, got {got:?}"
            );
        }
    }
}

/// Retrieving a record reports the event and changes nothing a later caller sees, and
/// `unmask` reports the same record over a store that masks nothing.
#[tokio::test]
async fn retrieving_a_record_is_a_read() {
    let logs = Logs::new();
    let (_, window) = stored_events(&logs).await;
    let before = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": STREAM_A}),
        )
        .await;

    let rows = query_rows(&logs, window, "fields @message").await;
    for row in &rows {
        let pointer = value(row, "@ptr");
        let masked = retrieved(&logs, &pointer, false).await;
        let unmasked = retrieved(&logs, &pointer, true).await;
        let again = retrieved(&logs, &pointer, false).await;
        let case = value(&masked, "@message");
        assert_eq!(
            masked, unmasked,
            "case {case:?}: this store masks nothing, so unmask reports the same record, got \
             {unmasked:?}"
        );
        assert_eq!(
            masked, again,
            "case {case:?}: two reads of one pointer report one record, got {again:?}"
        );
    }

    let after = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": STREAM_A}),
        )
        .await;
    assert_eq!(
        messages(&before),
        messages(&after),
        "case the stored events: retrieving records leaves every event as it was, got {after}"
    );
}

/// A pointer that addresses no record of this store is refused: one this service did not mint
/// as a parameter specified incorrectly, one it did mint whose resource is gone as that
/// missing resource.
#[tokio::test]
async fn a_pointer_that_addresses_no_record_is_refused() {
    let logs = Logs::new();
    let (base, _) = stored_events(&logs).await;

    // A pointer this service DID mint, for the event ingested first, to vary one part of at a
    // time against.
    let minted = log_record_pointer(GROUP, STREAM_A, base + 1_000, 0);
    let parts = log_record_pointer_parts(&minted)
        .unwrap_or_else(|| panic!("a minted pointer reads back, got {minted}"));

    // (case, the pointer presented, the error code, the message)
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "a pointer nothing minted",
            "not-a-pointer".to_string(),
            INVALID_PARAMETER,
            INVALID_POINTER.to_string(),
        ),
        (
            "an empty pointer",
            String::new(),
            INVALID_PARAMETER,
            INVALID_POINTER.to_string(),
        ),
        (
            "a pointer whose payload this service cannot read",
            "ab".to_string(),
            INVALID_PARAMETER,
            INVALID_POINTER.to_string(),
        ),
        (
            "a token of another kind of this service's own",
            encode_token(&json!({"kind": "GetQueryResults", "queryId": "q", "after": 0})),
            INVALID_PARAMETER,
            INVALID_POINTER.to_string(),
        ),
        (
            "a pointer naming a log group this account does not hold",
            log_record_pointer("/records/no-such-group", STREAM_A, parts.timestamp_ms, parts.seq),
            NOT_FOUND,
            resource_not_found_message("log group"),
        ),
        (
            "a pointer naming a log stream the group does not hold",
            log_record_pointer(GROUP, "no-such-stream", parts.timestamp_ms, parts.seq),
            NOT_FOUND,
            resource_not_found_message("log stream"),
        ),
        (
            "a pointer naming an instant the stream holds no event at",
            log_record_pointer(GROUP, STREAM_A, parts.timestamp_ms - 1, parts.seq),
            NOT_FOUND,
            resource_not_found_message("log record"),
        ),
        (
            "a pointer naming an ingestion the stream does not hold",
            log_record_pointer(GROUP, STREAM_A, parts.timestamp_ms, parts.seq + 10_000),
            NOT_FOUND,
            resource_not_found_message("log record"),
        ),
    ];
    for (case, pointer, expected_code, expected_message) in cases {
        let (code, message) = logs
            .refusal(RECORD, &json!({"logRecordPointer": pointer}))
            .await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }
}

/// A pointer a QUERY handed out keeps addressing the record that query read once the log
/// stream, and then the log group, that held it is deleted: the query's result set holds the
/// records its rows point at, so the `@ptr` it handed out is as durable as the rows are. A
/// pointer no result set holds addresses only what the store still holds, and says which
/// resource is gone ([`a_pointer_that_addresses_no_record_is_refused`]).
#[tokio::test]
async fn a_pointer_a_query_handed_out_outlives_the_resources_that_held_its_record() {
    let logs = Logs::new();
    let (_, window) = stored_events(&logs).await;
    let rows = query_rows(&logs, window, "fields @message, @logStream").await;
    let pointer_in = |stream: &str| -> String {
        rows.iter()
            .find(|row| value(row, "@logStream") == stream)
            .map(|row| value(row, "@ptr"))
            .unwrap_or_else(|| panic!("case {stream}: a row of that stream was produced"))
    };
    let in_a = pointer_in(STREAM_A);
    let in_b = pointer_in(STREAM_B);

    // Both resolve while their stream is held, and each record is kept to compare against
    // what the pointer reports once its resources are gone.
    let mut held: Vec<(&str, BTreeMap<String, String>)> = Vec::new();
    for (case, pointer) in [(STREAM_A, &in_a), (STREAM_B, &in_b)] {
        let got = retrieved(&logs, pointer, false).await;
        assert_eq!(
            value(&got, "@logStream"),
            case,
            "case {case}: the pointer resolves while its stream is held, got {got:?}"
        );
        held.push((case, got));
    }
    let record_of = |stream: &str| -> BTreeMap<String, String> {
        held.iter()
            .find(|(case, _)| *case == stream)
            .map(|(_, record)| record.clone())
            .unwrap_or_else(|| panic!("case {stream}: its record was read"))
    };

    // One stream deleted: its pointer still reports the record the query read it as, because
    // the result set holds it.
    logs.ok(
        "DeleteLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": STREAM_B}),
    )
    .await;
    assert_eq!(
        retrieved(&logs, &in_b, false).await,
        record_of(STREAM_B),
        "case a deleted stream: the result set holds the record its row pointed at"
    );
    let still = retrieved(&logs, &in_a, false).await;
    assert_eq!(
        value(&still, "@logStream"),
        STREAM_A,
        "case another stream: deleting one stream leaves the other's records, got {still:?}"
    );

    // The group deleted: the remaining pointer still reports its own record too.
    logs.ok("DeleteLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    for (case, pointer) in [(STREAM_A, &in_a), (STREAM_B, &in_b)] {
        assert_eq!(
            retrieved(&logs, pointer, false).await,
            record_of(case),
            "case a deleted group: the result set holds the record its row pointed at"
        );
    }
}

/// The `logRecord` member is a map of field name to field value, so a record's values are
/// text whatever JSON type the message held them as.
#[tokio::test]
async fn every_value_of_a_record_is_text() {
    let logs = Logs::new();
    let (_, window) = stored_events(&logs).await;
    let rows = query_rows(&logs, window, "fields @message").await;
    for row in &rows {
        let pointer = value(row, "@ptr");
        let answer = logs
            .ok(RECORD, &json!({"logRecordPointer": pointer}))
            .await;
        let record = answer["logRecord"]
            .as_object()
            .unwrap_or_else(|| panic!("GetLogRecord answers a logRecord map, got {answer}"));
        for (field, held) in record {
            assert!(
                matches!(held, Value::String(_)),
                "case {field}: a record's values are field VALUES, got {held}"
            );
        }
    }
}
