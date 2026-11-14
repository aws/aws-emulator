// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the first call of a scheduled-query workflow: `StartQuery` over log
//! groups that already hold ingested events, named the way a scheduled query names them.
//!
//! The rule under test: `StartQuery` is SERVED — it compiles the query string, resolves the
//! log groups the request selects and mints a `queryId` the store then holds the query under —
//! for every request a scheduled-query workflow makes. In particular:
//!
//! * the time range selects which EVENTS the query reads, so a range spanning the group's
//!   events, one opening on a single one of them and one holding none at all are all accepted —
//!   as long as the range reaches the instant the log group was created, since a range closing
//!   before that covers no instant of the group's lifetime;
//! * a log group is resolved from whichever form of `logGroupIdentifiers` names it — the
//!   name, the group's ARN, or the ARN of one of its streams — and two forms naming ONE group
//!   select one group, so the started query reports that single `logGroupName`, while the ARN
//!   version carrying the trailing wildcard names no log group to query and is refused;
//! * the query string and log groups a scheduled query is configured with start an interactive
//!   query of the same log data, whatever destination the schedule delivers its results to.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so an answer
//! a handler does not compute from the shared store cannot pass, and an operation that is not
//! implemented at all cannot pass either.
//!
//! The tables span what the rule generalises over rather than one request: where the window
//! falls relative to the events the group holds (spanning them, one instant on one of them,
//! wholly before, wholly after, from the epoch), which form names the log groups and how many
//! groups one request selects, and which pipeline a scheduled query carries. Any one request
//! an issue happens to cite is a single row.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, group_with_stream, now_ms, Logs};

/// The operations under test, and the reads each is proved through.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";
const CREATE_SCHEDULED: &str = "CreateScheduledQuery";
const GET_SCHEDULED: &str = "GetScheduledQuery";

/// The code a request whose parameters are "specified incorrectly" is refused under.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The log group every case queries, and the stream its events are written to.
const GROUP: &str = "/aws/emulator/scheduled-query";
const STREAM: &str = "deliveries";

/// A second log group, so a case selecting two groups has one to add and the filters have one
/// to leave out.
const OTHER_GROUP: &str = "/aws/emulator/scheduled-query-two";

/// The IAM roles a scheduled query executes and delivers under, which the model gives their
/// own members (`CreateScheduledQuery$executionRoleArn`, `S3Configuration$roleArn`).
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";
const DELIVERY_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-delivery";

/// How far apart in TIME the seeded events are, which is also the resolution the query's own
/// window is expressed in: `StartQuery$startTime` / `$endTime` are "epoch time, the number of
/// seconds since January 1, 1970, 00:00:00 UTC", so one second per event keeps every row's
/// window able to name a single event.
const EVENT_SPACING_MS: i64 = 1_000;

/// The messages seeded into the group, one per `EVENT_SPACING_MS`.
const MESSAGES: [&str; 3] = [
    "ERROR order 1 failed",
    "INFO order 2 accepted",
    "ERROR order 3 failed",
];

/// Seed `GROUP` with [`MESSAGES`], one event per second, and answer the epoch SECOND the first
/// of them falls in — the unit the query's window is expressed in.
async fn seed_events(logs: &Logs) -> i64 {
    group_with_stream(logs, GROUP, STREAM, None).await;
    logs.ok("CreateLogGroup", &json!({"logGroupName": OTHER_GROUP}))
        .await;
    // Whole seconds, so a window naming one second names exactly one event.
    let first_ms = base_ms() / 1_000 * 1_000;
    let events: Vec<(i64, &str)> = MESSAGES
        .iter()
        .enumerate()
        .map(|(index, message)| (first_ms + index as i64 * EVENT_SPACING_MS, *message))
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
    // The group really holds the events every window below is placed against.
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
        )
        .await;
    let stored = page["events"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        stored,
        MESSAGES.len(),
        "the seeded events must be stored before a query reads them, got {page}"
    );
    first_ms / 1_000
}

/// The instant every window below closes at: the present, rounded UP to the next whole second
/// so the range reaches the instant the log groups queried were created. A range that closes
/// before a log group it reads existed covers no instant of that group's lifetime and is
/// refused, which is the subject of `tests/all/query_window_before_group_creation.rs`; the
/// seeded events are an hour old, so where a window OPENS is what varies here.
fn horizon() -> i64 {
    now_ms() / 1_000 + 1
}

/// The `queryId` a successful `StartQuery` answered.
fn query_id(case: &str, answer: &Value) -> String {
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// Start `request` as a client does, asserting the operation is SERVED rather than refused,
/// and answer the `queryId` it minted.
async fn started(logs: &Logs, case: &str, request: &Value) -> String {
    let (status, body) = logs.call(START, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartQuery must be served and answer a queryId, got {status} with body \
         {body}"
    );
    let id = query_id(case, &body);
    assert!(
        !id.is_empty(),
        "case {case:?}: the minted queryId identifies the query, got {body}"
    );
    id
}

/// The one query `DescribeQueries` reports under `id` for `filter`, or `None` when none does.
async fn described(logs: &Logs, filter: &Value, id: &str) -> Option<Value> {
    let page = logs.ok(DESCRIBE, filter).await;
    page["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueries reports a queries list, got {page}"))
        .iter()
        .find(|query| query["queryId"].as_str() == Some(id))
        .cloned()
}

/// Where the window OPENS selects which events a query reads; it is not a precondition on
/// starting one.
#[tokio::test]
async fn a_query_of_a_group_holding_events_is_served_for_every_window_in_the_class() {
    let logs = Logs::new();
    let first = seed_events(&logs).await;
    let last = first + (MESSAGES.len() as i64 - 1);
    let query_string = "fields @timestamp, @message | filter @message like /ERROR/";

    // (case, startTime, endTime) — where the window opens relative to the seeded events, and
    // how far past the present it closes.
    let cases: [(&str, i64, i64); 7] = [
        (
            "a window spanning every seeded event",
            first - 60,
            horizon(),
        ),
        ("a window opening on the oldest event", first, horizon()),
        ("a window opening on the newest event", last, horizon()),
        (
            "a window opening an hour before every seeded event",
            first - 3_600,
            horizon(),
        ),
        (
            "a window opening after every seeded event, holding none",
            last + 1_800,
            horizon(),
        ),
        ("a window that opens at the epoch", 0, horizon()),
        (
            "a window closing a day after the present",
            first - 60,
            horizon() + 24 * 60 * 60,
        ),
    ];
    let mut minted: Vec<String> = Vec::new();
    for (case, start_time, end_time) in cases {
        let id = started(
            &logs,
            case,
            &json!({
                "logGroupName": GROUP,
                "startTime": start_time,
                "endTime": end_time,
                "queryString": query_string,
            }),
        )
        .await;
        assert!(
            !minted.contains(&id),
            "case {case:?}: every started query gets an id no earlier one used, got {id}"
        );
        // The query is in the store, not merely acknowledged: it is reported back under the id
        // the start answered, over the log group it reads, with the query string it was
        // started with.
        let reported = described(&logs, &json!({}), &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the started query must be reported"));
        assert_eq!(
            reported["queryString"].as_str(),
            Some(query_string),
            "case {case:?}: the reported query string is the one that was started, got {reported}"
        );
        assert_eq!(
            reported["logGroupName"].as_str(),
            Some(GROUP),
            "case {case:?}: the reported log group is the one queried, got {reported}"
        );
        minted.push(id);
    }
}

/// One log-group selection a start names, and what the store must then hold for it.
struct Selection {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The `logGroupIdentifiers` the request names.
    identifiers: Vec<String>,
    /// The single `QueryInfo$logGroupName` the started query reports — "The name of the log
    /// group scanned by this query" — or `None` for a query that scans several.
    scanned: Option<&'static str>,
    /// Every log group `DescribeQueries` must report the query under.
    reported_under: Vec<&'static str>,
}

/// Every form that names the log groups to query resolves to the groups the store keys by
/// name, so what the start selects is the same set however the request spells it.
#[tokio::test]
async fn the_log_groups_are_resolved_from_every_identifier_form() {
    let logs = Logs::new();
    let first = seed_events(&logs).await;
    let (wildcard_arn, group_arn) = logs.group_arns(GROUP).await;
    let (_, other_arn) = logs.group_arns(OTHER_GROUP).await;
    let stream_arn = format!("{group_arn}:log-stream:{STREAM}");
    let stream_arn_wildcard = format!("{stream_arn}:*");
    let query_string = "fields @timestamp, @message";

    let cases: Vec<Selection> = vec![
        Selection {
            case: "the group's name",
            identifiers: vec![GROUP.to_string()],
            scanned: Some(GROUP),
            reported_under: vec![GROUP],
        },
        Selection {
            case: "the group's ARN",
            identifiers: vec![group_arn.clone()],
            scanned: Some(GROUP),
            reported_under: vec![GROUP],
        },
        Selection {
            case: "the ARN of one of the group's streams",
            identifiers: vec![stream_arn.clone()],
            scanned: Some(GROUP),
            reported_under: vec![GROUP],
        },
        // Two spellings of ONE group select one group, so the query still reports the single
        // log group it scans.
        Selection {
            case: "two forms naming the same group",
            identifiers: vec![GROUP.to_string(), group_arn.clone()],
            scanned: Some(GROUP),
            reported_under: vec![GROUP],
        },
        // "The name of the log group scanned by this query" names one group, so a query over
        // two reports none — and is reported under each.
        Selection {
            case: "two different groups",
            identifiers: vec![group_arn.clone(), other_arn.clone()],
            scanned: None,
            reported_under: vec![GROUP, OTHER_GROUP],
        },
    ];
    for Selection {
        case,
        identifiers,
        scanned,
        reported_under,
    } in cases
    {
        let id = started(
            &logs,
            case,
            &json!({
                "logGroupIdentifiers": identifiers,
                "startTime": first - 60,
                "endTime": horizon(),
                "queryString": query_string,
            }),
        )
        .await;
        let reported = described(&logs, &json!({}), &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the started query must be reported"));
        assert_eq!(
            reported["logGroupName"].as_str(),
            scanned,
            "case {case:?}: expected the scanned log group {scanned:?}, got {reported}"
        );
        for group in &reported_under {
            assert!(
                described(&logs, &json!({"logGroupName": group}), &id)
                    .await
                    .is_some(),
                "case {case:?}: the query must be reported for {group}"
            );
        }
        // … and only under those: a group the query does not read does not report it.
        for group in [GROUP, OTHER_GROUP] {
            if reported_under.contains(&group) {
                continue;
            }
            assert!(
                described(&logs, &json!({"logGroupName": group}), &id)
                    .await
                    .is_none(),
                "case {case:?}: the query reads no events of {group}, so it is not reported \
                 for it"
            );
        }
    }

    // The ARN version carrying the trailing wildcard is NOT a form that names a log group to
    // query: "If you specify an ARN, use the format
    // arn:aws:logs:region:account-id:log-group:log_group_name Don't include an * at the end"
    // (`StartQuery$logGroupIdentifiers`) — and the member's own modelled pattern
    // (`smithy_query(query="shape",shape="LogGroupIdentifier")`) admits no `*` either, so the
    // request is refused as the incorrectly specified parameter it is.
    for (case, identifier) in [
        ("the ARN with the trailing wildcard", wildcard_arn.as_str()),
        (
            "a stream ARN with the trailing wildcard",
            stream_arn_wildcard.as_str(),
        ),
    ] {
        let (status, body) = logs
            .call(
                START,
                &json!({
                    "logGroupIdentifiers": [identifier],
                    "startTime": first - 60,
                    "endTime": horizon(),
                    "queryString": query_string,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} with body {body}"
        );
    }
}

/// What a scheduled query is configured to run — its query string over its log groups — starts
/// an interactive query of the same log data, whichever destination its results are delivered
/// to.
#[tokio::test]
async fn a_scheduled_querys_own_configuration_starts_an_interactive_query() {
    let logs = Logs::new();
    let first = seed_events(&logs).await;
    let (_, group_arn) = logs.group_arns(GROUP).await;

    // (case, the scheduled query's name, its query string, the log groups it reads, where its
    // results are delivered)
    let cases: Vec<(&str, &str, &str, Vec<String>, Value)> = vec![
        (
            "a filtering pipeline delivered to Amazon S3",
            "orders-errors-to-s3",
            "fields @timestamp, @message | filter @message like /ERROR/ | limit 20",
            vec![group_arn.clone()],
            json!({"s3Configuration": {
                "destinationIdentifier": "s3://scheduled-query-results/orders/errors",
                "roleArn": DELIVERY_ROLE,
            }}),
        ),
        (
            "an aggregating pipeline delivered to Amazon S3 under a prefix",
            "orders-counts-to-s3",
            "stats count(*) as total by bin(5m)",
            vec![GROUP.to_string()],
            json!({"s3Configuration": {
                "destinationIdentifier": "s3://scheduled-query-results/orders/counts",
                "roleArn": DELIVERY_ROLE,
                "ownerAccountId": "000000000000",
            }}),
        ),
        (
            "a pipeline over two log groups delivered to a lookup table",
            "orders-rows-to-lookup-table",
            "fields @timestamp, @logStream, @message | sort @timestamp desc",
            vec![group_arn.clone(), OTHER_GROUP.to_string()],
            json!({"lookupTableConfiguration": {
                "tableName": "scheduled_query_rows",
                "roleArn": DELIVERY_ROLE,
            }}),
        ),
    ];
    for (case, name, query_string, identifiers, destination) in cases {
        logs.ok(
            CREATE_SCHEDULED,
            &json!({
                "name": name,
                "queryLanguage": "CWLI",
                "queryString": query_string,
                "scheduleExpression": "cron(*/5 * * * ? *)",
                "executionRoleArn": EXECUTION_ROLE,
                "logGroupIdentifiers": identifiers,
                "destinationConfiguration": destination,
            }),
        )
        .await;

        // The configuration the schedule will run is read back through the service, so the
        // query started below is the stored one rather than the test's own copy.
        let scheduled = logs.ok(GET_SCHEDULED, &json!({"identifier": name})).await;
        assert_eq!(
            scheduled["queryString"].as_str(),
            Some(query_string),
            "case {case:?}: the scheduled query runs the string it was created with, got \
             {scheduled}"
        );
        assert_eq!(
            scheduled["destinationConfiguration"], destination,
            "case {case:?}: the results are delivered where the create said, got {scheduled}"
        );
        let configured = scheduled["logGroupIdentifiers"].clone();
        assert_eq!(
            configured,
            json!(identifiers),
            "case {case:?}: the scheduled query reads the log groups it was created with, got \
             {scheduled}"
        );

        let id = started(
            &logs,
            case,
            &json!({
                "logGroupIdentifiers": configured,
                "startTime": first - 60,
                "endTime": horizon(),
                "queryString": scheduled["queryString"].clone(),
            }),
        )
        .await;
        let reported = described(&logs, &json!({}), &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the started query must be reported"));
        assert_eq!(
            reported["queryString"].as_str(),
            Some(query_string),
            "case {case:?}: the started query is the scheduled query's own, got {reported}"
        );
        assert_eq!(
            reported["queryLanguage"].as_str(),
            Some("CWLI"),
            "case {case:?}: the query language is the one the pipeline is written in, got \
             {reported}"
        );
    }
}
