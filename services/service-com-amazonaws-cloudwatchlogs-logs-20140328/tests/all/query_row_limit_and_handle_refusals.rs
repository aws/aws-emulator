// SPDX-License-Identifier: CC0-1.0

//! The row bound a query's START states, the durability of the handle it answers, and the
//! refusals `GetQueryResults` answers for a handle or a token it cannot use.
//!
//! The rules under test:
//!
//! * `StartQuery$limit` ("The maximum number of log events to return from the query. The
//!   maximum limit is 100,000", `@range 1..100000`) is AUTHORITATIVE over whatever row bound
//!   the query STRING carries — a Logs Insights `limit` command, a PPL `head`, a SQL `LIMIT` —
//!   so a start that names one replaces the string's bound in both directions: a parameter
//!   above it widens the answer, one below it narrows it, and a start that names none leaves
//!   the string's own bound to decide. The bound is on the ROWS returned, not on the events
//!   matched, so `recordsMatched` counts every matching event either way;
//! * a `queryId` is a durable handle on the result set its query PRODUCED: a query that has
//!   run answers the snapshot of that run, so changing the log groups it read afterwards —
//!   more events, a deleted stream, a deleted group — changes neither its rows nor its
//!   statistics;
//! * a `queryId` outside the length bounds the model declares for one (`QueryId`,
//!   `@length 1..256`) is refused with the one sentence this service words that bound with,
//!   rather than as a clause of the validation envelope — whichever operation the id arrived
//!   on and whichever end it breaks;
//! * a `queryId` that names no query of this account is refused by `GetQueryResults` as the
//!   incorrectly specified PARAMETER it is — a results handle that was never issued was never
//!   valid — in this operation's own sentence, while `StopQuery` keeps refusing the same id as
//!   the missing resource it models, in its own: the two templates are distinct;
//! * a `nextToken` `GetQueryResults` cannot resume THIS query's results from — one minted for
//!   another query, one it cannot decode — is refused `InvalidParameterException` with this
//!   operation's sentence stated inside the SDK error envelope every refusal of this service
//!   that carries one states, so the service, status and code the text reports are the ones the
//!   response itself answers with.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! a handler does not actually compute cannot pass.
//!
//! The tables span what the rules generalise over rather than one query: which direction the
//! parameter moves the string's bound, which language states that bound, each published end of
//! the parameter's range and one past it, and which FORM an unusable handle or token takes.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    encode_token, query_results_invalid_query_id_message, query_results_invalid_token_message,
    QUERY_STATUS_COMPLETE,
};

use crate::harness::{base_ms, batch, now_ms, without_sdk_envelope, Logs};

/// The operations under test.
const START: &str = "StartQuery";
const RESULTS: &str = "GetQueryResults";
const STOP: &str = "StopQuery";

/// The modelled error codes the rules name.
const INVALID_PARAMETER: &str = "InvalidParameterException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The HTTP status both of them are refused with, which is the status the SDK envelope of such
/// a refusal states (`InvalidParameterException`, `ResourceNotFoundException`: HTTP 400).
const REFUSED_STATUS: u16 = 400;

/// The log group every case queries, and the stream its events are written to.
const GROUP: &str = "/insights/row-limit";
const STREAM: &str = "stream-one";

/// How many events the group holds. More than any row bound a case states, so a bound that is
/// honoured is visible in the rows and the match count is never the bound.
const EVENTS: usize = 12;

/// Each published end of `StartQuery$limit`'s modelled range (`EventsLimitStartQuery`,
/// `@range 1..100000`), and the value just past each end.
const LIMIT_RANGE_MIN: i64 = 1;
const LIMIT_RANGE_MAX: i64 = 100_000;

/// The characters a query id may be long: the `@length` maximum of the model's `QueryId`
/// shape (1..256), which every `queryId` member of this service targets.
const QUERY_ID_MAX_CHARS: usize = 256;

/// How often, and how many times, a case polls for the status a started query settles at. The
/// product is far past the spans a start arms, so a settled query is never missed and no case
/// waits for a fixed duration instead of for the status itself.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLL_ATTEMPTS: usize = 400;

/// One row of the row-bound table.
struct Bound {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// The `queryLanguage` the query string is written in, as the model's enum spells it.
    language: &'static str,
    /// The query string, which may carry a row bound of its own.
    query: String,
    /// The `limit` the start names, or `None` for a start that names none.
    limit: Option<i64>,
    /// How many rows the results must carry.
    rows: usize,
    /// The `n` values those rows must carry, in order, for a query that orders by `n`; `None`
    /// for one whose row ORDER is not what this row is about.
    ordered: Option<Vec<String>>,
}

/// The log group holding [`EVENTS`] events — the `i`th stamped `i` seconds after the base
/// instant with the message `{"n": i}` — and the window, in the epoch SECONDS `StartQuery`
/// documents, that holds every one of them.
async fn group_with_numbered_events(logs: &Logs) -> (i64, i64) {
    numbered_events_in(logs, GROUP).await
}

/// [`group_with_numbered_events`], for a case that needs a log group of its own — one it
/// changes or deletes under a query that already read it.
async fn numbered_events_in(logs: &Logs, group: &str) -> (i64, i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": group, "logStreamName": STREAM}),
    )
    .await;
    // Aligned on a whole second, because a query's window is stated in whole SECONDS.
    let base = base_ms() / 1_000 * 1_000;
    let messages: Vec<String> = (0..EVENTS).map(|n| format!("{{\"n\": {n}}}")).collect();
    let events: Vec<(i64, &str)> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (base + index as i64 * 1_000, message.as_str()))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": STREAM,
            "logEvents": batch(&events),
        }),
    )
    .await;
    (base / 1_000 - 60, now_ms() / 1_000 + 600)
}

/// Start one query and answer the `queryId` it minted.
async fn start(logs: &Logs, request: &Value) -> String {
    let answer = logs.ok(START, request).await;
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// The results of `query_id` once the query has reached `Complete`, polled for the way a
/// client polls for them ("If you see a value of Scheduled or Running for the status, you can
/// retry the operation later to see the final results").
async fn completed(logs: &Logs, case: &str, query_id: &str) -> Value {
    for _ in 0..POLL_ATTEMPTS {
        let answer = logs.ok(RESULTS, &json!({"queryId": query_id})).await;
        if answer["status"].as_str() == Some(QUERY_STATUS_COMPLETE) {
            return answer;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the query must reach {QUERY_STATUS_COMPLETE} within the polls allowed")
}

/// The rows of a results answer, each as the value it carries under `field`.
fn column(case: &str, answer: &Value, field: &str) -> Vec<String> {
    answer["results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the results are a list, got {answer}"))
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a row is a list of fields, got {row}"))
                .iter()
                .find(|pair| pair["field"].as_str() == Some(field))
                .and_then(|pair| pair["value"].as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `limit` the start names bounds the ROWS the query answers, whatever row bound the query
/// string itself carries.
#[tokio::test]
async fn the_starts_limit_parameter_bounds_the_rows_over_any_the_query_string_carries() {
    let logs = Logs::new();
    let (start_time, end_time) = group_with_numbered_events(&logs).await;
    let numbers = |count: usize| Some((0..count).map(|n| n.to_string()).collect::<Vec<String>>());
    let source = format!("source = logGroups(logGroupIdentifier: [\"{GROUP}\"])");

    let table: Vec<Bound> = vec![
        Bound {
            case: "a parameter and no bound in the query string",
            language: "CWLI",
            query: "fields n | sort n asc".to_string(),
            limit: Some(3),
            rows: 3,
            ordered: numbers(3),
        },
        Bound {
            case: "a parameter WIDER than the limit command",
            language: "CWLI",
            query: "fields n | sort n asc | limit 2".to_string(),
            limit: Some(5),
            rows: 5,
            ordered: numbers(5),
        },
        Bound {
            case: "a parameter NARROWER than the limit command",
            language: "CWLI",
            query: "fields n | sort n asc | limit 5".to_string(),
            limit: Some(3),
            rows: 3,
            ordered: numbers(3),
        },
        Bound {
            case: "a parameter equal to the limit command",
            language: "CWLI",
            query: "fields n | sort n asc | limit 4".to_string(),
            limit: Some(4),
            rows: 4,
            ordered: numbers(4),
        },
        Bound {
            case: "a parameter wider than the rows the query produced at all",
            language: "CWLI",
            query: "fields n | sort n asc | limit 2".to_string(),
            limit: Some(50),
            rows: EVENTS,
            ordered: numbers(EVENTS),
        },
        Bound {
            case: "no parameter, so the limit command decides",
            language: "CWLI",
            query: "fields n | sort n asc | limit 2".to_string(),
            limit: None,
            rows: 2,
            ordered: numbers(2),
        },
        Bound {
            case: "the smallest parameter the member admits",
            language: "CWLI",
            query: "fields n | sort n asc | limit 5".to_string(),
            limit: Some(LIMIT_RANGE_MIN),
            rows: 1,
            ordered: numbers(1),
        },
        Bound {
            case: "the largest parameter the member admits",
            language: "CWLI",
            query: "fields n | sort n asc | limit 2".to_string(),
            limit: Some(LIMIT_RANGE_MAX),
            rows: EVENTS,
            ordered: numbers(EVENTS),
        },
        Bound {
            case: "a parameter WIDER than a PPL head",
            language: "PPL",
            query: format!("{source} | fields n | head 2"),
            limit: Some(5),
            rows: 5,
            ordered: None,
        },
        Bound {
            case: "a parameter NARROWER than a PPL head",
            language: "PPL",
            query: format!("{source} | fields n | head 8"),
            limit: Some(3),
            rows: 3,
            ordered: None,
        },
        Bound {
            case: "no parameter, so a PPL head decides",
            language: "PPL",
            query: format!("{source} | fields n | head 2"),
            limit: None,
            rows: 2,
            ordered: None,
        },
        Bound {
            case: "a parameter WIDER than a SQL LIMIT",
            language: "SQL",
            query: format!("SELECT `n` FROM `{GROUP}` LIMIT 2"),
            limit: Some(5),
            rows: 5,
            ordered: None,
        },
        Bound {
            case: "a parameter NARROWER than a SQL LIMIT",
            language: "SQL",
            query: format!("SELECT `n` FROM `{GROUP}` LIMIT 8"),
            limit: Some(3),
            rows: 3,
            ordered: None,
        },
        Bound {
            case: "no parameter, so a SQL LIMIT decides",
            language: "SQL",
            query: format!("SELECT `n` FROM `{GROUP}` LIMIT 2"),
            limit: None,
            rows: 2,
            ordered: None,
        },
    ];

    for row in table {
        let case = row.case;
        let mut request = json!({
            "queryLanguage": row.language,
            "queryString": row.query,
            "startTime": start_time,
            "endTime": end_time,
        });
        // "The exception is queries using the OpenSearch Service SQL query language, where you
        // specify the log group names inside the querystring instead of here": a SQL statement
        // names the groups in its own `FROM` clause, so the request names none.
        if row.language != "SQL" {
            request["logGroupName"] = json!(GROUP);
        }
        if let Some(limit) = row.limit {
            request["limit"] = json!(limit);
        }
        let id = start(&logs, &request).await;
        let answer = completed(&logs, case, &id).await;
        let produced = column(case, &answer, "n");
        assert_eq!(
            produced.len(),
            row.rows,
            "case {case:?}: expected {} rows, got {produced:?}",
            row.rows
        );
        if let Some(expected) = row.ordered {
            assert_eq!(
                produced, expected,
                "case {case:?}: expected the rows {expected:?}, got {produced:?}"
            );
        }
        // "These values reflect the full raw results of the query": the bound is on the rows
        // returned, so every matching event is still counted.
        let statistics = &answer["statistics"];
        assert_eq!(
            statistics["recordsMatched"].as_f64(),
            Some(EVENTS as f64),
            "case {case:?}: a row bound does not change the events matched, got {answer}"
        );
        assert_eq!(
            statistics["resultCount"].as_f64(),
            Some(row.rows as f64),
            "case {case:?}: the result count is the rows the query produced, got {answer}"
        );
        // No page was asked for, so the rows the query produced are the one page answered.
        assert!(
            answer["nextToken"].is_null(),
            "case {case:?}: a page holding every row of the result continues nowhere, got {answer}"
        );
    }
}

/// A `queryId` is a durable handle on the result set its query PRODUCED: what the query
/// answers is the snapshot of its run, so changing the log groups it read afterwards — more
/// events, a deleted stream, a deleted group — changes neither its rows nor its statistics.
#[tokio::test]
async fn a_query_id_is_a_durable_handle_on_the_result_set_its_query_produced() {
    let logs = Logs::new();

    // (case, the log group of the case, the change made to it after the query ran, as the
    // operation and request a client makes it with) — each change is one a fresh run over the
    // store would answer differently.
    let changes: Vec<(&str, &str, &str, Value)> = vec![
        (
            "more events ingested into the queried window",
            "/insights/durable-ingest",
            "PutLogEvents",
            json!({"logStreamName": STREAM}),
        ),
        (
            "the queried log stream deleted",
            "/insights/durable-stream",
            "DeleteLogStream",
            json!({"logStreamName": STREAM}),
        ),
        (
            "the queried log group deleted",
            "/insights/durable-group",
            "DeleteLogGroup",
            json!({}),
        ),
    ];
    for (case, group, operation, request) in changes {
        let (start_time, end_time) = numbered_events_in(&logs, group).await;
        let id = start(
            &logs,
            &json!({
                "logGroupName": group,
                "queryString": "fields n | sort n asc",
                "startTime": start_time,
                "endTime": end_time,
            }),
        )
        .await;
        let produced = completed(&logs, case, &id).await;
        let rows = column(case, &produced, "n");
        assert_eq!(
            rows.len(),
            EVENTS,
            "case {case:?}: the query produced one row per stored event, got {rows:?}"
        );

        let mut body = request;
        body["logGroupName"] = json!(group);
        if operation == "PutLogEvents" {
            // Events inside the queried window, so a fresh run would match them.
            let base = start_time.saturating_add(120) * 1_000;
            body["logEvents"] =
                batch(&[(base, r#"{"n": 97}"#), (base + 1_000, r#"{"n": 98}"#)]);
        }
        logs.ok(operation, &body).await;

        // The same handle still answers the result set the query produced.
        let again = logs.ok(RESULTS, &json!({"queryId": &id})).await;
        assert_eq!(
            column(case, &again, "n"),
            rows,
            "case {case:?}: the handle answers the rows its query produced, got {again}"
        );
        assert_eq!(
            again["statistics"], produced["statistics"],
            "case {case:?}: the statistics are the counts over the run that produced the \
             rows, got {again}"
        );
        assert_eq!(
            again["status"].as_str(),
            Some(QUERY_STATUS_COMPLETE),
            "case {case:?}: the query stays complete, got {again}"
        );

        // So does a PAGE of it, continued by the token the page before it handed back.
        let first = logs
            .ok(RESULTS, &json!({"queryId": &id, "maxItems": 5}))
            .await;
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a partial page carries a token, got {first}"))
            .to_string();
        let second = logs
            .ok(
                RESULTS,
                &json!({"queryId": &id, "maxItems": 5, "nextToken": token}),
            )
            .await;
        let paged: Vec<String> = column(case, &first, "n")
            .into_iter()
            .chain(column(case, &second, "n"))
            .collect();
        assert_eq!(
            paged,
            rows[..paged.len()].to_vec(),
            "case {case:?}: the pages walk the rows the run produced, got {paged:?}"
        );
    }
}

/// A `limit` outside the member's modelled range is refused rather than clamped, at either end.
#[tokio::test]
async fn a_limit_outside_the_members_range_is_refused() {
    let logs = Logs::new();
    let (start_time, end_time) = group_with_numbered_events(&logs).await;

    // (case, the value just past one published end of `@range 1..100000`)
    for (case, limit) in [
        ("one below the smallest the member admits", LIMIT_RANGE_MIN - 1),
        ("one above the largest the member admits", LIMIT_RANGE_MAX + 1),
    ] {
        let (code, message) = logs
            .refusal(
                START,
                &json!({
                    "logGroupName": GROUP,
                    "queryString": "fields n",
                    "startTime": start_time,
                    "endTime": end_time,
                    "limit": limit,
                }),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert!(
            message.contains("'limit'"),
            "case {case:?}: the refusal names the member at fault, got {message:?}"
        );
    }
}

/// The sentence CloudWatch Logs refuses a `queryId` outside the length bounds the model
/// declares for one with. The 256 is the `@length` maximum of the model's `QueryId` shape
/// (1..256), which every `queryId` member of this service targets; one sentence covers both
/// ends of that bound, because an id shorter than the minimum is a blank one.
const QUERY_ID_LENGTH_TEXT: &str = "Query id is blank or exceeded max length 256";

/// A `queryId` outside the bounds the model declares for one is refused with the one sentence
/// this service words that bound with, whichever operation the id arrived on and whichever end
/// it breaks — while an id AT either bound reaches the handler and is answered by it.
#[tokio::test]
async fn a_query_id_outside_the_modelled_length_bounds_is_refused_with_its_own_sentence() {
    let logs = Logs::new();
    let (start_time, end_time) = group_with_numbered_events(&logs).await;
    let held = start(
        &logs,
        &json!({
            "logGroupName": GROUP,
            "queryString": "fields n",
            "startTime": start_time,
            "endTime": end_time,
        }),
    )
    .await;

    // Every operation whose input names a query id by the member that targets `QueryId`.
    let operations = [RESULTS, STOP, "ListLogGroupsForQuery"];

    // (case, an id outside the bounds): the blank id, one character past the maximum, and far
    // past it.
    let outside: Vec<(&str, String)> = vec![
        ("the blank id", String::new()),
        (
            "one character past the published maximum",
            "q".repeat(QUERY_ID_MAX_CHARS + 1),
        ),
        ("far past the published maximum", "q".repeat(4_000)),
    ];
    for operation in operations {
        for (case, query_id) in &outside {
            let case = format!("{operation}: {case}");
            let (code, message) = logs
                .refusal(operation, &json!({"queryId": query_id}))
                .await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
            );
            assert_eq!(
                message, QUERY_ID_LENGTH_TEXT,
                "case {case:?}: expected {QUERY_ID_LENGTH_TEXT:?}, got {message:?}"
            );
        }

        // An id AT either published bound is inside them, so the operation's own handler
        // answers it — for an id it holds no query under, with that operation's refusal.
        for (case, query_id) in [
            ("at the published minimum length", "q".to_string()),
            (
                "at the published maximum length",
                "q".repeat(QUERY_ID_MAX_CHARS),
            ),
        ] {
            let case = format!("{operation}: an id {case}");
            let (_, message) = logs
                .refusal(operation, &json!({"queryId": query_id}))
                .await;
            assert_ne!(
                message, QUERY_ID_LENGTH_TEXT,
                "case {case:?}: an id inside the bounds is answered by the handler, got \
                 {message:?}"
            );
        }
    }

    // The query this account does hold is still read by its own id, so the rule refused only
    // the ids outside the bounds.
    let answer = completed(&logs, "the held query", &held).await;
    assert_eq!(
        answer["status"].as_str(),
        Some(QUERY_STATUS_COMPLETE),
        "case the held query: its own id still reads its results, got {answer}"
    );
}

/// A `queryId` this account holds no query under is refused by `GetQueryResults` as an
/// incorrectly specified parameter, and by `StopQuery` as a missing resource: the two
/// operations model the same pair of errors and pick different members of it, in different
/// sentences.
#[tokio::test]
async fn an_unissued_results_handle_is_refused_as_an_incorrectly_specified_parameter() {
    let logs = Logs::new();
    let (start_time, end_time) = group_with_numbered_events(&logs).await;
    let held = start(
        &logs,
        &json!({
            "logGroupName": GROUP,
            "queryString": "fields n",
            "startTime": start_time,
            "endTime": end_time,
        }),
    )
    .await;

    // Each id names no query of this account: a well-formed UUID nothing issued, a string of
    // no particular form, a held id with one character changed, and the longest id the member
    // admits (`QueryId`, `@length 1..256`).
    let cases: Vec<(&str, String)> = vec![
        (
            "a well-formed UUID this service never issued",
            "0319d195-b8cf-44bc-afc3-2ce664d267bd".to_string(),
        ),
        ("an id of no particular form", "not-a-query-id".to_string()),
        (
            "a held id with its last character changed",
            format!("{}z", &held[..held.len() - 1]),
        ),
        (
            "the longest id the member admits",
            "q".repeat(QUERY_ID_MAX_CHARS),
        ),
    ];
    for (case, query_id) in cases {
        let (code, message) = logs.refusal(RESULTS, &json!({"queryId": query_id})).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message,
            query_results_invalid_query_id_message(&query_id),
            "case {case:?}: expected {:?}, got {message:?}",
            query_results_invalid_query_id_message(&query_id)
        );

        // The same id handed to `StopQuery` keeps that operation's own refusal: the error
        // class differs, and so does the sentence.
        let (stop_code, stop_message) = logs.refusal(STOP, &json!({"queryId": query_id})).await;
        assert_eq!(
            stop_code, NOT_FOUND,
            "case {case:?}: StopQuery refuses the same id as a missing resource, got \
             {stop_code} {stop_message:?}"
        );
        assert_ne!(
            stop_message, message,
            "case {case:?}: the two operations word this refusal differently, got \
             {stop_message:?} from both"
        );
    }
}

/// A `nextToken` `GetQueryResults` cannot resume this query's results from is refused with
/// this operation's own sentence, stated inside the SDK error envelope of the refusal.
#[tokio::test]
async fn a_token_that_resumes_no_position_in_this_querys_results_is_refused() {
    let logs = Logs::new();
    let (start_time, end_time) = group_with_numbered_events(&logs).await;
    let request = json!({
        "logGroupName": GROUP,
        "queryString": "fields n | sort n asc",
        "startTime": start_time,
        "endTime": end_time,
    });
    let id = start(&logs, &request).await;
    let other = start(&logs, &request).await;
    for (case, query_id) in [("the first query", &id), ("the second query", &other)] {
        completed(&logs, case, query_id).await;
    }

    // A token this service DID mint, for a position inside the FIRST query's results.
    let page = logs
        .ok(RESULTS, &json!({"queryId": &id, "maxItems": 5}))
        .await;
    let minted = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a partial page hands back a token, got {page}"))
        .to_string();

    // The forms a token takes that no position of the second query's results can be read out
    // of: one minted for another query, one this service never minted, one carrying another
    // enumeration's cursor kind, and a long run of text.
    let cases: Vec<(&str, String)> = vec![
        ("a token minted for another query", minted),
        ("a token this service never minted", "not-a-token".to_string()),
        (
            "a cursor minted under another kind",
            encode_token(&json!({"kind": "DescribeQueries", "after": "q"})),
        ),
        ("a long run of text", "z".repeat(300)),
    ];
    let expected = query_results_invalid_token_message();
    let sentence = without_sdk_envelope(
        "the expected wording",
        &expected,
        REFUSED_STATUS,
        INVALID_PARAMETER,
    );
    for (case, token) in cases {
        let (code, message) = logs
            .refusal(
                RESULTS,
                &json!({"queryId": &other, "maxItems": 5, "nextToken": token}),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        // The envelope states this response's own request id, so the sentence is compared with
        // the envelope — asserted to report this refusal's service, status and code — removed.
        let said = without_sdk_envelope(case, &message, REFUSED_STATUS, INVALID_PARAMETER);
        assert_eq!(
            said, sentence,
            "case {case:?}: expected the sentence {sentence:?}, got {said:?}"
        );
    }
}
