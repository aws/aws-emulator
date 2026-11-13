// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetQueryResults`: reading back the results of a CloudWatch Logs
//! Insights query that `StartQuery` started.
//!
//! The rules under test:
//!
//! * the results of a query are COMPUTED from the log events the queried log groups hold
//!   inside the query's time range — every row's values come from an event this store was
//!   given, and the pipeline's commands (`fields`, `filter`, `parse`, `stats`, `sort`,
//!   `dedup`, `limit`) decide which rows and which columns a result carries;
//! * each row of log records carries the `@ptr` identifying the record it came from, and
//!   `@timestamp` is the instant of that record;
//! * the same events are queried by each of the three query languages the model declares
//!   (`CWLI`, `PPL`, `SQL`), and a SQL statement's `FROM` clause names the log groups it
//!   reads itself;
//! * the status reported is the one the query has already reached on the clock, and only a
//!   query that has started has results;
//! * `maxItems` pages the results and the `nextToken` of one query's results continues that
//!   query's enumeration and no other;
//! * a `queryId` this account holds no query under is refused as an incorrectly specified
//!   parameter;
//! * reading the results changes nothing a later caller sees.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rules generalise over rather than one query: which command of
//! the pipeline produces the rows, whether a result is log records or an aggregation, which
//! language the query is written in, and which status the query is in. Any one query string
//! an issue happens to mention is a single row.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    insights::parse_timestamp, query_results_invalid_query_id_message, QUERY_STATUS_CANCELLED,
    QUERY_STATUS_COMPLETE, QUERY_STATUS_FAILED, QUERY_STATUS_SCHEDULED,
};

use crate::harness::{base_ms, batch, now_ms, Logs, REGION};

/// The operations under test.
const START: &str = "StartQuery";
const RESULTS: &str = "GetQueryResults";

/// The modelled error code the rules name.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The log group every case queries, and the one no case writes an event into.
const GROUP: &str = "/insights/results";
const OTHER_GROUP: &str = "/insights/results-other";

/// The stream the events are written to.
const STREAM: &str = "stream-one";

/// How long a case waits before treating a started query as finished: past both spans the
/// start armed (`QUERY_RUNNING_DELAY_MS` + `QUERY_COMPLETION_DELAY_MS`), with room for the
/// test's own scheduling.
fn past_completion() -> Duration {
    Duration::from_millis(900)
}

/// The events every case queries, as `(offset from the base instant, message)` pairs.
///
/// Three are JSON objects, so the fields CloudWatch Logs discovers in them — `level`,
/// `duration`, `user` — are queryable; the fourth is bare text, so it carries `@message` and
/// none of those fields, which is what makes a query over an absent field a case of its own.
const EVENTS: [(i64, &str); 4] = [
    (1_000, r#"{"level":"ERROR","duration":120,"user":"amy"}"#),
    (2_000, r#"{"level":"INFO","duration":30,"user":"bob"}"#),
    (3_000, r#"{"level":"ERROR","duration":480,"user":"amy"}"#),
    (4_000, "plain text WARN 42"),
];

/// A log group holding [`EVENTS`], and the window a query of it is started with in the epoch
/// SECONDS `StartQuery` documents for its range.
async fn group_with_events(logs: &Logs) -> (i64, i64, i64) {
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
    )
    .await;
    // Aligned on a whole second, because a query's window is stated in whole SECONDS: an
    // event stamped on a second boundary falls inside or outside a window by the rule under
    // test rather than by where the test's own clock happened to land inside a second.
    let base = base_ms() / 1_000 * 1_000;
    let events: Vec<(i64, &str)> = EVENTS
        .iter()
        .map(|(offset, message)| (base + offset, *message))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&events),
        }),
    )
    .await;
    // The window covers every event and the instant the query is started at.
    (base, base / 1_000 - 60, now_ms() / 1_000 + 60)
}

/// Start one query and answer the `queryId` it minted.
async fn start(logs: &Logs, request: &Value) -> String {
    let answer = logs.ok(START, request).await;
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// The rows of a `GetQueryResults` answer, each as the field/value pairs it carries.
fn rows(answer: &Value) -> Vec<BTreeMap<String, String>> {
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

/// The value of one field of one row, or the empty string when the row does not carry it.
fn value(row: &BTreeMap<String, String>, field: &str) -> String {
    row.get(field).cloned().unwrap_or_default()
}

/// Start `query_string` over [`GROUP`], wait for it to finish and answer its results.
async fn completed(logs: &Logs, window: (i64, i64), query_string: &str) -> Value {
    let id = start(
        logs,
        &json!({
            "logGroupName": GROUP,
            "startTime": window.0,
            "endTime": window.1,
            "queryString": query_string,
        }),
    )
    .await;
    tokio::time::sleep(past_completion()).await;
    logs.ok(RESULTS, &json!({"queryId": id})).await
}

/// One case of the pipeline table: what the case is, the query string it starts, and the
/// `(field, value)` pairs every row it must produce carries.
type PipelineCase<'a> = (&'a str, &'a str, Vec<Vec<(&'a str, &'a str)>>);

/// One case of the statistics table: what the case is, the window the query reads, the query
/// string, and the records it must scan and match.
type StatisticsCase<'a> = (&'a str, (i64, i64), &'a str, f64, f64);

/// One case of the query-language table: what the case is, the language, the request that
/// starts it, and the rows it must produce.
type LanguageCase<'a> = (&'a str, &'a str, Value, Vec<Vec<(&'a str, &'a str)>>);

/// Each pipeline command produces the rows it computes from the stored events.
#[tokio::test]
async fn the_results_are_computed_from_the_stored_log_events() {
    let logs = Logs::new();
    let (base, start_time, end_time) = group_with_events(&logs).await;
    let window = (start_time, end_time);

    // (case, the query string, the rows it must produce as `(field, value)` pairs each row
    // must carry)
    let cases: Vec<PipelineCase<'_>> = vec![
        (
            "a filter over the raw message, newest event first",
            r"filter @message like /ERROR/",
            vec![
                vec![("@message", EVENTS[2].1)],
                vec![("@message", EVENTS[0].1)],
            ],
        ),
        (
            "a filter on a discovered field, with the fields it selects",
            r#"fields level, duration | filter level = "ERROR" | sort duration asc"#,
            vec![
                vec![("level", "ERROR"), ("duration", "120")],
                vec![("level", "ERROR"), ("duration", "480")],
            ],
        ),
        (
            "a filter comparing a discovered field as a number",
            "fields duration | filter duration > 100 | sort duration desc",
            vec![vec![("duration", "480")], vec![("duration", "120")]],
        ),
        (
            "a filter on a field the bare-text event does not carry",
            "fields user | filter not ispresent(level) | fields @message",
            vec![vec![("@message", EVENTS[3].1)]],
        ),
        (
            "an aggregation over every scanned event",
            "stats count(*) as events, avg(duration) as mean, max(duration) as worst",
            vec![vec![
                ("events", "4"),
                ("mean", "210"),
                ("worst", "480"),
            ]],
        ),
        (
            "an aggregation grouped by a discovered field",
            r#"filter ispresent(level) | stats count(*) as hits by level | sort hits desc"#,
            vec![
                vec![("level", "ERROR"), ("hits", "2")],
                vec![("level", "INFO"), ("hits", "1")],
            ],
        ),
        (
            "a distinct count of a discovered field",
            "stats count_distinct(user) as users, count(user) as with_user",
            vec![vec![("users", "2"), ("with_user", "3")]],
        ),
        (
            "a parse of the bare-text message into an ephemeral field",
            r"filter @message like /WARN/ | parse @message /WARN (?<code>\d+)/ | display code",
            vec![vec![("code", "42")]],
        ),
        (
            "a glob parse naming its fields",
            r#"filter @message like /WARN/ | parse @message "plain * WARN *" as word, code | display word, code"#,
            vec![vec![("word", "text"), ("code", "42")]],
        ),
        (
            "a deduplication of a discovered field",
            "filter ispresent(user) | fields user | dedup user | sort user asc",
            vec![vec![("user", "amy")], vec![("user", "bob")]],
        ),
        (
            "a limit on the rows returned",
            "fields @message | limit 1",
            vec![vec![("@message", EVENTS[3].1)]],
        ),
        (
            // Every event is stamped on its own whole second, so a one-second span puts each
            // into a bucket of its own, keyed by the instant that bucket starts at.
            "an aggregation grouped into one-second spans",
            "stats count(*) as events by bin(1s)",
            vec![
                vec![("events", "1")],
                vec![("events", "1")],
                vec![("events", "1")],
                vec![("events", "1")],
            ],
        ),
        (
            "an expression defining a field",
            r#"filter level = "INFO" | fields duration / 10 as tenths"#,
            vec![vec![("tenths", "3")]],
        ),
        (
            "a filter whose field is absent from every event",
            "fields @message | filter missingField = \"x\"",
            vec![],
        ),
    ];
    for (case, query_string, expected) in cases {
        let answer = completed(&logs, window, query_string).await;
        let produced = rows(&answer);
        assert_eq!(
            answer["status"].as_str(),
            Some(QUERY_STATUS_COMPLETE),
            "case {case:?}: a query past the spans the start armed is {QUERY_STATUS_COMPLETE}, \
             got {answer}"
        );
        assert_eq!(
            produced.len(),
            expected.len(),
            "case {case:?}: expected {} rows, got {produced:?}",
            expected.len()
        );
        for (index, wanted) in expected.iter().enumerate() {
            let row = &produced[index];
            for (field, expected_value) in wanted {
                assert_eq!(
                    &value(row, field),
                    expected_value,
                    "case {case:?}: row {index} must carry {field}={expected_value}, got {row:?}"
                );
            }
        }
    }

    // A span group is keyed by the instant its bucket starts at, in the same rendering every
    // other timestamp of a result carries.
    let binned = completed(&logs, window, "stats count(*) as events by bin(1s)").await;
    let produced = rows(&binned);
    let bins: Vec<Option<i64>> = produced
        .iter()
        .map(|row| parse_timestamp(&value(row, "bin(1s)")))
        .collect();
    assert_eq!(
        bins,
        EVENTS
            .iter()
            .map(|(offset, _)| Some(base + offset))
            .collect::<Vec<Option<i64>>>(),
        "case one-second spans: each bucket is keyed by the instant it starts at, got \
         {produced:?}"
    );

    // Every row of LOG RECORDS carries the pointer to the record it came from, and each
    // pointer is that record's own; a row of an AGGREGATION is no single log record and
    // carries none.
    let answer = completed(&logs, window, "fields @message, @timestamp").await;
    let produced = rows(&answer);
    let mut pointers: Vec<String> = Vec::new();
    for (index, row) in produced.iter().enumerate() {
        let pointer = value(row, "@ptr");
        assert!(
            parse_timestamp(&value(row, "@timestamp")).is_some(),
            "case row {index}: a log record row carries the instant of its own record, got {row:?}"
        );
        assert!(
            !pointer.is_empty() && !pointers.contains(&pointer),
            "case row {index}: every log record row carries its own @ptr, got {row:?}"
        );
        pointers.push(pointer);
    }
    assert_eq!(
        pointers.len(),
        EVENTS.len(),
        "case the pointers: one row per event of the window, got {produced:?}"
    );
    let aggregated = completed(&logs, window, "stats count(*) as events").await;
    assert!(
        rows(&aggregated)
            .iter()
            .all(|row| !row.contains_key("@ptr")),
        "case an aggregation: a group is no log record, so it carries no @ptr, got {aggregated}"
    );

    // The default columns of a query that selects none are the event's timestamp and its
    // message, and the timestamp is the instant the event was stamped with.
    let answer = completed(&logs, window, "filter @message like /WARN/").await;
    let produced = rows(&answer);
    let row = produced
        .first()
        .unwrap_or_else(|| panic!("the filter selects the bare-text event, got {produced:?}"));
    assert_eq!(
        parse_timestamp(&value(row, "@timestamp")),
        Some(base + EVENTS[3].0),
        "case the default columns: @timestamp is the instant of the event, got {row:?}"
    );
    assert_eq!(
        value(row, "@message"),
        EVENTS[3].1,
        "case the default columns: @message is the event's own text, got {row:?}"
    );
}

/// The statistics are the counts over the run, and the window selects which events run.
#[tokio::test]
async fn the_statistics_count_what_the_run_read_and_matched() {
    let logs = Logs::new();
    let (base, start_time, end_time) = group_with_events(&logs).await;

    // (case, the window, the query string, scanned, matched)
    let cases: Vec<StatisticsCase<'_>> = vec![
        (
            "every event of the group",
            (start_time, end_time),
            "fields @message",
            4.0,
            4.0,
        ),
        (
            "a filter selecting two of the four",
            (start_time, end_time),
            r"filter @message like /ERROR/",
            4.0,
            2.0,
        ),
        (
            "a window holding none of the events",
            (0, 1_000),
            "fields @message",
            0.0,
            0.0,
        ),
        (
            // The window is inclusive at both ends and is stated in whole SECONDS, so one
            // ending on the second the second event falls in holds exactly the first two.
            "a window ending on the second of the second event",
            (start_time, (base + EVENTS[1].0) / 1_000),
            "fields @message",
            2.0,
            2.0,
        ),
    ];
    for (case, window, query_string, scanned, matched) in cases {
        let answer = completed(&logs, window, query_string).await;
        let statistics = &answer["statistics"];
        assert_eq!(
            statistics["recordsScanned"].as_f64(),
            Some(scanned),
            "case {case:?}: expected {scanned} records scanned, got {answer}"
        );
        assert_eq!(
            statistics["recordsMatched"].as_f64(),
            Some(matched),
            "case {case:?}: expected {matched} records matched, got {answer}"
        );
        assert_eq!(
            statistics["resultCount"].as_f64(),
            Some(rows(&answer).len() as f64),
            "case {case:?}: the result count is the rows the query produced, got {answer}"
        );
        assert_eq!(
            statistics["logGroupsScanned"].as_f64(),
            Some(1.0),
            "case {case:?}: one log group was queried, got {answer}"
        );
    }

    // The bytes scanned are the bytes of the messages that were read.
    let answer = completed(&logs, (start_time, end_time), "fields @message").await;
    let expected: f64 = EVENTS
        .iter()
        .map(|(_, message)| message.len() as f64)
        .sum();
    assert_eq!(
        answer["statistics"]["bytesScanned"].as_f64(),
        Some(expected),
        "case every event: the bytes scanned are the messages that were read, got {answer}"
    );
}

/// Each query language runs over the same stored events.
#[tokio::test]
async fn every_query_language_queries_the_same_events() {
    let logs = Logs::new();
    let (base, start_time, end_time) = group_with_events(&logs).await;
    // A log group whose name carries capitals, so a source selector that names it has to
    // carry the name as written: a query of the lower-cased name reads no group at all.
    const MIXED_CASE_GROUP: &str = "/Insights/MixedCase";
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": MIXED_CASE_GROUP}),
    )
    .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": MIXED_CASE_GROUP, "logStreamName": STREAM}),
    )
    .await;
    const MIXED_CASE_MESSAGE: &str = r#"{"level":"ERROR","user":"cat"}"#;
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": MIXED_CASE_GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&[(base + 1_000, MIXED_CASE_MESSAGE)]),
        }),
    )
    .await;

    // (case, the language, the request, the rows it must produce)
    let cases: Vec<LanguageCase<'_>> = vec![
        (
            // The selector is written with no space around its `=` and names a log group
            // whose name carries capitals, so the stage's arguments are read exactly as the
            // query string wrote them.
            "a PPL pipeline whose source selector names one log group by identifier",
            "PPL",
            json!({
                "logGroupName": GROUP,
                "queryString": format!(
                    "source=logGroups(logGroupIdentifier:[\"{MIXED_CASE_GROUP}\"])| fields `@message`"
                ),
            }),
            vec![vec![("@message", MIXED_CASE_MESSAGE)]],
        ),
        (
            "a PPL pipeline with a where and a stats",
            "PPL",
            json!({
                "logGroupName": GROUP,
                "queryString":
                    "source = logGroups(namePrefix: [\"/insights/\"]) | where level = 'ERROR' \
                     | stats count() as hits by user",
            }),
            vec![vec![("user", "amy"), ("hits", "2")]],
        ),
        (
            "a PPL pipeline whose field names are back-quoted",
            "PPL",
            json!({
                "logGroupName": GROUP,
                "queryString":
                    "source = logGroups(namePrefix: [\"/insights/results\"]) \
                     | where `level` = 'INFO' | stats avg(`duration`) as mean by `user`",
            }),
            vec![vec![("user", "bob"), ("mean", "30")]],
        ),
        (
            "a PPL pipeline bounded with head",
            "PPL",
            json!({
                "logGroupName": GROUP,
                "queryString": "source = logGroups(namePrefix: [\"/insights/\"]) \
                                | where duration > 100 | fields duration | sort duration | head 1",
            }),
            vec![vec![("duration", "120")]],
        ),
        (
            // PPL writes the same grouping into spans of time as `span(<field>, <width>)`.
            "a PPL pipeline grouping into one-second spans",
            "PPL",
            json!({
                "logGroupName": GROUP,
                "queryString": "source = logGroups(namePrefix: [\"/insights/results\"]) \
                                | stats count() as n by span(`@timestamp`, 1s)",
            }),
            vec![
                vec![("n", "1")],
                vec![("n", "1")],
                vec![("n", "1")],
                vec![("n", "1")],
            ],
        ),
        (
            // "you specify the log group names inside the querystring instead of here": the
            // FROM clause names the log groups the statement reads, so it decides them even
            // when the request also named one — here a group holding no event at all.
            "a SQL statement whose FROM clause overrides the request's log groups",
            "SQL",
            json!({
                "logGroupIdentifiers": [OTHER_GROUP],
                "queryString": format!(
                    "SELECT `level`, count(*) AS hits FROM `{GROUP}` \
                     WHERE `level` = 'ERROR' GROUP BY `level`"
                ),
            }),
            vec![vec![("level", "ERROR"), ("hits", "2")]],
        ),
        (
            "a SQL statement selecting rows with an ordering and a bound",
            "SQL",
            json!({
                "queryString": format!(
                    "SELECT `user`, `duration` FROM `{GROUP}` WHERE `duration` >= 120 \
                     ORDER BY `duration` DESC LIMIT 1"
                ),
            }),
            vec![vec![("user", "amy"), ("duration", "480")]],
        ),
        (
            "a SQL statement matching text with LIKE",
            "SQL",
            json!({
                "queryString": format!(
                    "SELECT `@message` FROM `{GROUP}` WHERE `@message` LIKE '%WARN%'"
                ),
            }),
            vec![vec![("@message", EVENTS[3].1)]],
        ),
        (
            // "COUNT(DISTINCT column)" counts the column's distinct values, which is the
            // aggregate the pipeline languages spell `count_distinct`.
            "a SQL statement counting DISTINCT values of a column",
            "SQL",
            json!({
                "queryString": format!(
                    "SELECT count(DISTINCT `user`) AS users, count(*) AS rows_read \
                     FROM `{GROUP}`"
                ),
            }),
            vec![vec![("users", "2"), ("rows_read", "4")]],
        ),
    ];
    for (case, language, request, expected) in cases {
        let mut body = request;
        body["queryLanguage"] = json!(language);
        for (member, instant) in [("startTime", start_time), ("endTime", end_time)] {
            body[member] = json!(instant);
        }
        let id = start(&logs, &body).await;
        tokio::time::sleep(past_completion()).await;
        let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
        assert_eq!(
            answer["queryLanguage"].as_str(),
            Some(language),
            "case {case:?}: the language reported is the one the query was started with, got \
             {answer}"
        );
        assert_eq!(
            answer["status"].as_str(),
            Some(QUERY_STATUS_COMPLETE),
            "case {case:?}: expected {QUERY_STATUS_COMPLETE}, got {answer}"
        );
        let produced = rows(&answer);
        assert_eq!(
            produced.len(),
            expected.len(),
            "case {case:?}: expected {} rows, got {produced:?}",
            expected.len()
        );
        for (index, wanted) in expected.iter().enumerate() {
            for (field, expected_value) in wanted {
                assert_eq!(
                    &value(&produced[index], field),
                    expected_value,
                    "case {case:?}: row {index} must carry {field}={expected_value}, got {:?}",
                    produced[index]
                );
            }
        }
    }
}

/// The status reported is the one the query has reached, and only a started query has rows.
#[tokio::test]
async fn the_status_reported_is_the_one_the_query_has_reached() {
    let logs = Logs::new();
    let (_, start_time, end_time) = group_with_events(&logs).await;
    let request = json!({
        "logGroupName": GROUP,
        "startTime": start_time,
        "endTime": end_time,
        "queryString": "fields @message",
    });

    // Straight after the start the query has not run, so it has no results yet.
    let id = start(&logs, &request).await;
    let scheduled = logs.ok(RESULTS, &json!({"queryId": id})).await;
    assert_eq!(
        scheduled["status"].as_str(),
        Some(QUERY_STATUS_SCHEDULED),
        "case straight after the start: a query that has not run yet is \
         {QUERY_STATUS_SCHEDULED}, got {scheduled}"
    );
    assert!(
        rows(&scheduled).is_empty(),
        "case straight after the start: a query that has not run has no results, got {scheduled}"
    );
    assert_eq!(
        scheduled["statistics"]["recordsScanned"].as_f64(),
        Some(0.0),
        "case straight after the start: nothing was scanned, got {scheduled}"
    );

    tokio::time::sleep(past_completion()).await;
    let complete = logs.ok(RESULTS, &json!({"queryId": id})).await;
    assert_eq!(
        complete["status"].as_str(),
        Some(QUERY_STATUS_COMPLETE),
        "case past the spans the start armed: expected {QUERY_STATUS_COMPLETE}, got {complete}"
    );
    assert_eq!(
        rows(&complete).len(),
        EVENTS.len(),
        "case past the spans the start armed: every event of the window is a row, got {complete}"
    );

    // Reading the results is a READ: calling it twice answers the same thing.
    let again = logs.ok(RESULTS, &json!({"queryId": id})).await;
    assert_eq!(
        complete, again,
        "case twice: reading the results changes nothing a later caller sees"
    );

    // A query stopped before it ran reports the status it was stopped into, and no rows.
    let stopped = start(&logs, &request).await;
    logs.ok("StopQuery", &json!({"queryId": stopped})).await;
    let cancelled = logs.ok(RESULTS, &json!({"queryId": stopped})).await;
    assert_eq!(
        cancelled["status"].as_str(),
        Some(QUERY_STATUS_CANCELLED),
        "case a stopped query: expected {QUERY_STATUS_CANCELLED}, got {cancelled}"
    );
    assert!(
        rows(&cancelled).is_empty(),
        "case a stopped query: a query that was stopped produced no results, got {cancelled}"
    );

    // "If you associated an KMS key with the CloudWatch Logs Insights query results in this
    // account, this field displays the ARN of the key that's used to encrypt the query
    // results": no association, no key.
    assert!(
        complete["encryptionKey"].is_null(),
        "case no associated key: the results report no encryption key, got {complete}"
    );
    let key = format!("arn:aws:kms:{REGION}:123456789012:key/insights-results");
    logs.ok(
        "AssociateKmsKey",
        &json!({
            "resourceIdentifier": engine::emulator_arn("logs", REGION, "query-result:*"),
            "kmsKeyId": key,
        }),
    )
    .await;
    let encrypted = logs.ok(RESULTS, &json!({"queryId": id})).await;
    assert_eq!(
        encrypted["encryptionKey"].as_str(),
        Some(key.as_str()),
        "case an associated key: the results report the key that encrypts them, got {encrypted}"
    );
}

/// `maxItems` pages the results, and a token continues the enumeration it belongs to.
#[tokio::test]
async fn the_results_are_paged_by_the_items_a_request_asks_for() {
    let logs = Logs::new();
    let (_, start_time, end_time) = group_with_events(&logs).await;
    let request = json!({
        "logGroupName": GROUP,
        "startTime": start_time,
        "endTime": end_time,
        "queryString": "fields @message",
    });
    let id = start(&logs, &request).await;
    let other = start(&logs, &request).await;
    tokio::time::sleep(past_completion()).await;

    // (case, the page size, the sizes of the pages it hands back)
    let cases: [(&str, i64, Vec<usize>); 3] = [
        ("one row at a time", 1, vec![1, 1, 1, 1]),
        ("two rows at a time", 2, vec![2, 2]),
        ("more rows than the query produced", 10, vec![4]),
    ];
    for (case, page_size, expected) in cases {
        let mut sizes: Vec<usize> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"queryId": id, "maxItems": page_size});
            if let Some(token) = token.as_deref() {
                body["nextToken"] = json!(token);
            }
            let answer = logs.ok(RESULTS, &body).await;
            let page = rows(&answer);
            sizes.push(page.len());
            for row in &page {
                let pointer = value(row, "@ptr");
                assert!(
                    !seen.contains(&pointer),
                    "case {case:?}: a paged enumeration reports each record once, got {pointer}"
                );
                seen.push(pointer);
            }
            token = answer["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            sizes, expected,
            "case {case:?}: expected pages of {expected:?}, got {sizes:?}"
        );
        assert_eq!(
            seen.len(),
            EVENTS.len(),
            "case {case:?}: every row of the result is reported across the pages, got {seen:?}"
        );
    }

    // A token is a position inside ONE query's results: handed to another query, it names no
    // position at all.
    let first = logs
        .ok(RESULTS, &json!({"queryId": id, "maxItems": 1}))
        .await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a partial page carries a token, got {first}"))
        .to_string();
    for (case, body) in [
        (
            "a token minted for another query",
            json!({"queryId": other, "nextToken": token}),
        ),
        (
            "a token this service did not mint",
            json!({"queryId": id, "nextToken": "not-a-token"}),
        ),
    ] {
        let code = logs.err(RESULTS, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code}"
        );
    }
}

/// A query string whose expressions this service cannot run reports the modelled `Failed`
/// status, and the service goes on serving every other caller.
///
/// The query languages of CloudWatch Logs Insights are AWS's and OpenSearch's own pipeline
/// languages, so their expressions are read by this service itself (the SQL one is parsed by
/// `sqlparser`, the regular expressions by `regex`). A caller writes those expressions, so
/// this pins what a malformed, truncated, oversized or deeply nested one does: the query is
/// answered with the status of an execution that produced nothing, never with a panic, a hang
/// or a plausible wrong answer.
#[tokio::test]
async fn a_query_string_the_service_cannot_run_reports_the_failed_status() {
    let logs = Logs::new();
    let (_, start_time, end_time) = group_with_events(&logs).await;
    let source = format!("source = logGroups(namePrefix: [\"{GROUP}\"])");

    // (case, the language, the query string)
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "an expression with a bracket nothing closes",
            "CWLI",
            "filter (".to_string(),
        ),
        (
            "an expression truncated at its operator",
            "CWLI",
            "fields @message | filter @message =".to_string(),
        ),
        (
            "an expression nesting deeper than the service evaluates",
            "CWLI",
            format!("filter {}1{}", "(".repeat(200), ")".repeat(200)),
        ),
        (
            // A FLAT chain nests no brackets, so it is bounded by the number of parts an
            // expression may hold rather than by its depth.
            "an expression chaining more parts than the service evaluates",
            "CWLI",
            format!("filter 1{}", " or 1".repeat(400)),
        ),
        (
            // Nearly the whole of what the query string may carry, and every bracket of it
            // unclosed: the parse is bounded by its nesting limit rather than by the input.
            "a bracket run filling the query string",
            "CWLI",
            format!("filter {}", "(".repeat(9_000)),
        ),
        (
            "a stats command naming no aggregate",
            "CWLI",
            "stats by level".to_string(),
        ),
        (
            // The language defines `diff` — so `StartQuery` accepts it — but it compares the
            // window with the PREVIOUS window of the same length, which this run does not
            // read: the query produces nothing rather than the rows of the one window.
            "a command the query language defines and this service does not run",
            "CWLI",
            "fields @message | diff".to_string(),
        ),
        (
            "a function the query language defines and this service does not run",
            "CWLI",
            "fields substr(@message, 0, 5) as head".to_string(),
        ),
        (
            "a filter calling a function this service does not run",
            "CWLI",
            "filter sqrt(duration) > 2".to_string(),
        ),
        (
            "a PPL pipeline truncated at a where command",
            "PPL",
            format!("{source} | where"),
        ),
        (
            "a SQL statement calling a function this service does not run",
            "SQL",
            format!("SELECT lower(`level`) AS l FROM `{GROUP}`"),
        ),
        (
            "a SQL statement whose condition carries a subquery",
            "SQL",
            format!(
                "SELECT `level` FROM `{GROUP}` WHERE `level` IN (SELECT `level` FROM `{GROUP}`)"
            ),
        ),
        (
            // `DISTINCT` under an aggregate this service does not compute distinctly is
            // refused rather than counted as though it were not there.
            "a SQL statement summing DISTINCT values",
            "SQL",
            format!("SELECT sum(DISTINCT `duration`) AS total FROM `{GROUP}`"),
        ),
        (
            "a SQL statement whose aggregate is a window function",
            "SQL",
            format!(
                "SELECT `user`, count(*) OVER (PARTITION BY `user`) AS hits FROM `{GROUP}`"
            ),
        ),
        (
            "a PPL search command carrying a condition",
            "PPL",
            format!("search source=logGroups(namePrefix: [\"{GROUP}\"]) level = 'ERROR'"),
        ),
    ];
    for (case, language, query_string) in cases {
        let mut body = json!({
            "queryLanguage": language,
            "startTime": start_time,
            "endTime": end_time,
            "queryString": query_string,
        });
        // The SQL language names the log groups to read inside the statement ("you specify
        // the log group names inside the querystring instead of here",
        // `StartQuery$logGroupName`), so a SQL case names none in the request; every other
        // language names them there.
        if language != "SQL" {
            body["logGroupName"] = json!(GROUP);
        }
        let id = start(&logs, &body).await;
        tokio::time::sleep(past_completion()).await;
        let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
        assert_eq!(
            answer["status"].as_str(),
            Some(QUERY_STATUS_FAILED),
            "case {case:?}: a query this service cannot run is {QUERY_STATUS_FAILED}, got {answer}"
        );
        assert!(
            rows(&answer).is_empty(),
            "case {case:?}: a query that produced nothing reports no rows, got {answer}"
        );
    }

    // The service survived every one of them: a query that does run still answers its rows.
    let answer = completed(&logs, (start_time, end_time), "fields @message").await;
    assert_eq!(
        rows(&answer).len(),
        EVENTS.len(),
        "case after the malformed queries: the service still answers a query it can run, got \
         {answer}"
    );

    // The query string itself is bounded by the model (`QueryString`, `@length(0..10000)`),
    // so one character past that maximum is refused before any of this runs.
    let (status, body) = logs
        .call(
            START,
            &json!({
                "logGroupName": GROUP,
                "startTime": start_time,
                "endTime": end_time,
                "queryString": format!("fields @message | filter @message like \"{}\"", "x".repeat(10_001)),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a query string past its modelled maximum: expected HTTP 400, got {status} with \
         body {body}"
    );
}

/// A query that would COMPUTE more than the run's budget admits is abandoned, and reports the
/// `Failed` status of an execution that produced nothing.
///
/// A value a query computes is not bounded by the log event it came from — a concatenation is
/// as long as its operands make it — so this pins that the service bounds it rather than
/// allocating whatever the statement asks for.
#[tokio::test]
async fn a_query_computing_more_than_the_budget_admits_is_abandoned() {
    let logs = Logs::new();
    let (_, start_time, end_time) = group_with_events(&logs).await;
    const BIG_GROUP: &str = "/insights/results-big";
    logs.ok("CreateLogGroup", &json!({"logGroupName": BIG_GROUP}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": BIG_GROUP, "logStreamName": STREAM}),
    )
    .await;
    // One event whose message is a sixteenth of a megabyte: a handful of concatenations of it
    // is past the bound on one computed value.
    let message = "x".repeat(64 * 1024);
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": BIG_GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&[(base_ms(), message.as_str())]),
        }),
    )
    .await;

    let concatenated = vec!["`@message`"; 20].join(" || ");
    let id = start(
        &logs,
        &json!({
            "queryLanguage": "SQL",
            "startTime": start_time,
            "endTime": end_time,
            "queryString": format!("SELECT {concatenated} AS big FROM `{BIG_GROUP}`"),
        }),
    )
    .await;
    tokio::time::sleep(past_completion()).await;
    let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
    assert_eq!(
        answer["status"].as_str(),
        Some(QUERY_STATUS_FAILED),
        "case a statement computing past the value bound: expected {QUERY_STATUS_FAILED}, got \
         {answer}"
    );
    assert!(
        rows(&answer).is_empty(),
        "case a statement computing past the value bound: an abandoned run reports no rows, got \
         {answer}"
    );

    // The same statement over one concatenation stays inside the bound and answers its row,
    // so what the bound refuses is the SIZE and not the operator.
    let id = start(
        &logs,
        &json!({
            "queryLanguage": "SQL",
            "startTime": start_time,
            "endTime": end_time,
            "queryString":
                format!("SELECT `@message` || `@message` AS pair FROM `{BIG_GROUP}`"),
        }),
    )
    .await;
    tokio::time::sleep(past_completion()).await;
    let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
    let produced = rows(&answer);
    assert_eq!(
        produced.len(),
        1,
        "case one concatenation: the statement answers the event's row, got {answer}"
    );
    assert_eq!(
        value(&produced[0], "pair").len(),
        message.len() * 2,
        "case one concatenation: the computed value is both copies of the message"
    );
}

/// A sort orders by one TOTAL order: numbers as numbers, text as text, every number before
/// every piece of text, and a value the record does not carry after every value it does.
///
/// The rule matters beyond the order it produces: a comparison that ordered `9` before `10`
/// as numbers and `"10"` before `"1a"` as text would cycle, and a sort driven by a
/// comparison that cycles has no defined result.
#[tokio::test]
async fn a_sort_orders_numbers_before_text_and_absent_values_last() {
    let logs = Logs::new();
    let (_, start_time, end_time) = group_with_events(&logs).await;
    const MIXED_GROUP: &str = "/insights/results-mixed";
    logs.ok("CreateLogGroup", &json!({"logGroupName": MIXED_GROUP}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": MIXED_GROUP, "logStreamName": STREAM}),
    )
    .await;
    // One field holding a number, a larger number, text that opens with a digit, and nothing
    // at all — the four cases the order has to place against each other.
    let base = base_ms() / 1_000 * 1_000;
    let events: [(i64, &str); 4] = [
        (base + 1_000, r#"{"size":9}"#),
        (base + 2_000, r#"{"size":10}"#),
        (base + 3_000, r#"{"size":"1a"}"#),
        (base + 4_000, r#"{"other":"none"}"#),
    ];
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": MIXED_GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&events),
        }),
    )
    .await;

    let id = start(
        &logs,
        &json!({
            "logGroupName": MIXED_GROUP,
            "startTime": start_time,
            "endTime": end_time,
            "queryString": "fields size | sort size asc",
        }),
    )
    .await;
    tokio::time::sleep(past_completion()).await;
    let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
    let produced = rows(&answer);
    let ordered: Vec<String> = produced.iter().map(|row| value(row, "size")).collect();
    assert_eq!(
        ordered,
        vec![
            "9".to_string(),
            "10".to_string(),
            "1a".to_string(),
            String::new()
        ],
        "case ascending: numbers in order, then text, then the event carrying none, got \
         {produced:?}"
    );

    // Descending is that same order reversed, so one rule places the rows either way.
    let id = start(
        &logs,
        &json!({
            "logGroupName": MIXED_GROUP,
            "startTime": start_time,
            "endTime": end_time,
            "queryString": "fields size | sort size desc",
        }),
    )
    .await;
    tokio::time::sleep(past_completion()).await;
    let answer = logs.ok(RESULTS, &json!({"queryId": id})).await;
    let produced = rows(&answer);
    let ordered: Vec<String> = produced.iter().map(|row| value(row, "size")).collect();
    assert_eq!(
        ordered,
        vec![
            String::new(),
            "1a".to_string(),
            "10".to_string(),
            "9".to_string()
        ],
        "case descending: the same order reversed, got {produced:?}"
    );
}

/// A `queryId` this account holds no query under is refused as the incorrectly specified
/// parameter it is: a results handle that was never issued was never valid.
#[tokio::test]
async fn an_unheld_query_id_is_refused_as_an_incorrectly_specified_parameter() {
    let logs = Logs::new();
    let (_, start_time, end_time) = group_with_events(&logs).await;
    let held = start(
        &logs,
        &json!({
            "logGroupName": GROUP,
            "startTime": start_time,
            "endTime": end_time,
            "queryString": "fields @message",
        }),
    )
    .await;

    // Each id names no query of this account: one of the form the service mints, one that is
    // not that form at all, and one that is a held id with a character changed.
    let cases: [(&str, String); 3] = [
        (
            "an id of the form the service mints",
            "11112222-3333-4444-5555-666677778888".to_string(),
            ),
        ("an id of no particular form", "nope".to_string()),
        (
            "a held id with its last character changed",
            format!("{}z", &held[..held.len() - 1]),
        ),
    ];
    for (case, query_id) in cases {
        let (code, message) = logs.refusal(RESULTS, &json!({"queryId": query_id})).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code}"
        );
        assert_eq!(
            message,
            query_results_invalid_query_id_message(&query_id),
            "case {case:?}: the refusal names the id that was rejected, got {message:?}"
        );
    }
}
