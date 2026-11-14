// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartQuery` and `DescribeQueries`: starting a CloudWatch Logs
//! Insights query and reading the started query back.
//!
//! The rules under test:
//!
//! * a query string the service cannot COMPILE is refused `MalformedQueryException`, with
//!   the detail of what the compiler stopped on in the `QueryCompileError` the model declares
//!   for it;
//! * a request whose modelled time window is not a range — its `endTime` before its
//!   `startTime` — and one that does not name the log groups to query exactly once (one of
//!   `logGroupName`, `logGroupNames`, `logGroupIdentifiers`, or a `SOURCE` command in the
//!   query string) are refused `InvalidParameterException`;
//! * a request naming a log group this account does not hold is refused
//!   `ResourceNotFoundException`, and so is one whose `lookup` command names a lookup table
//!   this account does not hold;
//! * an accepted request mints a `queryId` and stores the query, which `DescribeQueries`
//!   then reports — `Scheduled` at first and `Complete` once the clock is past the spans the
//!   start armed, filtered by the log group, the status and the query language the request
//!   names.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rules generalise over rather than one request: which command of
//! the pipeline is at fault, whether the fault is a regular expression, a redefined ephemeral
//! field, an unknown command or an empty stage; which member names the log groups and how
//! many it names (both published bounds and past the maximum); and which end of the window is
//! out of order. Any one query string an issue happens to mention is a single row.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const START: &str = "StartQuery";
const DESCRIBE: &str = "DescribeQueries";

/// The modelled error codes the rules name.
const MALFORMED: &str = "MalformedQueryException";
const INVALID_PARAMETER: &str = "InvalidParameterException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The statuses a query passes through, as the model's `QueryStatus` enum spells them.
const SCHEDULED: &str = "Scheduled";
const COMPLETE: &str = "Complete";

/// How long a case waits before treating a started query as finished: past both spans the
/// start armed (`QUERY_RUNNING_DELAY_MS` + `QUERY_COMPLETION_DELAY_MS`), with room for the
/// test's own scheduling.
fn past_completion() -> Duration {
    Duration::from_millis(900)
}

/// A query string that compiles, used by every case whose subject is NOT the query string.
const VALID_QUERY: &str = "fields @timestamp, @message | filter @message like /ERROR/ | limit 20";

/// A log group and the window a query of it is started with, in the unit `StartQuery`
/// documents for its range: "epoch time, the number of SECONDS since January 1, 1970,
/// 00:00:00 UTC" — not the milliseconds every other timestamp of this service carries. The
/// group is created here, so its creation instant precedes the window every accepted case
/// uses.
async fn group_with_window(logs: &Logs, group: &str) -> (i64, i64) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    // Rounded UP to the next whole second, so the window's end is at or after the instant the
    // group was just created.
    let now_secs = now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// The `queryId` a successful `StartQuery` answered.
fn query_id(answer: &Value) -> String {
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// The queries `DescribeQueries` reports for `body`.
async fn described(logs: &Logs, body: &Value) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, body).await;
    page["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeQueries reports a queries list, got {page}"))
        .clone()
}

/// The one reported query carrying `id`, or `None` when none does.
async fn described_query(logs: &Logs, body: &Value, id: &str) -> Option<Value> {
    described(logs, body)
        .await
        .into_iter()
        .find(|query| query["queryId"].as_str() == Some(id))
}

/// Every query string the compiler must refuse, each with the fault it carries.
#[tokio::test]
async fn a_query_string_that_does_not_compile_is_refused_as_malformed() {
    let logs = Logs::new();
    const GROUP: &str = "/insights/compile";
    let (start_time, end_time) = group_with_window(&logs, GROUP).await;

    // (case, the query string, the text the compile error must mention)
    let cases: [(&str, &str, &str); 9] = [
        (
            "a parse regex with an unclosed capture group and no closing slash",
            r"fields @message | parse @message /(?<level",
            "regular expression",
        ),
        (
            "a parse regex with an unclosed capture group",
            r"fields @message | parse @message /(?<level>[A-Z]+/",
            "regular expression",
        ),
        (
            "a parse regex with a dangling quantifier",
            r"fields @message | parse @message /+/",
            "regular expression",
        ),
        (
            "a later fields command redefining an already-parsed field",
            r"parse @message /(?<duration>\d+)/ | fields duration as duration",
            "already defined",
        ),
        (
            "a stats command defining an already-parsed field a second time",
            r"parse @message /(?<duration>\d+)/ | stats count(*) as duration",
            "already defined",
        ),
        (
            "a second parse redefining an already-parsed field",
            r"parse @message /(?<duration>\d+)/ | parse @message /(?<duration>\S+)/",
            "already defined",
        ),
        (
            "a command the query language does not define",
            "fields @message | shuffle @message",
            "query command",
        ),
        (
            "an empty stage left by a trailing pipe",
            "fields @message | ",
            "empty command",
        ),
        (
            "a limit that is not a number of log events",
            "fields @message | limit none",
            "positive number",
        ),
    ];
    for (case, query_string, expected_detail) in cases {
        let (status, body) = logs
            .call(
                START,
                &json!({
                    "logGroupName": GROUP,
                    "startTime": start_time,
                    "endTime": end_time,
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
            code, MALFORMED,
            "case {case:?}: expected {MALFORMED}, got {code} with body {body}"
        );
        let detail = body["queryCompileError"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            detail.contains(expected_detail),
            "case {case:?}: the compile error must say what it stopped on \
             ({expected_detail:?}), got {body}"
        );
        // "Details about this error are displayed in a QueryCompileError object": the
        // location is part of that detail, so a client can point at the text at fault.
        let location = &body["queryCompileError"]["location"];
        let (start_offset, end_offset) = (
            location["startCharOffset"].as_i64(),
            location["endCharOffset"].as_i64(),
        );
        assert!(
            start_offset.is_some()
                && end_offset.is_some()
                && start_offset <= end_offset
                && end_offset.unwrap_or_default() <= query_string.chars().count() as i64,
            "case {case:?}: the compile error locates the text at fault inside the query, got \
             {body}"
        );
    }

    // An empty query string compiles to no pipeline at all, which is the same refusal.
    let code = logs
        .err(
            START,
            &json!({
                "logGroupName": GROUP,
                "startTime": start_time,
                "endTime": end_time,
                "queryString": "",
            }),
        )
        .await;
    assert_eq!(
        code, MALFORMED,
        "case an empty query string: expected {MALFORMED}, got {code}"
    );
}

/// Query strings that DO compile are accepted, and each one is stored under the id the start
/// answered.
#[tokio::test]
async fn a_query_string_that_compiles_is_accepted_and_stored() {
    let logs = Logs::new();
    const GROUP: &str = "/insights/accepted";
    let (start_time, end_time) = group_with_window(&logs, GROUP).await;

    // (case, the query string) — one row per form the pipeline takes.
    let cases: [(&str, &str); 7] = [
        ("one command", "fields @message"),
        (
            "a filter over a quoted string carrying a pipe",
            r#"fields @message | filter @message = "a|b" | limit 1"#,
        ),
        (
            "a parse regex whose alternation carries a pipe",
            r"fields @message | parse @message /(?<level>WARN|ERROR)/",
        ),
        (
            "a filter whose regular expression carries a pipe",
            r"fields @message | filter @message like /WARN|ERROR/ | limit 5",
        ),
        (
            "a parse that defines a field a later command reads",
            r"parse @message /(?<duration>\d+)/ | stats avg(duration) as mean",
        ),
        (
            "a glob parse defining two fields",
            r#"parse @message "* [*]" as user, level | sort level desc"#,
        ),
        (
            "a stats command grouped into bins",
            "stats count(*) as total by bin(5m)",
        ),
    ];
    let mut started: Vec<String> = Vec::new();
    for (case, query_string) in cases {
        let answer = logs
            .ok(
                START,
                &json!({
                    "logGroupName": GROUP,
                    "startTime": start_time,
                    "endTime": end_time,
                    "queryString": query_string,
                }),
            )
            .await;
        let id = query_id(&answer);
        assert!(
            !started.contains(&id),
            "case {case:?}: every started query gets an id no earlier one used, got {id}"
        );
        let reported = described_query(&logs, &json!({}), &id)
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
        started.push(id);
    }
}

/// The window and the log-group selection, which are judged as PARAMETERS rather than as
/// query syntax.
#[tokio::test]
async fn the_window_and_the_log_group_selection_are_judged_as_parameters() {
    let logs = Logs::new();
    const GROUP: &str = "/insights/parameters";
    const OTHER_GROUP: &str = "/insights/parameters-two";
    logs.ok("CreateLogGroup", &json!({"logGroupName": OTHER_GROUP}))
        .await;
    let (start_time, end_time) = group_with_window(&logs, GROUP).await;
    let (_, group_arn) = logs.group_arns(GROUP).await;

    // (case, the request, the modelled code it is refused under)
    let too_many: Vec<String> = (0..51).map(|index| format!("/insights/g{index}")).collect();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an endTime one millisecond before the startTime",
            json!({
                "logGroupName": GROUP,
                "startTime": start_time,
                "endTime": start_time - 1,
                "queryString": VALID_QUERY,
            }),
            INVALID_PARAMETER,
        ),
        (
            "a window whose ends are swapped",
            json!({
                "logGroupName": GROUP,
                "startTime": end_time,
                "endTime": start_time,
                "queryString": VALID_QUERY,
            }),
            INVALID_PARAMETER,
        ),
        (
            "two of the three log-group members",
            json!({
                "logGroupName": GROUP,
                "logGroupNames": [OTHER_GROUP],
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
            INVALID_PARAMETER,
        ),
        (
            "no log-group member and no SOURCE command",
            json!({
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
            INVALID_PARAMETER,
        ),
        (
            "one more log group than a query may include",
            json!({
                "logGroupNames": too_many,
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
            INVALID_PARAMETER,
        ),
        (
            "a log group name this account does not hold",
            json!({
                "logGroupName": "/insights/no-such-group",
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
            NOT_FOUND,
        ),
        (
            "a log group ARN this account does not hold",
            json!({
                "logGroupIdentifiers": [format!("{group_arn}-missing")],
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
            NOT_FOUND,
        ),
        (
            "one held log group and one this account does not hold",
            json!({
                "logGroupNames": [GROUP, "/insights/no-such-group"],
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
            NOT_FOUND,
        ),
        (
            // The range covers no instant of the group's lifetime: there was no log group to
            // query for the whole of it, so the query is not one the service runs.
            "a window that ends before the log group was created",
            json!({
                "logGroupName": GROUP,
                "startTime": 0,
                "endTime": 1_000,
                "queryString": VALID_QUERY,
            }),
            MALFORMED,
        ),
    ];
    for (case, body, expected) in cases {
        let (status, response) = logs.call(START, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {response}"
        );
        let code = crate::harness::error_code(&response);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with body {response}"
        );
    }

    // The accepted selections: each member that names log groups, at its published minimum
    // and above it, and a query whose own SOURCE command selects them instead.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "logGroupName",
            json!({"logGroupName": GROUP, "queryString": VALID_QUERY}),
        ),
        (
            "logGroupNames with one group",
            json!({"logGroupNames": [GROUP], "queryString": VALID_QUERY}),
        ),
        (
            "logGroupNames with two groups",
            json!({"logGroupNames": [GROUP, OTHER_GROUP], "queryString": VALID_QUERY}),
        ),
        (
            "logGroupIdentifiers naming the group by ARN",
            json!({"logGroupIdentifiers": [group_arn], "queryString": VALID_QUERY}),
        ),
        (
            "a SOURCE command instead of a member",
            json!({
                "queryString":
                    r#"SOURCE logGroups(namePrefix: ["/insights/"]) | fields @message"#,
            }),
        ),
        (
            "a window of a single instant",
            json!({
                "logGroupName": GROUP,
                "startTime": end_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
        ),
        (
            // The range may open long before the group existed: what it has to cover is the
            // group's lifetime, which a range ending in the present does.
            "a window opening at the published minimum of the range",
            json!({
                "logGroupName": GROUP,
                "startTime": 0,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
        ),
    ];
    for (case, body) in accepted {
        let mut request = body;
        for (member, value) in [("startTime", start_time), ("endTime", end_time)] {
            if request.get(member).is_none() {
                request[member] = json!(value);
            }
        }
        let answer = logs.ok(START, &request).await;
        assert!(
            !query_id(&answer).is_empty(),
            "case {case:?}: an accepted start answers a queryId, got {answer}"
        );
    }

    // A query over TWO log groups reports no single `logGroupName`: "The name of the log
    // group scanned by this query" names one group.
    let answer = logs
        .ok(
            START,
            &json!({
                "logGroupNames": [GROUP, OTHER_GROUP],
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
        )
        .await;
    let id = query_id(&answer);
    let reported = described_query(&logs, &json!({}), &id)
        .await
        .expect("the started query must be reported");
    assert!(
        reported["logGroupName"].is_null(),
        "case two log groups: a query of several groups reports no single log group, got \
         {reported}"
    );
    // … and it is reported under EACH of the groups it reads.
    for group in [GROUP, OTHER_GROUP] {
        assert!(
            described_query(&logs, &json!({"logGroupName": group}), &id)
                .await
                .is_some(),
            "case two log groups: the query must be reported for {group}"
        );
    }
}

/// The status a started query is in is the one the clock already made due, and the filters
/// `DescribeQueries` takes select on what the store holds.
#[tokio::test]
async fn a_started_query_reaches_complete_on_the_clock_and_is_filtered_by_what_it_holds() {
    let logs = Logs::new();
    const GROUP: &str = "/insights/lifecycle";
    const OTHER_GROUP: &str = "/insights/lifecycle-two";
    logs.ok("CreateLogGroup", &json!({"logGroupName": OTHER_GROUP}))
        .await;
    // The instant the start is made at, in the milliseconds `QueryInfo$createTime` carries —
    // which is NOT the unit the query's own window is in.
    let before_ms = now_ms();
    let (start_time, end_time) = group_with_window(&logs, GROUP).await;

    let answer = logs
        .ok(
            START,
            &json!({
                "logGroupName": GROUP,
                "startTime": start_time,
                "endTime": end_time,
                "queryString": VALID_QUERY,
            }),
        )
        .await;
    let id = query_id(&answer);
    let scheduled = described_query(&logs, &json!({}), &id)
        .await
        .expect("the started query must be reported");
    assert_eq!(
        scheduled["status"].as_str(),
        Some(SCHEDULED),
        "case straight after the start: a query that has not run yet is {SCHEDULED}, got \
         {scheduled}"
    );
    assert!(
        scheduled["createTime"]
            .as_i64()
            .is_some_and(|created| created >= before_ms),
        "case straight after the start: the reported createTime is when the query was \
         started, got {scheduled}"
    );

    // A second query of the other group, so the filters below have something to leave out.
    let other = query_id(
        &logs
            .ok(
                START,
                &json!({
                    "logGroupName": OTHER_GROUP,
                    "startTime": start_time,
                    "endTime": end_time,
                    "queryString": "fields @message",
                }),
            )
            .await,
    );

    tokio::time::sleep(past_completion()).await;
    let completed = described_query(&logs, &json!({}), &id)
        .await
        .expect("the query must still be reported");
    assert_eq!(
        completed["status"].as_str(),
        Some(COMPLETE),
        "case past the spans the start armed: the query is {COMPLETE}, got {completed}"
    );
    assert!(
        completed["queryDuration"].is_null(),
        "case past the spans the start armed: nothing executed the query, so no duration is \
         reported for it, got {completed}"
    );

    // (case, the filter, the ids it must report)
    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        ("no filter", json!({}), vec![&id, &other]),
        (
            "the log group of the first query",
            json!({"logGroupName": GROUP}),
            vec![&id],
        ),
        (
            "the status every query has reached",
            json!({"status": COMPLETE}),
            vec![&id, &other],
        ),
        (
            "a status no query is in",
            json!({"status": SCHEDULED}),
            vec![],
        ),
        (
            "the query language every query used",
            json!({"queryLanguage": "CWLI"}),
            vec![&id, &other],
        ),
        (
            "a query language no query used",
            json!({"queryLanguage": "SQL"}),
            vec![],
        ),
    ];
    for (case, filter, expected) in cases {
        let reported: Vec<String> = described(&logs, &filter)
            .await
            .iter()
            .map(|query| query["queryId"].as_str().unwrap_or_default().to_string())
            .collect();
        for id in &expected {
            assert!(
                reported.contains(id),
                "case {case:?}: expected the filter to report {id}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected {} queries, got {reported:?}",
            expected.len()
        );
    }

    // A log group the account does not hold is a missing resource rather than an empty page.
    let code = logs
        .err(DESCRIBE, &json!({"logGroupName": "/insights/no-such-group"}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case a filter naming a missing log group: expected {NOT_FOUND}, got {code}"
    );

    // Reporting is a read: calling it twice reports the same thing.
    let first = described(&logs, &json!({})).await;
    let second = described(&logs, &json!({})).await;
    assert_eq!(
        first, second,
        "case twice: reporting the queries changes nothing a later caller sees"
    );
}
